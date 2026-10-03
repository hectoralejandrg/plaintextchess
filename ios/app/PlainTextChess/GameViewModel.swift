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

/// Opponent mode for a game (design D4/D6): two human players, or a human
/// (White) versus the CPU (Black) at a chosen difficulty.
enum GameMode: Equatable {
    case twoPlayers
    case cpu(CpuDifficulty)
}

/// CPU difficulty levels, matching the core's search depth (design D2):
/// 1 easy, 2 medium, 3 hard. Raw type is `Int32` to match the FFI `i32`.
enum CpuDifficulty: Int32, CaseIterable {
    case easy = 1
    case medium = 2
    case hard = 3

    var displayName: String {
        switch self {
        case .easy:
            return "Easy"
        case .medium:
            return "Medium"
        case .hard:
            return "Hard"
        }
    }

    /// Parse a name ("easy", "medium", "hard") from launch arguments.
    static func named(_ name: String) -> CpuDifficulty? {
        switch name.lowercased() {
        case "easy":
            return .easy
        case "medium":
            return .medium
        case "hard":
            return .hard
        default:
            return nil
        }
    }
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
    /// True while the CPU is computing its reply (design D5): the board input
    /// is locked and the status row shows "CPU is thinking…".
    @Published private(set) var cpuThinking = false

    private var session: GameSession
    /// Full UCI moves of the currently selected piece (design D1): the
    /// 5-character ones are the promotion options.
    private var selectedMoves: [String] = []
    /// Side to move, tracked locally: the core's board state carries only the
    /// position, and the app is the session's sole operator in milestone 1.
    private var toMove: String = "w"

    /// Opponent mode for the current game (design D4/D6).
    private var gameMode: GameMode = .twoPlayers
    /// The side the CPU plays (design D6: the CPU is always Black).
    private let cpuColor = "b"
    /// Bumped on every `startGame` so an in-flight CPU move from an older game
    /// is discarded instead of applied (design D5).
    private var cpuGeneration = 0

    #if DEBUG
    /// DEBUG test hook (like `ANIM_SCALE`/`DRAG`): seconds to hold the
    /// "CPU is thinking…" state before applying the computed move, so the
    /// thinking window and the input lock are deterministic to observe on the
    /// simulator. Inert unless set via `-PLAINTCHESS_CPU_DELAY`.
    private var debugCpuDelay: Double = 0.0

    func setDebugCpuDelay(_ seconds: Double) {
        debugCpuDelay = seconds
    }
    #endif

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
        // Locked while the CPU is thinking (design D5): no selection, no move.
        guard !cpuThinking else { return }
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
        // Locked while the CPU is thinking (design D5).
        guard !cpuThinking else { return false }
        guard case .playing(let toMove, _) = status else { return false }
        guard let (row, col) = FenBoard.parseSquare(squareName) else { return false }
        let piece = board.grid[row][col]
        return !piece.isEmpty && isOwn(piece, toMove: toMove)
    }

    /// Discard the current session and start a fresh game in `mode`
    /// (spec: new-game action; design D4/D5/D6).
    func startGame(_ mode: GameMode) {
        gameMode = mode
        cpuGeneration += 1
        cpuThinking = false
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
            return
        }
        triggerCpuMoveIfNeeded()
    }

    /// Start a two-player game. Kept as the convenience the **New game**
    /// action and the DEBUG `newgame` script token use (design D4).
    func newGame() {
        startGame(.twoPlayers)
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
            // After any successful move (human or CPU), let the CPU answer if
            // it is its turn (design D5/D6). A no-op after a CPU move.
            triggerCpuMoveIfNeeded()
        } catch {
            errorMessage = "Move failed: \(error)"
        }
    }

    /// Run the CPU driver when the CPU is to move (design D5/D6): mode is cpu,
    /// it is the CPU's color, the game is still playing, and no move is in
    /// flight. The move is computed off the main thread and applied on the main
    /// queue through the same internal path a human move uses, but only if the
    /// generation still matches (a newer game discards the stale result).
    private func triggerCpuMoveIfNeeded() {
        guard case .cpu(let difficulty) = gameMode else { return }
        guard case .playing(let toMove, _) = status else { return }
        guard toMove == cpuColor else { return }
        guard !cpuThinking else { return }

        cpuThinking = true
        let generation = cpuGeneration
        let session = self.session
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let result: Result<String, Error>
            do {
                let uci = try session.getCpuMove(difficulty: difficulty.rawValue)
                result = .success(uci)
            } catch {
                result = .failure(error)
            }
            DispatchQueue.main.async {
                guard let self else { return }
                // A newer game started while we computed: drop the stale move
                // (design D5). `startGame` already cleared `cpuThinking` for
                // the new game, so there is nothing to undo here.
                guard self.cpuGeneration == generation else { return }
                switch result {
                case .success(let uci):
                    #if DEBUG
                    if self.debugCpuDelay > 0 {
                        // Hold the "thinking" state to observe it (test hook).
                        DispatchQueue.main.asyncAfter(deadline: .now() + self.debugCpuDelay) { [weak self] in
                            self?.applyCpuMove(uci, generation: generation)
                        }
                    } else {
                        self.applyCpuMove(uci, generation: generation)
                    }
                    #else
                    self.applyCpuMove(uci, generation: generation)
                    #endif
                case .failure(let error):
                    self.cpuThinking = false
                    self.errorMessage = "CPU move failed: \(error)"
                }
            }
        }
    }

    /// Apply a computed CPU move through the same internal path a human move
    /// uses, but only if the game generation still matches (design D5).
    private func applyCpuMove(_ uci: String, generation: Int) {
        guard cpuGeneration == generation else {
            // A newer game started in the meantime: discard the stale move.
            return
        }
        cpuThinking = false
        // 5-char promotion UCIs included: destination is chars 2-3.
        let from = String(uci.prefix(2))
        let to = String(uci.dropFirst(2).prefix(2))
        playMove(from: from, to: to, uci: uci)
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
