use shakmaty::fen::FenOpts;
use shakmaty::uci::Uci;
use shakmaty::{Chess, Color, Piece, Position, Role, Setup, Square};

pub struct BoardManager {
    board: Chess,
}

impl BoardManager {
    pub fn new() -> Self {
        Self {
            board: Chess::default(),
        }
    }

    pub fn get_fen(&self) -> String {
        shakmaty::fen::board_fen(self.board.board(), &FenOpts::new())
    }

    pub fn get_valid_moves(&self, square: &str) -> Vec<String> {
        let Ok(sq) = Square::from_bytes(square.as_bytes()) else {
            return Vec::new();
        };
        self.board
            .legals()
            .iter()
            .filter(|m| m.from() == Some(sq))
            .map(|m| Uci::from_move(&self.board, m).to_string())
            .collect()
    }

    pub fn play_move(&mut self, uci_move: &str) -> Result<(), &'static str> {
        let uci = Uci::from_bytes(uci_move.as_bytes()).map_err(|_| "Invalid UCI move")?;
        let m = uci.to_move(&self.board).map_err(|_| "Invalid move")?;
        let next = self.board.clone().play(&m).map_err(|_| "Move not legal")?;
        self.board = next;
        Ok(())
    }

    pub fn is_check(&self) -> bool {
        self.board.is_check()
    }

    pub fn is_checkmate(&self) -> bool {
        self.board.is_checkmate()
    }

    pub fn is_draw(&self) -> bool {
        !self.board.is_checkmate()
            && (self.board.is_stalemate() || self.board.is_insufficient_material())
    }

    /// Whether `color` cannot deliver checkmate with its current material
    /// (the standard flag-fall judgment). Single source in the core; the
    /// online server reads it through the `EngineSession` port.
    pub fn insufficient_material_for(&self, color: Color) -> bool {
        crate::domain::material::insufficient_to_mate(self.board.board(), color == Color::White)
    }

    pub fn get_piece_at(&self, square: &str) -> String {
        let Ok(sq) = Square::from_bytes(square.as_bytes()) else {
            return String::new();
        };
        match self.board.board().piece_at(sq) {
            Some(Piece { color, role }) => piece_char(color, role).to_string(),
            None => String::new(),
        }
    }

    /// CPU move for the side to move at the given difficulty (1 easy,
    /// 2 medium, 3 hard). Pure query: the board is not mutated. `None`
    /// when the difficulty is unknown or the side to move has no legal
    /// move (game over).
    pub fn best_move(&self, difficulty: u8) -> Option<String> {
        crate::domain::search::best_move(&self.board, difficulty)
    }
}

/// Standard chess piece letters: uppercase for white, lowercase for black.
fn piece_char(color: Color, role: Role) -> char {
    let c = match role {
        Role::King => 'k',
        Role::Queen => 'q',
        Role::Rook => 'r',
        Role::Bishop => 'b',
        Role::Knight => 'n',
        Role::Pawn => 'p',
    };
    if color == Color::White {
        c.to_ascii_uppercase()
    } else {
        c
    }
}
