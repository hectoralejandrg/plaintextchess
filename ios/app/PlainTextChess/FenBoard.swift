import Foundation

/// Errors raised while rendering-side processing of core data.
enum FenError: Error {
    case malformed(String)
}

/// An 8x8 board parsed from the board string returned by the core
/// (design D2: the board state returned by the core is the app-side source of
/// truth; the app never re-implements rules).
///
/// Note: `get_board_state()` returns the 8-rank position only (no FEN
/// side-to-move field), so the caller supplies `sideToMove` - the view model
/// tracks it locally and flips it after each played move.
///
/// Layout: `grid[row][col]` with row 0 = rank 8 (top of the screen) and
/// col 0 = file a (left). Pieces are single characters ("K", "q", ...);
/// empty squares are "".
struct FenBoard: Equatable {
    let grid: [[String]]
    /// "w" or "b" - the side to move, tracked by the view model.
    let sideToMove: String

    static let startBoard = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR"

    private static let pieceChars: Set<Character> = Set("pnbrqkPNBRQK")

    init(board: String, sideToMove: String) throws {
        guard sideToMove == "w" || sideToMove == "b" else {
            throw FenError.malformed(board)
        }
        let ranks = board.split(separator: "/").map { String($0) }
        guard ranks.count == 8 else { throw FenError.malformed(board) }

        var grid: [[String]] = []
        for rank in ranks {
            var row: [String] = []
            for ch in rank {
                if let count = Int(String(ch)) {
                    for _ in 0..<count { row.append("") }
                } else if Self.pieceChars.contains(ch) {
                    row.append(String(ch))
                } else {
                    throw FenError.malformed(board)
                }
            }
            guard row.count == 8 else { throw FenError.malformed(board) }
            grid.append(row)
        }
        self.grid = grid
        self.sideToMove = sideToMove
    }

    /// "e2" from grid coordinates (row 0 = rank 8, col 0 = file a).
    static func squareName(row: Int, col: Int) -> String {
        let files = "abcdefgh"
        let file = files.dropFirst(col).prefix(1)
        return "\(file)\(8 - row)"
    }

    /// Grid coordinates for a square name ("e2"); nil when not a valid square.
    static func parseSquare(_ name: String) -> (row: Int, col: Int)? {
        let files = "abcdefgh"
        var chars = name.makeIterator()
        guard
            let file = chars.next(),
            let rankChar = chars.next(),
            chars.next() == nil,
            let fileIndex = files.firstIndex(of: file),
            let rank = Int(String(rankChar)),
            (1...8).contains(rank)
        else { return nil }
        let col = files.distance(from: files.startIndex, to: fileIndex)
        return (row: 8 - rank, col: col)
    }
}
