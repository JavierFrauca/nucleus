//! Measures the *real* in-memory footprint of the vector indexes (Flat `f32` vs
//! Sq `int8`) with a counting global allocator — exact heap bytes, not derived
//! from the struct layout.
//!
//! Run: `cargo run --release --example index_footprint`
//!
//! Each vector is generated on the fly and dropped immediately, so the measured
//! delta is purely what the index retains (codes/data + ids + the position map).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use nucleus_core::id::ChunkId;
use nucleus_core::index::{build_index, IndexKind};

// --- counting allocator ----------------------------------------------------

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = System.alloc(l);
        if !p.is_null() {
            ALLOCATED.fetch_add(l.size(), Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l);
        ALLOCATED.fetch_sub(l.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = System.realloc(ptr, layout, new_size);
        if !p.is_null() {
            ALLOCATED.fetch_add(new_size, Ordering::Relaxed);
            ALLOCATED.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        p
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

// --- bench -----------------------------------------------------------------

/// Deterministic pseudo-random vector in roughly [-0.5, 0.5] (the index
/// L2-normalises internally, so the scale is irrelevant).
fn random_vec(dim: usize, seed: &mut u64) -> Vec<f32> {
    (0..dim)
        .map(|_| {
            *seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((*seed >> 33) as f32 / (1u64 << 31) as f32) - 0.5
        })
        .collect()
}

/// Net heap bytes retained by an index holding `n` vectors of dimension `dim`.
fn measure(kind: IndexKind, dim: usize, n: usize) -> usize {
    let before = ALLOCATED.load(Ordering::Relaxed);
    let mut index = build_index(kind, dim);
    let mut seed = 0x9E3779B97F4A7C15u64;
    for i in 0..n {
        let v = random_vec(dim, &mut seed);
        index.upsert(ChunkId::new(i as u64), &v).unwrap();
    }
    let held = ALLOCATED.load(Ordering::Relaxed) - before;
    drop(index);
    held
}

fn main() {
    let dim = 384;
    println!("Index footprint — measured heap, dim = {dim}\n");
    println!(
        "{:>10}  {:<6}  {:>12}  {:>10}  {:>8}",
        "N", "kind", "total bytes", "bytes/vec", "MB"
    );

    for &n in &[50_000usize, 200_000, 1_000_000] {
        let mut per_vec = [0f64; 2];
        for (j, (kind, name)) in [(IndexKind::Flat, "flat"), (IndexKind::Sq, "sq")]
            .into_iter()
            .enumerate()
        {
            let bytes = measure(kind, dim, n);
            per_vec[j] = bytes as f64 / n as f64;
            println!(
                "{:>10}  {:<6}  {:>12}  {:>10.1}  {:>8.1}",
                n,
                name,
                bytes,
                per_vec[j],
                bytes as f64 / (1024.0 * 1024.0)
            );
        }
        println!(
            "{:>10}  {:<6}  ratio flat/sq = {:.2}x\n",
            "", "", per_vec[0] / per_vec[1]
        );
    }
}
