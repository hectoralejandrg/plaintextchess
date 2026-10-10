//! Allocation-counting harness for the chess core (change
//! `optimize-chess-core`, task 1.2, design D1/D5).
//!
//! Counts heap allocations and wall time per `best_move` at each
//! difficulty from the same representative midgame positions as
//! `benches/chess_bench.rs`, so the measurement gate (task 1.4) and
//! the post-optimization comparison (task 2.3) have exact numbers.
//!
//! Run (release, like a phone's optimized build):
//!
//!     cargo run --release --example alloc_harness
//!
//! The counting allocator is process-wide, so the harness runs one
//! measurement at a time and reports the delta around the call.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use shakmaty::fen::Fen;
use shakmaty::{Chess, Position};

use chess_core::domain::search::best_move;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Delegating allocator that counts every heap allocation (and its
/// size) before handing it to the system allocator.
struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

/// The same two representative midgame positions as the criterion
/// bench (see `benches/chess_bench.rs`).
fn midgame_positions() -> Vec<(&'static str, Chess)> {
    [
        (
            "italian",
            "r1bqkbnr/pppp1ppp/2n5/4p3/4P3/5N2/PPPP1PPP/RNBQKB1R w KQkq - 2 3",
        ),
        (
            "ruy_lopez",
            "r1bqk2r/pppp1ppp/2n2n2/1B2p3/4P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 2 5",
        ),
    ]
    .into_iter()
    .map(|(name, fen)| {
        let fen: Fen = fen.parse().expect("valid FEN");
        let pos = Chess::from_setup(&fen).expect("legal position");
        (name, pos)
    })
    .collect()
}

fn reset_counters() {
    ALLOCATIONS.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
}

fn main() {
    let positions = midgame_positions();
    println!("allocations per best_move (release build)");
    println!("{:<10} {:<11} {:<12} {:<14} {}", "position", "difficulty", "allocations", "bytes", "think time");
    for (name, pos) in &positions {
        for difficulty in 1..=3u8 {
            // Warm up (page in, JIT-none but branch predictors etc.),
            // then measure three single calls and report each.
            let _ = best_move(pos, difficulty);
            for run in 1..=3 {
                reset_counters();
                let start = Instant::now();
                let uci = best_move(pos, difficulty);
                let elapsed = start.elapsed();
                let allocations = ALLOCATIONS.load(Ordering::Relaxed);
                let bytes = ALLOCATED_BYTES.load(Ordering::Relaxed);
                assert!(uci.is_some(), "no move at difficulty {difficulty}");
                println!(
                    "{:<10} {:<11} {:<12} {:<14} {:?} (run {run})",
                    name, difficulty, allocations, bytes, elapsed
                );
            }
        }
    }
}
