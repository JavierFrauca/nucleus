//! Pre-download the default embedding model into a cache directory.
//!
//! Run at **image build time** so the published Docker image already contains
//! the ~450 MB model and the first ingest/search doesn't pay the download. The
//! cache directory is taken from `NUCLEUS_MODEL_CACHE` (default
//! `/opt/nucleus/models`); point the running server at the same path.
//!
//! ```bash
//! NUCLEUS_MODEL_CACHE=/opt/nucleus/models \
//!   cargo run --release -p nucleus-core --example prefetch_model
//! ```

use std::path::PathBuf;

use nucleus_core::embed::{Embedder, LocalEmbedder, DEFAULT_MODEL};

fn main() {
    let cache =
        std::env::var("NUCLEUS_MODEL_CACHE").unwrap_or_else(|_| "/opt/nucleus/models".into());
    println!("prefetching `{DEFAULT_MODEL}` into {cache}…");
    let embedder = LocalEmbedder::with_options(Some(PathBuf::from(&cache)), false);
    // A single embed forces fastembed to download and load the model.
    embedder
        .embed_query(DEFAULT_MODEL, "warmup")
        .expect("failed to prefetch the default model");
    println!("done; model cached in {cache}");
}
