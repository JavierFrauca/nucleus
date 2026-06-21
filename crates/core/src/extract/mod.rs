//! In-engine text extraction from common document formats.
//!
//! Ingestion is **transparent**: the client uploads the raw file and Nucleus
//! detects the format and extracts the text. Supported: `txt`/`md`/`csv` (plain),
//! `html`, `pdf`, `xls`/`xlsx`/`ods` (spreadsheets), `docx`. Legacy binary `.doc`
//! is not supported in pure Rust — convert to `.docx`/`.pdf`.

use std::io::{Cursor, Read};

use crate::error::NucleusError;
use crate::Result;

/// Extract plain text from `bytes`, choosing the parser by `filename` extension.
pub fn extract_text(filename: &str, bytes: &[u8]) -> Result<String> {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let text = match ext.as_str() {
        "txt" | "text" | "md" | "markdown" | "csv" | "tsv" | "log" => {
            String::from_utf8_lossy(bytes).into_owned()
        }
        "html" | "htm" | "xhtml" => extract_html(bytes)?,
        "pdf" => extract_pdf(bytes)?,
        "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" => extract_spreadsheet(bytes)?,
        "docx" => extract_docx(bytes)?,
        "doc" => {
            return Err(NucleusError::invalid(
                "formato .doc heredado no soportado; convierte a .docx o .pdf",
            ))
        }
        other => {
            return Err(NucleusError::invalid(format!(
                "formato no soportado: .{other}"
            )))
        }
    };
    Ok(normalize(&text))
}

/// Whether a filename's extension is one we can extract.
pub fn is_supported(filename: &str) -> bool {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "txt"
            | "text"
            | "md"
            | "markdown"
            | "csv"
            | "tsv"
            | "log"
            | "html"
            | "htm"
            | "xhtml"
            | "pdf"
            | "xlsx"
            | "xlsm"
            | "xlsb"
            | "xls"
            | "ods"
            | "docx"
    )
}

/// Collapse runs of whitespace and blank lines (extractors leave a lot of it).
fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank_run = 0;
    for line in s.lines() {
        let trimmed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            out.push_str(&trimmed);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

fn extract_pdf(bytes: &[u8]) -> Result<String> {
    pdf_extract::extract_text_from_mem(bytes)
        .map_err(|e| NucleusError::invalid(format!("no se pudo extraer el PDF: {e}")))
}

fn extract_html(bytes: &[u8]) -> Result<String> {
    Ok(html2text::from_read(bytes, 100))
}

fn extract_spreadsheet(bytes: &[u8]) -> Result<String> {
    use calamine::{open_workbook_auto_from_rs, Data, Reader};

    let cursor = Cursor::new(bytes.to_vec());
    let mut workbook = open_workbook_auto_from_rs(cursor)
        .map_err(|e| NucleusError::invalid(format!("hoja de cálculo: {e}")))?;
    let mut out = String::new();
    for name in workbook.sheet_names().to_owned() {
        let Ok(range) = workbook.worksheet_range(&name) else {
            continue;
        };
        out.push_str("# ");
        out.push_str(&name);
        out.push('\n');
        for row in range.rows() {
            let line = row
                .iter()
                .map(|cell| {
                    if matches!(cell, Data::Empty) {
                        String::new()
                    } else {
                        cell.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(" | ");
            if !line.trim().is_empty() {
                out.push_str(&line);
                out.push('\n');
            }
        }
    }
    Ok(out)
}

fn extract_docx(bytes: &[u8]) -> Result<String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes.to_vec()))
        .map_err(|e| NucleusError::invalid(format!("docx: {e}")))?;
    let mut xml = String::new();
    archive
        .by_name("word/document.xml")
        .map_err(|e| NucleusError::invalid(format!("docx sin word/document.xml: {e}")))?
        .read_to_string(&mut xml)
        .map_err(|e| NucleusError::invalid(format!("docx ilegible: {e}")))?;
    Ok(strip_docx_xml(&xml))
}

/// Turn WordprocessingML into plain text: paragraph ends become newlines, tags
/// are dropped, and the basic XML entities are unescaped.
fn strip_docx_xml(xml: &str) -> String {
    let with_breaks = xml.replace("</w:p>", "\n");
    let mut out = String::with_capacity(with_breaks.len());
    let mut in_tag = false;
    for ch in with_breaks.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_and_unsupported() {
        // runs of blank lines collapse to a single paragraph break
        assert_eq!(
            extract_text("a.txt", b"hola  mundo\n\n\n x").unwrap(),
            "hola mundo\n\nx"
        );
        assert_eq!(
            extract_text("a.md", b"# T\n\ntexto").unwrap(),
            "# T\n\ntexto"
        );
        assert!(extract_text("a.doc", b"\xff\xfe").is_err());
        assert!(extract_text("a.zip", b"PK").is_err());
        assert!(is_supported("x.PDF"));
        assert!(!is_supported("x.doc"));
    }

    #[test]
    fn docx_xml_stripping() {
        let xml =
            r#"<w:p><w:r><w:t>Hola</w:t></w:r></w:p><w:p><w:r><w:t>R&amp;D</w:t></w:r></w:p>"#;
        assert_eq!(
            strip_docx_xml(xml).split_whitespace().collect::<Vec<_>>(),
            vec!["Hola", "R&D"]
        );
    }
}
