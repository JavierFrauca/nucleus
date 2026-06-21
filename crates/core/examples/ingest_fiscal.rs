//! Rebuild the fiscal demo corpus into a fresh database with the *current*
//! schema (the old `fiscal.redb` predates the `subdomain_id` field, so its
//! bincode records no longer decode). Extracts, chunks, embeds and indexes the
//! raw PDFs in-process — the same path the server's file-upload endpoint uses.
//!
//! Run (PowerShell):
//!   $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"; $env:CARGO_INCREMENTAL="0"
//!   cargo run --release -p nucleus-core --example ingest_fiscal

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use nucleus_core::embed::{Embedder, LocalEmbedder};
use nucleus_core::engine::IngestBody;
use nucleus_core::extract::extract_text;
use nucleus_core::index::IndexKind;
use nucleus_core::storage::Storage;
use nucleus_core::Engine;

const DB: &str = r"C:\tmp\nucleus_demo\fiscal_v2.redb";
const MODEL_CACHE: &str = r"C:\tmp\nucleus_models";
const FISCAL_DIR: &str = r"C:\tmp\ingesta_documentos\fiscal";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Start from a clean database file.
    let _ = std::fs::remove_file(DB);

    let storage = Storage::open(PathBuf::from(DB))?;
    let embedder: Arc<dyn Embedder> = Arc::new(LocalEmbedder::with_options(
        Some(PathBuf::from(MODEL_CACHE)),
        false,
    ));
    let engine = Engine::open(storage, embedder, IndexKind::Flat, None)?;

    let domain = engine.create_domain("fiscal", None)?;
    println!(
        "dominio 'fiscal' id={} ({}, dim={})",
        domain.id.get(),
        domain.model,
        domain.dim
    );

    let mut pdfs: Vec<PathBuf> = std::fs::read_dir(FISCAL_DIR)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("pdf"))
        .collect();
    pdfs.sort();
    println!("ingestando {} PDFs…\n", pdfs.len());

    let t0 = Instant::now();
    let mut ok = 0usize;
    let mut chunks_total = 0usize;
    for (i, path) in pdfs.iter().enumerate() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let bytes = std::fs::read(path)?;
        let text = match extract_text(&name, &bytes) {
            Ok(t) => t,
            Err(e) => {
                println!("  [{:>2}] {name}: extract FALLÓ: {e}", i + 1);
                continue;
            }
        };
        let mut meta = BTreeMap::new();
        meta.insert("filename".to_string(), name.clone());
        let t = Instant::now();
        match engine.ingest_document(
            domain.id,
            &name,
            Some(name.clone()),
            meta,
            vec![],
            IngestBody::Text(text),
        ) {
            Ok(out) => {
                chunks_total += out.chunk_count;
                ok += 1;
                println!(
                    "  [{:>2}/{}] {name}: {} chunks ({:.1}s)",
                    i + 1,
                    pdfs.len(),
                    out.chunk_count,
                    t.elapsed().as_secs_f64()
                );
            }
            Err(e) => println!("  [{:>2}] {name}: ingest FALLÓ: {e}", i + 1),
        }
    }
    println!(
        "\nlisto: {ok}/{} docs, {chunks_total} chunks en {:.0}s -> {DB}",
        pdfs.len(),
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}
