//! Criterion benchmarks for the chess core (change `optimize-chess-core`,
//! design D5).
//!
//! This is the measuring instrument for the measurement gate (design D1):
//! it establishes the think-time and legal-move-generation baselines the
//! later tasks compare against. Baselines are recorded in `benches/README.md`.
//!
//! Groups:
//! - `best_move`: CPU think time at difficulties 1-3 from two
//!   representative midgame positions.
//! - `legal_moves`: full legal-move generation (what every
//!   `get_valid_moves` call triggers internally) and the worst-case query
//!   loop of one query per square.

use shakmaty::fen::Fen;
use shakmaty::{Chess, Position};

use chess_core::domain::BoardManager;
use chess_core::domain::search::best_move;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

/// Two representative midgame positions (design D5): the Italian game
/// after 2...Nc6 and the Ruy Lopez after 4...Nf6. Both are quiet,
/// well-developed positions with a full move set - the common case a
/// phone CPU sees mid-game.
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

/// A `BoardManager` on the Italian position, reached by playing the
/// opening plies (the manager has no FEN loader; `play_move` is its
/// only way to leave the start position).
fn italian_manager() -> BoardManager {
    let mut manager = BoardManager::new();
    for uci in ["e2e4", "e7e5", "g1f3", "b8c6"] {
        manager.play_move(uci).expect("opening ply");
    }
    manager
}

fn bench_best_move(c: &mut Criterion) {
    let mut group = c.benchmark_group("best_move");
    for (name, pos) in midgame_positions() {
        for difficulty in 1..=3u8 {
            group.bench_with_input(
                BenchmarkId::from_parameter(format!("{name}/difficulty_{difficulty}")),
                &pos,
                |b, pos| {
                    b.iter(|| best_move(std::hint::black_box(pos), std::hint::black_box(difficulty)))
                },
            );
        }
    }
    group.finish();
}

fn bench_legal_moves(c: &mut Criterion) {
    let mut group = c.benchmark_group("legal_moves");
    for (name, pos) in midgame_positions() {
        // The cost every single `get_valid_moves` call pays: a full
        // legal-move generation (including check detection). `legals()`
        // returns a stack-allocated `ArrayVec`, so this measures pure
        // generation time, without the `Vec<String>` collection the
        // `get_valid_moves` wrapper adds on top.
        group.bench_with_input(
            BenchmarkId::new("legals_generation", name),
            &pos,
            |b, pos| {
                b.iter(|| std::hint::black_box(pos.legals().len()));
            },
        );
    }

    // The worst-case query loop: one `get_valid_moves` call per square.
    // The apps call it once per piece selection (see benches/README.md,
    // finding 1.3); this loop is the upper bound of a selection's cost
    // if that ever changed to a per-square or per-frame pattern.
    let manager = italian_manager();
    let squares: Vec<String> = ('a'..='h')
        .flat_map(|file| ('1'..='8').map(move |rank| format!("{file}{rank}")))
        .collect();
    group.bench_function("query_loop_all_squares", |b| {
        b.iter(|| {
            for square in &squares {
                std::hint::black_box(manager.get_valid_moves(square));
            }
        })
    });
    group.finish();
}

criterion_group!(benches, bench_best_move, bench_legal_moves);
criterion_main!(benches);
