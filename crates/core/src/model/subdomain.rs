use serde::{Deserialize, Serialize};

use crate::id::{DomainId, SubdomainId};

/// A subdomain is a concrete topic *within* a domain. In the turnkey contract it
/// is supplied by the caller at ingest time (by name); auto-induction from the
/// corpus is a later, optional layer. Documents/chunks may reference one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subdomain {
    pub id: SubdomainId,
    pub domain_id: DomainId,
    pub name: String,
    pub description: String,
    /// Creation time, Unix milliseconds.
    pub created_at: i64,
}
