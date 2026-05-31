//! No-dependency micro-benchmarks for `reel-spec` hot paths.
//!
//! Run: `cargo run -p reel-spec --example microbench --release`
//!
//! Dependency-free (no `criterion`) so it builds offline — a coarse signal,
//! not statistically rigorous. Turns design facts into runnable evidence:
//!
//! - F1 (fixed): content-addressing dedup holds — identical content hashes
//!   identically regardless of when it was built (the regression check for
//!   moving provenance out of identity).
//! - F3 (fixed): `fork` (`View::child_of`) is `O(1)` in namespace size —
//!   `Namespace::clone` is a refcount bump on an `imbl::OrdMap` root
//!   (forest-GC borrow per `reel-kernel-architecture-2026-05-27.md` §1
//!   and ADR-0034). Before: 8.3 ms at 100k entries (O(n)). After:
//!   flat across N — fork latency tracks `View::child_of` envelope
//!   (UUIDv7, capability vector, delta init), not the namespace size.
//!
//! For rigorous benchmarks wire `criterion` once a network fetch is safe.
#![allow(
    clippy::print_stdout,
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    unused_crate_dependencies,
    reason = "illustrative micro-benchmark example: printing results is its job; inherits dev-deps it does not use"
)]

use std::hint::black_box;
use std::time::{Duration, Instant};

use bytes::Bytes;
use reel_spec::{Block, Hash, View};

/// F3 — fork scaling. With `imbl::OrdMap` (ADR-0034) `ns/op` is **constant**
/// across `N` because `Namespace::clone` is a refcount bump. The verdict
/// line prints `O(1) HOLDS` iff the max/min ratio across the size sweep
/// is ≤ 1.5×; anything higher implies a regression.
fn bench_fork_scaling() {
    println!("\n== F3 (fixed): fork (View::child_of) scaling — O(1)? ==");
    println!("{:>11}  {:>12}  {:>16}", "ns_entries", "ns/op", "ns/op ÷ entry");
    let mut samples: Vec<f64> = Vec::new();
    for &n in &[1usize, 10, 100, 1_000, 10_000, 100_000] {
        let mut parent = View::root(vec![]);
        for i in 0..n {
            parent.namespace.insert(format!("k{i}").into(), Hash::from_bytes([0u8; 32]));
        }
        for _ in 0..3 {
            black_box(View::child_of(&parent, vec![]).unwrap());
        }
        let iters = (10_000_000usize / n.max(1)).clamp(50, 200_000);
        let t = Instant::now();
        for _ in 0..iters {
            black_box(View::child_of(black_box(&parent), vec![]).unwrap());
        }
        let per = t.elapsed().as_nanos() as f64 / iters as f64;
        samples.push(per);
        println!("{n:>11}  {per:>12.1}  {:>16.4}", per / n as f64);
    }
    // Flatness verdict — same envelope across 5 orders of magnitude is the
    // O(1) signal. We bound the ratio to 1.5× to absorb measurement noise.
    let max = samples.iter().copied().fold(f64::MIN, f64::max);
    let min = samples.iter().copied().fold(f64::MAX, f64::min);
    let ratio = max / min;
    let verdict = if ratio <= 1.5 { "O(1) HOLDS" } else { "REGRESSION: not O(1)" };
    println!(
        "  → max/min ns ratio across N ∈ [1, 100 000] = {ratio:.2}× → {verdict}\n  \
         (envelope cost = UUIDv7 + Vec<Capability>::clone + Delta init; \
         Namespace::clone itself is dominated by these.)",
    );
}

/// F1 (fixed) — identical content built at two instants must hash identically,
/// because identity is `BLAKE3(data)` with nothing time-dependent. This is the
/// regression guard for content-addressing dedup / determinism.
fn bench_dedup_check() {
    println!("\n== F1 (fixed): content-addressing dedup holds ==");
    let data = b"identical logical content".to_vec();
    let h1 = Block::new(data.clone()).unwrap().hash();
    std::thread::sleep(Duration::from_millis(2));
    let h2 = Block::new(data).unwrap().hash();
    let ok = h1 == h2;
    println!("  block A: {}", hex(&h1));
    println!("  block B: {}", hex(&h2));
    println!("  identical content (2 ms apart) → identical hash? {ok}  (forest-GC needs TRUE)");
    println!(
        "  → {}",
        if ok {
            "OK: identity = BLAKE3(data); dedup foundation holds."
        } else {
            "REGRESSION: identity is time-dependent again — provenance leaked into the hash."
        }
    );
}

/// Block construction throughput over a zero-copy `Bytes` payload. `Bytes`
/// clone is an O(1) refcount bump, so this measures ~pure BLAKE3 (no memcpy),
/// unlike the prior `Vec<u8>` path that paid a full copy per op (F6).
fn bench_block_throughput() {
    println!("\n== Block::new throughput (BLAKE3 over zero-copy Bytes) ==");
    for &sz in &[1_024usize, 64 * 1024, 1024 * 1024] {
        let data = Bytes::from(vec![0x5au8; sz]);
        let iters = (256 * 1024 * 1024 / sz).clamp(10, 100_000);
        let t = Instant::now();
        for _ in 0..iters {
            black_box(Block::new(black_box(data.clone())).unwrap());
        }
        let mbps = (sz as f64 * iters as f64) / t.elapsed().as_secs_f64() / (1024.0 * 1024.0);
        println!("  payload {sz:>9} B: {mbps:>8.0} MB/s  (Bytes clone O(1) ⇒ ~pure hash)");
    }
}

fn hex(h: &Hash) -> String {
    h.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

fn main() {
    println!("reel-spec micro-benchmarks (no-dep, release; coarse signal).");
    bench_fork_scaling();
    bench_dedup_check();
    bench_block_throughput();
}
