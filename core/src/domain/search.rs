//! Depth-limited minimax search for the CPU move (design D2).
//!
//! All chess logic stays in the core: the apps only ask for a move through
//! `GameSession::get_cpu_move` and play it through their regular move path.
//! The search is a pure query — it never mutates the session.

use shakmaty::uci::Uci;
use shakmaty::{Chess, Color, Move, Position, Role, Setup, Square};

/// Mate score: far above any material swing so mates outrank material.
const MATE: i32 = 100_000;

/// Difficulty → search depth in plies (design D2): easy 2, medium 3, hard 4.
fn depth_for(difficulty: u8) -> Option<u8> {
    match difficulty {
        1 => Some(2),
        2 => Some(3),
        3 => Some(4),
        _ => None,
    }
}

/// Pick the CPU's move: a legal UCI string for the side to move at the given
/// difficulty (1 easy, 2 medium, 3 hard). `None` when the difficulty is
/// unknown or the side to move has no legal move (game over).
///
/// Medium and hard are deterministic (same position + difficulty ⇒ same
/// move on every platform); easy perturbs leaf evaluations with xorshift
/// noise so the play varies between games.
pub fn best_move(position: &Chess, difficulty: u8) -> Option<String> {
    let depth = depth_for(difficulty)?;
    let moves = ordered_moves(position);
    if moves.is_empty() {
        return None;
    }
    let noisy = difficulty == 1;
    let mut rng = XorShift64::new();
    let mut best: Option<(i32, Move)> = None;
    for m in &moves {
        let next = match position.clone().play(m) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let score = -search(&next, depth - 1, -MATE * 2, MATE * 2, &mut rng, noisy);
        match best {
            Some((s, _)) if score <= s => {}
            _ => best = Some((score, m.clone())),
        }
    }
    best.map(|(_, m)| Uci::from_move(position, &m).to_string())
}

/// Negamax with alpha-beta pruning. Returns the score of `pos` from the
/// perspective of the side to move.
fn search(
    pos: &Chess,
    depth: u8,
    mut alpha: i32,
    beta: i32,
    rng: &mut XorShift64,
    noisy: bool,
) -> i32 {
    if depth == 0 {
        return evaluate(pos, rng, noisy);
    }
    let mut best = i32::MIN;
    let mut any = false;
    for m in ordered_moves(pos) {
        let next = match pos.clone().play(&m) {
            Ok(p) => p,
            Err(_) => continue,
        };
        any = true;
        let score = -search(&next, depth - 1, -beta, -alpha, rng, noisy);
        if score > best {
            best = score;
        }
        if best > alpha {
            alpha = best;
        }
        if alpha >= beta {
            break;
        }
    }
    if !any {
        // No legal move for the side to move: mated (bad, sooner is worse)
        // or stalemate (draw).
        return if pos.is_check() {
            -(MATE + depth as i32)
        } else {
            0
        };
    }
    best
}

/// Legal moves ordered for the search: captures first, most-valuable victim
/// first (MVV-lite). Keeps the hard depth inside its time budget without a
/// transposition table.
fn ordered_moves(position: &Chess) -> Vec<Move> {
    let mut moves: Vec<Move> = position.legals().into_iter().collect();
    moves.sort_by_key(|m| {
        // Captures first, most-valuable victim first (MVV-lite).
        let victim = m.capture().map(piece_value).unwrap_or(0);
        std::cmp::Reverse(victim)
    });
    moves
}

/// Material values in centipawns (the king is excluded from material).
fn piece_value(role: Role) -> i32 {
    match role {
        Role::Pawn => 100,
        Role::Knight => 320,
        Role::Bishop => 330,
        Role::Rook => 500,
        Role::Queen => 900,
        Role::King => 0,
    }
}

/// Static evaluation from the perspective of the side to move: material plus
/// small piece-square bonuses. `noisy` (easy difficulty only) adds xorshift
/// jitter to the result so easy games vary.
///
/// Two allocation-free passes over the 64 squares (design D3): the first
/// sums non-king material (to pick the endgame or middlegame king tables),
/// the second accumulates material plus piece-square bonuses. No `Vec` is
/// built, so leaf nodes allocate nothing; the scores are identical to the
/// previous implementation (guarded by the equivalence test below).
fn evaluate(pos: &Chess, rng: &mut XorShift64, noisy: bool) -> i32 {
    let board = pos.board();
    // First pass: total non-king material, to decide endgame PSTs.
    let mut material: i32 = 0;
    for sq in 0..64 {
        if let Some(piece) = board.piece_at(Square::new(sq)) {
            material += piece_value(piece.role);
        }
    }
    let endgame = material < 1300;
    // Second pass: material plus piece-square bonuses.
    let mut score: i32 = 0;
    for sq in 0..64 {
        let square = Square::new(sq);
        if let Some(piece) = board.piece_at(square) {
            let value = piece_value(piece.role)
                + pst_bonus(piece.role, square, piece.color, endgame);
            score += if piece.color == Color::White {
                value
            } else {
                -value
            };
        }
    }
    let score = if pos.turn() == Color::White {
        score
    } else {
        -score
    };
    if noisy {
        score + rng.noise()
    } else {
        score
    }
}

/// Piece-square bonus for `sq`. Tables are indexed rank-first with rank 1 as
/// row 0 from White's perspective; Black mirrors the rank axis.
fn pst_bonus(role: Role, sq: Square, color: Color, endgame: bool) -> i32 {
    let r = sq.rank() as usize;
    let f = sq.file() as usize;
    let r = if color == Color::Black { 7 - r } else { r };
    let idx = r * 8 + f;
    match role {
        Role::Pawn => PAWN_PST[idx],
        Role::Knight => KNIGHT_PST[idx],
        Role::Bishop => BISHOP_PST[idx],
        Role::Rook => ROOK_PST[idx],
        Role::King => {
            if endgame {
                KING_ENDGAME_PST[idx]
            } else {
                KING_MIDDLEGAME_PST[idx]
            }
        }
        Role::Queen => 0,
    }
}

// Piece-square tables, rows rank 1 → rank 8 (index = rank*8 + file),
// White's perspective. Small on purpose: at these search depths the bonus
// only needs to guide play, not replace it.

const PAWN_PST: [i32; 64] = [
    // rank 1 (unreachable):
    0, 0, 0, 0, 0, 0, 0, 0, // rank 2:
    -5, 0, 0, 5, 5, 0, 0, -5, // rank 3:
    0, 0, 5, 10, 10, 5, 0, 0, // rank 4:
    0, 5, 10, 15, 15, 10, 5, 0, // rank 5:
    5, 10, 15, 20, 20, 15, 10, 5, // rank 6:
    30, 30, 35, 40, 40, 35, 30, 30, // rank 7:
    50, 60, 70, 80, 80, 70, 60, 50, // rank 8 (unreachable):
    0, 0, 0, 0, 0, 0, 0, 0,
];

const KNIGHT_PST: [i32; 64] = [
    // rank 1:
    -10, -5, 0, 0, 0, 0, -5, -10, // rank 2:
    -5, 0, 5, 10, 10, 5, 0, -5, // rank 3:
    0, 5, 10, 15, 15, 10, 5, 0, // rank 4:
    0, 5, 10, 15, 15, 10, 5, 0, // rank 5:
    0, 0, 10, 15, 15, 10, 0, 0, // rank 6:
    -5, 0, 5, 10, 10, 5, 0, -5, // rank 7:
    -10, -5, 0, 5, 5, 0, -5, -10, // rank 8:
    -15, -10, -5, 0, 0, -5, -10, -15,
];

const BISHOP_PST: [i32; 64] = [
    // rank 1:
    0, 0, 0, 0, 0, 0, 0, 0, // rank 2:
    5, 10, 10, 10, 10, 10, 10, 5, // rank 3:
    5, 10, 15, 15, 15, 15, 10, 5, // rank 4:
    0, 10, 15, 20, 20, 15, 10, 0, // rank 5:
    0, 10, 15, 20, 20, 15, 10, 0, // rank 6:
    5, 10, 10, 15, 15, 10, 10, 5, // rank 7:
    5, 10, 10, 10, 10, 10, 10, 5, // rank 8:
    0, 0, 0, 0, 0, 0, 0, 0,
];

const ROOK_PST: [i32; 64] = [
    // rank 1:
    0, 0, 5, 10, 10, 5, 0, 0, // rank 2:
    5, 5, 5, 5, 5, 5, 5, 5, // ranks 3–6:
    5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5,
    // rank 7:
    15, 15, 15, 15, 15, 15, 15, 15, // rank 8:
    10, 10, 10, 10, 10, 10, 10, 10,
];

const KING_MIDDLEGAME_PST: [i32; 64] = [
    // rank 1:
    -20, -10, 0, 0, 0, 0, -10, -20, // rank 2:
    -10, -10, 5, 10, 10, 5, -10, -10, // rank 3:
    -10, 0, 0, 10, 10, 0, 0, -10, // rank 4:
    -10, -5, 0, 10, 10, 0, -5, -10, // rank 5:
    -10, -5, 0, 0, 0, 0, -5, -10, // rank 6:
    -10, 0, 0, 0, 0, 0, 0, -10, // rank 7:
    -20, -10, -10, -20, -20, -10, -10, -20, // rank 8:
    -40, -30, -30, -40, -40, -30, -30, -40,
];

const KING_ENDGAME_PST: [i32; 64] = [
    // rank 1:
    -20, -10, -5, 0, 0, -5, -10, -20, // rank 2:
    -15, -5, 0, 5, 5, 0, -5, -15, // rank 3:
    -10, 0, 5, 10, 10, 5, 0, -10, // rank 4:
    -10, 0, 5, 15, 15, 5, 0, -10, // rank 5:
    -10, 0, 5, 15, 15, 5, 0, -10, // rank 6:
    -10, 0, 0, 10, 10, 0, 0, -10, // rank 7:
    -15, -5, 0, 0, 0, 0, -5, -15, // rank 8:
    -20, -10, -5, -5, -5, -5, -10, -20,
];

/// Tiny xorshift64 PRNG (design D2: no `rand` dependency).
struct XorShift64(u64);

impl XorShift64 {
    /// Seed from the system time (a nonzero fallback keeps the stream alive
    /// if the clock is unavailable).
    fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
            .wrapping_mul(0x2545_F491_4F6C_DD1D);
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform-ish jitter in [-100, 100] centipawns.
    fn noise(&mut self) -> i32 {
        ((self.next() % 201) as i32) - 100
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `Chess` position from a FEN string (test helper).
    fn fen_position(fen: &str) -> Chess {
        let parsed: shakmaty::fen::Fen = fen.parse().unwrap_or_else(|e| panic!("bad fen: {e:?}"));
        Chess::from_setup(&parsed).unwrap_or_else(|e| panic!("setup from fen: {e:?}"))
    }

    /// The pre-rewrite `evaluate` (the version that built a `Vec` of
    /// the pieces), kept as the reference so the allocation-free
    /// rewrite cannot drift the scores (design D3).
    fn evaluate_reference(pos: &Chess) -> i32 {
        let board = pos.board();
        let mut material: i32 = 0;
        let mut pieces: Vec<(Square, Role, Color)> = Vec::new();
        for sq in 0..64 {
            let square = Square::new(sq);
            if let Some(piece) = board.piece_at(square) {
                material += piece_value(piece.role);
                pieces.push((square, piece.role, piece.color));
            }
        }
        let endgame = material < 1300;
        let mut score: i32 = 0;
        for (sq, role, color) in pieces {
            let value = piece_value(role) + pst_bonus(role, sq, color, endgame);
            score += if color == Color::White { value } else { -value };
        }
        if pos.turn() == Color::White {
            score
        } else {
            -score
        }
    }

    /// The allocation-free evaluation must return exactly the scores of
    /// the previous implementation on fixed positions (design D3), so
    /// engine strength cannot drift.
    #[test]
    fn evaluation_scores_match_the_previous_implementation() {
        for fen in [
            // Start position.
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
            // Middlegames (the benchmark positions).
            "r1bqkbnr/pppp1ppp/2n5/4p3/4P3/5N2/PPPP1PPP/RNBQKB1R w KQkq - 2 3",
            "r1bqk2r/pppp1ppp/2n2n2/1B2p3/4P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 2 5",
            // Endgame: king and rook vs king (500 material < 1300).
            "8/8/8/3k4/8/3K4/8/4R3 w - - 0 1",
            // Boundary around the 1300-material threshold:
            // queen + rook (1400, middlegame king tables)...
            "k7/8/8/8/8/8/8/K3Q2R w - - 0 1",
            // ...and queen alone (900, endgame king tables).
            "k7/8/8/8/8/8/8/K3Q3 w - - 0 1",
        ] {
            let pos = fen_position(fen);
            let mut rng = XorShift64::new();
            assert_eq!(
                evaluate(&pos, &mut rng, false),
                evaluate_reference(&pos),
                "score drift on {fen}"
            );
        }
    }

    #[test]
    fn each_difficulty_returns_a_legal_move_from_start() {
        let start = Chess::default();
        for difficulty in [1u8, 2, 3] {
            let uci = best_move(&start, difficulty).expect("a move");
            let m = Uci::from_bytes(uci.as_bytes())
                .unwrap()
                .to_move(&start)
                .expect("parse");
            assert!(
                start.clone().play(&m).is_ok(),
                "illegal {uci} at difficulty {difficulty}"
            );
        }
    }

    #[test]
    fn promotion_capture_uses_full_uci() {
        // White: P b7, K h1. Black: Q a8 (undefended), K e8. The pawn's
        // capture-promotion on a8 wins the queen and is clearly best.
        let pos = fen_position("q3k3/1P6/8/8/8/8/8/7K w - - 0 1");
        let best = best_move(&pos, 3).expect("a move");
        assert_eq!(best, "b7a8q");
        assert_eq!(best.len(), 5, "promotion UCI must carry the piece");
    }

    #[test]
    fn medium_and_hard_are_deterministic() {
        let start = Chess::default();
        assert_eq!(best_move(&start, 2), best_move(&start, 2));
        assert_eq!(best_move(&start, 3), best_move(&start, 3));
    }

    /// Easy (difficulty 1) perturbs leaf evaluations with
    /// xorshift noise (design D6), so consecutive CPU moves
    /// are not all the same, unlike deterministic medium/hard.
    /// Over enough calls the ±100 cp jitter must produce at
    /// least two distinct moves from the start position — it
    /// effectively always does, because the jitter dwarfs the
    /// small evaluation differences between opening moves.
    #[test]
    fn easy_moves_vary_between_calls() {
        let start = Chess::default();
        let mut distinct = std::collections::HashSet::new();
        for _ in 0..100 {
            let uci = best_move(&start, 1).expect("a move");
            distinct.insert(uci);
            if distinct.len() > 1 {
                return;
            }
        }
        panic!("easy returned the same move 100 times; noise lost?");
    }

    #[test]
    fn hard_move_from_start_fits_time_budget() {
        let start = Chess::default();
        let t0 = std::time::Instant::now();
        let _ = best_move(&start, 3);
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(1),
            "hard move took {:?}",
            t0.elapsed()
        );
    }

    /// Documented CPU move time budget per difficulty, in
    /// milliseconds, on the reference hardware (Apple M2 Max,
    /// release build — see `core/benches/README.md`). A
    /// synchronous `get_cpu_move` call has this bounded cost.
    /// Debug builds are ~20x slower on the same hardware, so
    /// they get a 2 s per-difficulty allowance; run
    /// `cargo test --release` to assert the documented budgets.
    #[cfg(not(debug_assertions))]
    const CPU_MOVE_BUDGET_MS: [u64; 3] = [5, 50, 100];
    #[cfg(debug_assertions)]
    const CPU_MOVE_BUDGET_MS: [u64; 3] = [2000, 2000, 2000];

    /// The hardest move stays within the documented time budget
    /// (spec: "CPU Move Within a Documented Time Budget"): every
    /// difficulty returns within its budget, from the start
    /// position and the representative midgame positions.
    #[test]
    fn cpu_move_fits_documented_budget() {
        let positions = [
            ("start", Chess::default()),
            (
                "italian",
                fen_position("r1bqkbnr/pppp1ppp/2n5/4p3/4P3/5N2/PPPP1PPP/RNBQKB1R w KQkq - 2 3"),
            ),
            (
                "ruy_lopez",
                fen_position("r1bqk2r/pppp1ppp/2n2n2/1B2p3/4P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 2 5"),
            ),
        ];
        for (difficulty, budget_ms) in (1..=3u8).zip(CPU_MOVE_BUDGET_MS) {
            for (name, pos) in &positions {
                let start = std::time::Instant::now();
                let uci = best_move(pos, difficulty);
                assert!(uci.is_some(), "no move at difficulty {difficulty}");
                let elapsed = start.elapsed();
                assert!(
                    elapsed < std::time::Duration::from_millis(budget_ms),
                    "difficulty {difficulty} from {name} took {elapsed:?}, \
                     over the {budget_ms} ms budget"
                );
            }
        }
    }

    #[test]
    fn no_move_when_side_to_move_is_mated() {
        // Back-rank mate: white rook on a8 checks the black king on g8 along
        // the 8th rank; the black pawns on f7/g7/h7 block every escape.
        let pos = fen_position("R5k1/5ppp/8/8/8/8/8/4K3 b - - 0 1");
        assert!(pos.is_checkmate());
        assert_eq!(best_move(&pos, 2), None);
        assert_eq!(best_move(&pos, 3), None);
    }

    #[test]
    fn unknown_difficulty_is_rejected() {
        let start = Chess::default();
        assert_eq!(best_move(&start, 0), None);
        assert_eq!(best_move(&start, 4), None);
    }
}
