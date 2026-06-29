//! `nucleus-core` — the engine behind Nucleus, a database specialised for RAG
//! workloads with first-class **domains** (namespaces) and **tagging**.
//!
//! This crate is transport-agnostic: it owns storage, the vector index, the
//! in-process embedding provider, the job queue and auth. The HTTP surface lives
//! in the separate `nucleus-server` crate.

pub mod auth;
pub mod backup;
pub mod batch;
pub mod chunking;
pub mod crypto;
pub mod embed;
pub mod engine;
pub mod error;
pub mod extract;
pub mod id;
pub mod index;
pub mod jobs;
pub mod model;
pub mod query;
pub mod rerank;
pub mod storage;
pub mod util;

pub use engine::Engine;
pub use error::{NucleusError, Result};
