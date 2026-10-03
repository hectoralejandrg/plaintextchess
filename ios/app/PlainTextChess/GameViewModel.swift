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

    /// A promotion awaiting the player's choice (design D1): the core reports
    /// a last-rank destination with one UCI move per promotion piece.
    struct PendingPromotion: Equatable {
        let from: String
        let to: String
        /// Full UCI options, ordered q/r/b/n.
        let options: [String]
    }

    @Published private(set) var board: FenBoard
    @Published private(set) var status: GameStatus = .starting
    @Published private(set) var selectedSquare: String?
    @Published private(set) var legalTargets: [String] = []
    @Published private(set) var lastMove: LastMove?
    @Published private(set) var moveList: [String] = []
    @Published private(set) var errorMessage: String?
    @Published private(set) var pendingPromotion: PendingPromotion?

    private var session: GameSession
    /// Full UCI moves of the currently selected piece (design D1): the
    /// 5-character ones are the promotion options.
    private var selectedMoves: [String] = []
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
                // D1: a last-rank destination with promotion UCI options opens
                // the picker instead of playing.
                if let pending = pendingPromotionFor(from: selected, to: squareName) {
                    pendingPromotion = pending
                    return
                }
                playMove(from: selected, to: squareName,
                         uci: selected + squareName)
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
            // The core returns full UCI strings ("e2e4", "e7e8q", ...). Keep
            // the full strings (promotion detection, D1) and derive the
            // destination squares for highlighting and tap matching.
            let uciMoves = (try? session.getValidMoves(square: squareName)) ?? []
            selectedMoves = uciMoves.filter { $0.hasPrefix(squareName) }
            // Destination is chars 2-3 (5-char promotion UCIs end in the
            // promotion piece, so `suffix(2)` is wrong for those).
            legalTargets = selectedMoves.map { String($0.dropFirst(2).prefix(2)) }
            errorMessage = nil
        } else {
            // Enemy piece that is not a legal capture target.
            if selectedSquare != nil {
                errorMessage = "Not a legal move"
            }
        }
    }

    /// Play the chosen promotion piece (design D1). `piece` is one of
    /// q/r/b/n as listed in `pendingPromotion.options`.
    func confirmPromotion(_ piece: String) {
        guard let pending = pendingPromotion else { return }
        pendingPromotion = nil
        playMove(from: pending.from, to: pending.to,
                 uci: pending.from + pending.to + piece)
    }

    /// Dismiss an open promotion choice without playing (design D2: any tap
    /// outside the picker cancels).
    func cancelPromotion() {
        pendingPromotion = nil
        clearSelection()
    }

    /// Whether the piece on this square may be picked up right now. Used by
    /// the drag gesture; the tap path answers the same question inside
    /// `select`.
    func canPickup(_ squareName: String) -> Bool {
        guard case .playing(let toMove, _) = status else { return false }
        guard let (row, col) = FenBoard.parseSquare(squareName) else { return false }
        let piece = board.grid[row][col]
        return !piece.isEmpty && isOwn(piece, toMove: toMove)
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

    /// D1: promotions are the 5-character UCI moves. Group them by
    /// destination and order the options q/r/b/n.
    private func pendingPromotionFor(from: String, to: String) -> PendingPromotion? {
        let promos = selectedMoves.filter {
            $0.count == 5 && $0.hasPrefix(from)
                && String($0.dropFirst(2).prefix(2)) == to
        }
        guard !promos.isEmpty else { return nil }
        let options: [String] = ["q", "r", "b", "n"].compactMap { piece in
            promos.first { $0.hasSuffix(piece) }
        }
        guard !options.isEmpty else { return nil }
        return PendingPromotion(from: from, to: to, options: options)
    }

    private func playMove(from: String, to: String, uci: String) {
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
        selectedMoves = []
        pendingPromotion = nil
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
            } else if token == "cancelpromo" {
                // Exercises the same intent the picker's outside-tap catcher
                // calls (design D5).
                self.cancelPromotion()
            } else if case .playing = self.status {
                // Drive the same intent path as a real user: select the
                // origin, then the destination. An illegal destination
                // exercises the "not a legal move" rejection path. A
                // 5-character token (e.g. "a2a1q") additionally confirms the
                // promotion through the same intent the picker button calls;
                // a 4-char token landing on a promotion destination stops
                // with the picker open (screenshot-able, design D5).
                let from = String(token.prefix(2))
                let to = String(token.dropFirst(2).prefix(2))
                self.select(from)
                self.select(to)
                if token.count == 5 {
                    self.confirmPromotion(String(token.suffix(1)))
                }
            }
            self.playScriptNow(Array(moves.dropFirst()), interval: interval)
        }
    }
#endif
}
