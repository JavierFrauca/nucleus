//! A small boolean **query language** for filtering chunks at search time.
//!
//! Grammar (case-insensitive keywords, `AND` binds tighter than `OR`):
//!
//! ```text
//! expr    := or
//! or      := and ( "OR"  and )*
//! and     := unary ( "AND" unary )*
//! unary   := "NOT" unary | primary
//! primary := "(" expr ")" | term
//! term    := "tag:" VALUE | "doc:" NUMBER | "meta." KEY ":" VALUE
//! ```
//!
//! `VALUE` is a bareword or a `"quoted string"` (quotes allow spaces). Examples:
//!
//! ```text
//! tag:legal AND NOT tag:draft
//! tag:legal AND (meta.lang:es OR meta.lang:en)
//! doc:42 OR tag:"contrato marco"
//! ```

use std::collections::HashMap;

use crate::error::NucleusError;
use crate::id::{DocumentId, TagId};
use crate::model::Chunk;
use crate::Result;

/// A parsed filter expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    /// Chunk carries the tag with this name.
    Tag(String),
    /// Chunk belongs to this document.
    Doc(u64),
    /// Chunk metadata `key` equals `value`.
    Meta(String, String),
}

impl Expr {
    /// Evaluate the expression against a chunk. `tags` maps tag *names* to ids
    /// within the chunk's domain (an unknown name simply never matches).
    pub fn matches(&self, chunk: &Chunk, tags: &HashMap<String, TagId>) -> bool {
        match self {
            Expr::And(a, b) => a.matches(chunk, tags) && b.matches(chunk, tags),
            Expr::Or(a, b) => a.matches(chunk, tags) || b.matches(chunk, tags),
            Expr::Not(e) => !e.matches(chunk, tags),
            Expr::Tag(name) => tags.get(name).is_some_and(|id| chunk.tags.contains(id)),
            Expr::Doc(id) => chunk.document_id == DocumentId::new(*id),
            Expr::Meta(key, value) => chunk.metadata.get(key).is_some_and(|v| v == value),
        }
    }
}

fn err(msg: impl Into<String>) -> NucleusError {
    NucleusError::invalid(format!("query: {}", msg.into()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    LParen,
    RParen,
    And,
    Or,
    Not,
    Term { field: String, value: String },
}

fn lex(input: &str) -> Result<Vec<Tok>> {
    let chars: Vec<char> = input.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '(' {
            toks.push(Tok::LParen);
            i += 1;
            continue;
        }
        if c == ')' {
            toks.push(Tok::RParen);
            i += 1;
            continue;
        }
        // Read a word, honouring "quoted" segments (which may contain spaces).
        let mut buf = String::new();
        while i < chars.len() {
            let c = chars[i];
            if c.is_whitespace() || c == '(' || c == ')' {
                break;
            }
            if c == '"' {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    buf.push(chars[i]);
                    i += 1;
                }
                if i >= chars.len() {
                    return Err(err("unterminated quoted string"));
                }
                i += 1; // closing quote
                continue;
            }
            buf.push(c);
            i += 1;
        }
        match buf.to_ascii_uppercase().as_str() {
            "AND" => toks.push(Tok::And),
            "OR" => toks.push(Tok::Or),
            "NOT" => toks.push(Tok::Not),
            _ => {
                let Some(idx) = buf.find(':') else {
                    return Err(err(format!("expected `field:value`, found `{buf}`")));
                };
                toks.push(Tok::Term {
                    field: buf[..idx].to_string(),
                    value: buf[idx + 1..].to_string(),
                });
            }
        }
    }
    Ok(toks)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn parse_or(&mut self) -> Result<Expr> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Some(Tok::Or)) {
            self.pos += 1;
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr> {
        let mut left = self.parse_unary()?;
        while matches!(self.peek(), Some(Tok::And)) {
            self.pos += 1;
            let right = self.parse_unary()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr> {
        if matches!(self.peek(), Some(Tok::Not)) {
            self.pos += 1;
            Ok(Expr::Not(Box::new(self.parse_unary()?)))
        } else {
            self.parse_primary()
        }
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        match self.toks.get(self.pos).cloned() {
            Some(Tok::LParen) => {
                self.pos += 1;
                let inner = self.parse_or()?;
                match self.toks.get(self.pos) {
                    Some(Tok::RParen) => {
                        self.pos += 1;
                        Ok(inner)
                    }
                    _ => Err(err("expected `)`")),
                }
            }
            Some(Tok::Term { field, value }) => {
                self.pos += 1;
                term_expr(&field, &value)
            }
            other => Err(err(format!("unexpected token: {other:?}"))),
        }
    }
}

fn term_expr(field: &str, value: &str) -> Result<Expr> {
    let lower = field.to_ascii_lowercase();
    if lower == "tag" {
        Ok(Expr::Tag(value.to_string()))
    } else if lower == "doc" {
        value
            .parse::<u64>()
            .map(Expr::Doc)
            .map_err(|_| err(format!("`doc` id must be a number, found `{value}`")))
    } else if let Some(key) = lower.strip_prefix("meta.") {
        if key.is_empty() {
            return Err(err("`meta.` needs a key, e.g. `meta.lang:es`"));
        }
        // Preserve the original-case key.
        Ok(Expr::Meta(field[5..].to_string(), value.to_string()))
    } else {
        Err(err(format!(
            "unknown field `{field}` (use tag, doc, meta.*)"
        )))
    }
}

/// Parse a filter string into an [`Expr`].
pub fn parse(input: &str) -> Result<Expr> {
    let toks = lex(input)?;
    if toks.is_empty() {
        return Err(err("empty query"));
    }
    let mut parser = Parser { toks, pos: 0 };
    let expr = parser.parse_or()?;
    if parser.pos != parser.toks.len() {
        return Err(err("unexpected trailing input"));
    }
    Ok(expr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{ChunkId, DocumentId, DomainId};
    use std::collections::BTreeMap;

    fn parses(s: &str) -> Expr {
        parse(s).unwrap()
    }

    #[test]
    fn precedence_and_over_or() {
        // a OR b AND c  ==  a OR (b AND c)
        let e = parses("tag:a OR tag:b AND tag:c");
        assert_eq!(
            e,
            Expr::Or(
                Box::new(Expr::Tag("a".into())),
                Box::new(Expr::And(
                    Box::new(Expr::Tag("b".into())),
                    Box::new(Expr::Tag("c".into())),
                )),
            )
        );
    }

    #[test]
    fn parens_quotes_and_fields() {
        assert_eq!(parses("doc:42"), Expr::Doc(42));
        assert_eq!(
            parses("meta.lang:es"),
            Expr::Meta("lang".into(), "es".into())
        );
        assert_eq!(
            parses("tag:\"contrato marco\""),
            Expr::Tag("contrato marco".into())
        );
        // grouping overrides precedence
        let e = parses("(tag:a OR tag:b) AND NOT tag:c");
        assert!(matches!(e, Expr::And(_, _)));
    }

    #[test]
    fn rejects_malformed() {
        assert!(parse("tag legal").is_err()); // no colon
        assert!(parse("doc:abc").is_err()); // non-numeric doc
        assert!(parse("(tag:a").is_err()); // missing paren
        assert!(parse("").is_err()); // empty
        assert!(parse("weird:x").is_err()); // unknown field
    }

    #[test]
    fn evaluation() {
        let mut meta = BTreeMap::new();
        meta.insert("lang".to_string(), "es".to_string());
        let chunk = Chunk {
            id: ChunkId::new(1),
            document_id: DocumentId::new(7),
            domain_id: DomainId::new(1),
            subdomain_id: None,
            ordinal: 0,
            text: String::new(),
            tags: vec![TagId::new(10)],
            metadata: meta,
            prev: None,
            next: None,
        };
        let mut names = HashMap::new();
        names.insert("legal".to_string(), TagId::new(10));
        names.insert("draft".to_string(), TagId::new(11));

        assert!(parses("tag:legal").matches(&chunk, &names));
        assert!(!parses("tag:draft").matches(&chunk, &names));
        assert!(parses("tag:legal AND meta.lang:es").matches(&chunk, &names));
        assert!(!parses("tag:legal AND meta.lang:en").matches(&chunk, &names));
        assert!(parses("tag:draft OR doc:7").matches(&chunk, &names));
        assert!(parses("NOT tag:draft").matches(&chunk, &names));
    }
}
