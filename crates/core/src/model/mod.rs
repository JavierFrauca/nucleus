//! Core domain entities, persisted via serde + bincode and exposed over the API.

mod chunk;
mod document;
mod domain;
mod subdomain;
mod tag;

pub use chunk::Chunk;
pub use document::Document;
pub use domain::Domain;
pub use subdomain::Subdomain;
pub use tag::Tag;
