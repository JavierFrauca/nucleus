//! Token-based authentication and authorization.
//!
//! Tokens are opaque random strings (`nuc_` + 256 bits of hex). Only their
//! SHA-256 hash is persisted, and lookup is by that hash — so the plaintext is
//! shown exactly once, at creation. Each token carries [`Scope`]s granting a
//! permission level over one domain or all domains.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::id::{DomainId, TokenId};

/// User-facing token prefix, purely for recognisability.
pub const TOKEN_PREFIX: &str = "nuc_";

/// Permission level, ordered Read < Write < Admin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Perm {
    Read,
    Write,
    Admin,
}

impl Perm {
    fn rank(self) -> u8 {
        match self {
            Perm::Read => 0,
            Perm::Write => 1,
            Perm::Admin => 2,
        }
    }
}

/// Which domain(s) a scope applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DomainScope {
    All,
    One(DomainId),
}

impl DomainScope {
    fn matches(self, domain: DomainId) -> bool {
        match self {
            DomainScope::All => true,
            DomainScope::One(d) => d == domain,
        }
    }
}

/// A single grant: `perm` over `domain`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub domain: DomainScope,
    pub perm: Perm,
}

impl Scope {
    /// A global administrator grant.
    pub fn admin_all() -> Self {
        Scope {
            domain: DomainScope::All,
            perm: Perm::Admin,
        }
    }
}

/// A persisted API token. The plaintext is never stored — only `hash`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiToken {
    pub id: TokenId,
    pub name: String,
    pub hash: [u8; 32],
    pub scopes: Vec<Scope>,
    pub created_at: i64,
    pub expires_at: Option<i64>,
}

/// The authenticated identity attached to a request after the bearer token is
/// validated.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub token_id: TokenId,
    pub scopes: Vec<Scope>,
}

impl AuthContext {
    /// Whether the caller may perform `need` on `domain`.
    pub fn allows(&self, domain: DomainId, need: Perm) -> bool {
        self.scopes
            .iter()
            .any(|s| s.domain.matches(domain) && s.perm.rank() >= need.rank())
    }

    /// Whether the caller is a global administrator (manage domains/tokens).
    pub fn is_admin(&self) -> bool {
        self.scopes
            .iter()
            .any(|s| matches!(s.domain, DomainScope::All) && s.perm == Perm::Admin)
    }
}

/// SHA-256 of a token string.
pub fn hash_token(token: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hasher.finalize().into()
}

/// Generate a fresh token, returning `(plaintext, hash)`.
pub fn generate_token() -> (String, [u8; 32]) {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let token = format!("{TOKEN_PREFIX}{}", to_hex(&bytes));
    let hash = hash_token(&token);
    (token, hash)
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perm_ordering() {
        let ctx = AuthContext {
            token_id: TokenId::new(1),
            scopes: vec![Scope {
                domain: DomainScope::One(DomainId::new(7)),
                perm: Perm::Write,
            }],
        };
        assert!(ctx.allows(DomainId::new(7), Perm::Read));
        assert!(ctx.allows(DomainId::new(7), Perm::Write));
        assert!(!ctx.allows(DomainId::new(7), Perm::Admin));
        assert!(!ctx.allows(DomainId::new(8), Perm::Read)); // wrong domain
        assert!(!ctx.is_admin());
    }

    #[test]
    fn admin_all_grants_everything() {
        let ctx = AuthContext {
            token_id: TokenId::new(1),
            scopes: vec![Scope::admin_all()],
        };
        assert!(ctx.is_admin());
        assert!(ctx.allows(DomainId::new(123), Perm::Write));
    }

    #[test]
    fn tokens_are_unique_and_hash_is_stable() {
        let (a, ha) = generate_token();
        let (b, _hb) = generate_token();
        assert!(a.starts_with("nuc_"));
        assert_ne!(a, b);
        assert_eq!(hash_token(&a), ha);
    }
}
