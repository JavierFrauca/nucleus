//! The crate-wide error type and `Result` alias.
//!
//! This is the refined descendant of the original `NucleusError` seed:
//! - targets **bincode 2.x** (`EncodeError`/`DecodeError`, the 1.x `bincode::Error`
//!   no longer exists),
//! - uses typed ids instead of raw `u64`,
//! - preserves error sources (`#[source]`/`#[from]`) so the cause chain survives,
//! - drops the redundant `Error` suffix on variants and uses lower-case messages,
//! - is `#[non_exhaustive]` so new variants are not a breaking change.

use thiserror::Error;

use crate::id::{ChunkId, DocumentId, DomainId, JobId, SubdomainId, TagId};

/// Convenience alias used throughout the crate.
pub type Result<T, E = NucleusError> = std::result::Result<T, E>;

/// Every fallible operation in Nucleus returns this error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NucleusError {
    /// An embedding model was requested but is not registered/available.
    #[error("model not found: {0}")]
    ModelNotFound(String),

    /// No domain exists with the given id.
    #[error("domain not found: {0}")]
    DomainNotFound(DomainId),

    /// No document exists with the given id.
    #[error("document not found: {0}")]
    DocumentNotFound(DocumentId),

    /// No subdomain exists with the given id.
    #[error("subdomain not found: {0}")]
    SubdomainNotFound(SubdomainId),

    /// No chunk exists with the given id.
    #[error("chunk not found: {0}")]
    ChunkNotFound(ChunkId),

    /// No tag exists with the given id.
    #[error("tag not found: {0}")]
    TagNotFound(TagId),

    /// No job exists with the given id.
    #[error("job not found: {0}")]
    JobNotFound(JobId),

    /// The embedding backend failed; the underlying cause is preserved.
    #[error("embedding backend failed")]
    Embedding(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// A vector did not match the dimension expected by the domain/index.
    #[error("dimension mismatch: expected {expected}, got {got}")]
    DimensionMismatch { expected: usize, got: usize },

    /// The persistent store (redb) returned an error. Boxed because `redb::Error`
    /// is large and this type is the `Err` of nearly every function (keeps
    /// `Result<T, NucleusError>` cheap to move).
    #[error("storage error")]
    Storage(#[source] Box<redb::Error>),

    /// Failed to encode a value for storage (bincode 2.x).
    #[error("failed to encode value")]
    Encode(#[source] Box<bincode::error::EncodeError>),

    /// Failed to decode a value from storage (bincode 2.x).
    #[error("failed to decode value")]
    Decode(#[source] Box<bincode::error::DecodeError>),

    /// An I/O operation failed.
    #[error("i/o error")]
    Io(#[from] std::io::Error),

    /// The request carried no valid credentials.
    #[error("unauthorized")]
    Unauthorized,

    /// The credentials are valid but lack the required scope.
    #[error("forbidden")]
    Forbidden,

    /// The request was malformed or violated an invariant.
    #[error("invalid request: {0}")]
    InvalidRequest(String),
}

impl NucleusError {
    /// Wrap any backend error as an [`NucleusError::Embedding`], preserving the cause.
    pub fn embedding<E>(source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Embedding(Box::new(source))
    }

    /// Wrap a plain message as an [`NucleusError::Embedding`]. Handy for backends
    /// (like fastembed) whose error type does not implement `std::error::Error`.
    pub fn embedding_msg(msg: impl Into<String>) -> Self {
        Self::Embedding(msg.into().into())
    }

    /// Shorthand for an [`NucleusError::InvalidRequest`].
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::InvalidRequest(msg.into())
    }
}

// redb surfaces a family of specific error types from its different operations.
// They all funnel into `redb::Error`, so route each into `Storage` (boxed) to
// keep call sites using a plain `?`.
macro_rules! from_redb {
    ($($ty:ty),* $(,)?) => {
        $(
            impl From<$ty> for NucleusError {
                fn from(e: $ty) -> Self {
                    NucleusError::Storage(Box::new(redb::Error::from(e)))
                }
            }
        )*
    };
}

from_redb!(
    redb::Error,
    redb::DatabaseError,
    redb::TransactionError,
    redb::TableError,
    redb::StorageError,
    redb::CommitError,
);

impl From<bincode::error::EncodeError> for NucleusError {
    fn from(e: bincode::error::EncodeError) -> Self {
        NucleusError::Encode(Box::new(e))
    }
}

impl From<bincode::error::DecodeError> for NucleusError {
    fn from(e: bincode::error::DecodeError) -> Self {
        NucleusError::Decode(Box::new(e))
    }
}
