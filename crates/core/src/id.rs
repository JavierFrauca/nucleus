//! Typed identifiers.
//!
//! Each entity gets its own newtype over `u64` so the compiler stops us from, say,
//! passing a [`DocumentId`] where a [`DomainId`] is expected. They round-trip to
//! `u64` for use as redb keys and serialise transparently for the HTTP API.

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl $name {
            /// Construct from a raw `u64`.
            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            /// The underlying `u64`.
            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<u64> for $name {
            fn from(value: u64) -> Self {
                Self(value)
            }
        }

        impl From<$name> for u64 {
            fn from(value: $name) -> u64 {
                value.0
            }
        }
    };
}

define_id!(
    /// Identifies a domain (namespace/collection).
    DomainId
);
define_id!(
    /// Identifies a document within a domain.
    DocumentId
);
define_id!(
    /// Identifies a chunk within a document.
    ChunkId
);
define_id!(
    /// Identifies a subdomain (a topic within a domain).
    SubdomainId
);
define_id!(
    /// Identifies a tag (label) within a domain's taxonomy.
    TagId
);
define_id!(
    /// Identifies a background job.
    JobId
);
define_id!(
    /// Identifies an API token.
    TokenId
);
