import Foundation
import Combine

/// Milestone-1 game status (design D6: pulled from the core after each move).
enum GameStatus: Equatable {
    case starting
    case playing(toMove: String, inCheck: Bool)
    case checkmated(winner: String)
    case drawn
    case failed(String)
}

/// Single source of truth for the game screen (design D4).
///
/// All chess rules live in the Rust core through the UniFFI `GameSession`;
/// this view model only renders state and forwards user intents.
final class GameViewModel: ObservableObject {

    struct LastMove: Equatable {
        let from: String
        let to: String
    }

    @Published private(set) var board: FenBoard
    @Published private(set) var status: GameStatus = .starting
    @Published private(set) var selectedSquare: String?
    @Published private(set) var legalTargets: [String] = []
    @Published private(set) var lastMove: LastMove?
    @Published private(set) var moveList: [String] = []
    @Published private(set) var errorMessage: String?

    private var session: GameSession
    /// Side to move, tracked locally: the core's board state carries only the
    /// position, and the app is the session's sole operator in milestone 1.
    private var toMove: String = "w"

    init() {
        let session = newGameSession(initialRating: 1500.0)
        self.session = session
        do {
            self.board = try FenBoard(board: session.getBoardState(), sideToMove: toMove)
            refreshStatus()
        } catch {
            // FFI failure at startup: render it in the UI instead of crashing.
            self.board = try! FenBoard(board: FenBoard.startBoard, sideToMove: "w")
            self.status = .failed("\(error)")
            self.errorMessage = "FFI error: \(error)"
        }
    }

    // MARK: - Intents

    /// Two-tap selection loop (design D3): select an own piece, then tap a
    /// highlighted destination to play. Tapping a non-legal square keeps the
    /// selection and shows "not a legal move" feedback.
    func select(_ squareName: String) {
        guard case .playing(let toMove, _) = status else { return }
        guard let (row, col) = FenBoard.parseSquare(squareName) else { return }
        let piece = board.grid[row][col]

        if let selected = selectedSquare {
            if selected == squareName {
                clearSelection()
                return
            }
            if legalTargets.contains(squareName) {
                playMove(from: selected, to: squareName)
                return
            }
        }

        if piece.isEmpty {
            if selectedSquare != nil {
                errorMessage = "Not a legal move"
            }
            return
        }

        if isOwn(piece, toMove: toMove) {
            selectedSquare = squareName
            // The core returns full UCI strings ("e2e4", "e2e3"); keep only the
            // destination square for highlighting and tap matching.
            let uciMoves = (try? session.getValidMoves(square: squareName)) ?? []
            legalTargets = uciMoves
                .filter { $0.hasPrefix(squareName) }
                .map { String($0.suffix(2)) }
            errorMessage = nil
        } else {
            // Enemy piece that is not a legal capture target.
            if selectedSquare != nil {
                errorMessage = "Not a legal move"
            }
        }
    }

    /// Discard the current session and start a fresh game (spec: new-game action).
    func newGame() {
        session = newGameSession(initialRating: 1500.0)
        toMove = "w"
        clearSelection()
        lastMove = nil
        moveList = []
        do {
            board = try FenBoard(board: session.getBoardState(), sideToMove: toMove)
            refreshStatus()
        } catch {
            board = try! FenBoard(board: FenBoard.startBoard, sideToMove: "w")
            status = .failed("\(error)")
            errorMessage = "FFI error: \(error)"
        }
    }

    // MARK: - Internals

    private func playMove(from: String, to: String) {
        let uci = from + to
        do {
            try session.playMove(uciMove: uci)
            toMove = toMove == "w" ? "b" : "w"
            clearSelection()
            lastMove = LastMove(from: from, to: to)
            moveList.append(uci)
            board = try FenBoard(board: session.getBoardState(), sideToMove: toMove)
            refreshStatus()
        } catch {
            errorMessage = "Move failed: \(error)"
        }
    }

    private func refreshStatus() {
        do {
            if try session.isCheckmate() {
                // The side to move is mated, so the opponent wins.
                let winner = board.sideToMove == "w" ? "Black" : "White"
                status = .checkmated(winner: winner)
            } else if try session.isDraw() {
                status = .drawn
            } else if try session.isCheck() {
                status = .playing(toMove: board.sideToMove, inCheck: true)
            } else {
                status = .playing(toMove: board.sideToMove, inCheck: false)
            }
            errorMessage = nil
        } catch {
            status = .failed("\(error)")
            errorMessage = "FFI error: \(error)"
        }
    }

    private func clearSelection() {
        selectedSquare = nil
        legalTargets = []
    }

    private func isOwn(_ piece: String, toMove side: String) -> Bool {
        side == "w" ? piece == piece.uppercased() : piece == piece.lowercased()
    }

#if DEBUG
    /// Test hook for the simulator smoke flow (design: spec verification):
    /// `xcrun simctl launch <sim> <bundle> -PLAINTCHESS_SCRIPT "e2e4 e7e5"`
    /// plays the UCI sequence through the same intent path as taps.
    func playScript(_ moves: [String], interval: Double = 0.5) {
        playScriptNow(moves, interval: interval)
    }

    private func playScriptNow(_ moves: [String], interval: Double) {
        guard !moves.isEmpty else { return }
        let token = moves[0]
        DispatchQueue.main.asyncAfter(deadline: .now() + interval) { [weak self] in
            guard let self else { return }
            if token == "newgame" {
                // Exercises the same New game action the button triggers.
                self.newGame()
            } else if case .playing = self.status {
                // Drive the same two-tap intent path as a real user: select
                // the origin, then the destination. An illegal destination
                // exercises the "not a legal move" rejection path.
                self.select(String(token.prefix(2)))
                self.select(String(token.dropFirst(2)))
            }
            self.playScriptNow(Array(moves.dropFirst()), interval: interval)
        }
    }
#endif
}
