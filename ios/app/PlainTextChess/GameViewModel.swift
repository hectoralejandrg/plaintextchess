import Foundation
import Combine

/// Milestone-1 game status (design D6: pulled from the core after each move).
enum GameStatus: Equatable {
    case starting
    case playing(toMove: String, inCheck: Bool)
    case checkmated(winner: String)
    case drawn
    /// App-level terminal state (design D4 of add-game-end-dialog): the
    /// resigner is the losing side, `winner` the opponent.
    case resigned(winner: String)
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

    /// Game-end dialog visibility (design D2 of add-game-end-dialog): the VM
    /// owns the presentation so the "appears once" rule has no view-layer
    /// race. `ContentView` binds the alert to this flag.
    @Published var showGameEndDialog = false
    /// Board orientation (design D6 of add-game-end-dialog): 0 = White on the
    /// bottom, 180 = board rotated. A display preference: it persists across
    /// new games in the session and is never reset by `startGame`.
    @Published private(set) var boardOrientation = 0

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

    /// Game-end dialog bookkeeping (design D2 of add-game-end-dialog): the
    /// modal is presented once per game and never re-presented after the
    /// player dismisses it; `startGame` resets both flags.
    private var gameEndPresented = false
    private var gameEndDismissed = false

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

    /// Whether the "New game" action may be started right now (spec:
    /// enforce-single-active-game "Game Mode Selection"): a new game may be
    /// started only when the current game is finished (checkmate, draw,
    /// resignation), when no move of it has been played yet, or when it is in
    /// an error state (recovery). While a game with at least one played move
    /// is in progress, New game is unavailable. Single source of truth: the
    /// UI button binds to it and `startGame` guards on it (design D1);
    /// `restart()` needs no special-casing because terminal ⇒ gate open (D2).
    var canStartNewGame: Bool {
        switch status {
        case .checkmated, .drawn, .resigned, .failed:
            return true
        case .starting, .playing:
            return moveList.isEmpty
        }
    }

    /// Discard the current session and start a fresh game in `mode`
    /// (spec: new-game action; design D4/D5/D6).
    func startGame(_ mode: GameMode) {
        // Single-active-game rule (enforce-single-active-game D1): never
        // replace a game that is still in progress.
        guard canStartNewGame else { return }
        gameMode = mode
        cpuGeneration += 1
        cpuThinking = false
        // Game-end dialog: a fresh game gets a fresh dialog (design D2).
        gameEndPresented = false
        gameEndDismissed = false
        showGameEndDialog = false
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

    // MARK: - Game-end dialog (design D2/D3 of add-game-end-dialog)

    /// The result message for the game-end modal (design D3), or `nil` while
    /// the game is not terminal. Computed from `status` + `gameMode` so it can
    /// never desynchronize from the status.
    var gameEndMessage: String? {
        switch status {
        case .checkmated(let winner):
            if case .cpu = gameMode {
                return winner == "White" ? "Checkmate! You won!" : "Checkmate! You lost."
            }
            return "Checkmate! \(winner) wins."
        case .resigned(let winner):
            if case .cpu = gameMode {
                // Only the human resigns against the CPU (design D4/D6 of
                // add-cpu-opponent: the human is White).
                return "You resigned. You lost."
            }
            let loser = winner == "White" ? "Black" : "White"
            return "\(loser) resigns. \(winner) wins."
        case .drawn:
            return "The game is drawn."
        default:
            return nil
        }
    }

    /// "Play again" in the game-end dialog (design D2): a fresh game in the
    /// current mode (and CPU difficulty).
    func restart() {
        startGame(gameMode)
    }

    /// "Done" in the game-end dialog (design D2): close the modal without
    /// changing the game. The status line keeps showing the result.
    func dismissGameEnd() {
        gameEndDismissed = true
        showGameEndDialog = false
    }

    // MARK: - Resign / undo / flip (design D4/D5/D6 of add-game-end-dialog)

    /// Whether the game can be resigned or undone right now.
    var canResign: Bool {
        if case .playing = status { return true }
        return false
    }

    var canUndo: Bool {
        guard !moveList.isEmpty else { return false }
        guard case .playing = status else { return false }
        return true
    }

    /// Resign the current game (design D4): an app-level terminal state. In a
    /// two-player game the side to move resigns; against the CPU the human
    /// (White) resigns. Bumping the generation discards an in-flight CPU move
    /// (same mechanism as `startGame`, design D5 of add-cpu-opponent).
    func resign() {
        guard case .playing = status else { return }
        cpuGeneration += 1
        cpuThinking = false
        clearSelection()
        lastMove = nil
        let winner: String
        switch gameMode {
        case .twoPlayers:
            winner = toMove == "w" ? "Black" : "White"
        case .cpu:
            winner = "Black"
        }
        status = .resigned(winner: winner)
        errorMessage = nil
        markTerminalIfNeeded()
    }

    /// Take back the last move (design D5): the core has no undo, so a fresh
    /// session is replayed with the kept UCI moves. Two players: one ply; CPU:
    /// the last pair so it is the human's turn again (one ply when the CPU is
    /// still thinking its reply, which the generation bump discards).
    func undo() {
        guard canUndo else { return }
        let kept: [String]
        if case .cpu = gameMode {
            // Even count: the CPU already replied → take the whole pair back.
            kept = moveList.count.isMultiple(of: 2)
                ? Array(moveList.dropLast(2))
                : Array(moveList.dropLast(1))
        } else {
            kept = Array(moveList.dropLast(1))
        }
        // Discard any in-flight CPU move (design D5 generation guard).
        cpuGeneration += 1
        cpuThinking = false
        do {
            let rebuilt = newGameSession(initialRating: 1500.0)
            var side = "w"
            for uci in kept {
                try rebuilt.playMove(uciMove: uci)
                side = side == "w" ? "b" : "w"
            }
            // Replay succeeded: commit the rebuilt session and bookkeeping.
            // (On failure nothing above touched the live state.)
            session = rebuilt
            toMove = side
            moveList = kept
            clearSelection()
            if let lastUci = kept.last {
                // 5-char promotion UCIs included: destination is chars 2-3.
                lastMove = LastMove(from: String(lastUci.prefix(2)),
                                    to: String(lastUci.dropFirst(2).prefix(2)))
            } else {
                lastMove = nil
            }
            board = try FenBoard(board: rebuilt.getBoardState(), sideToMove: toMove)
            refreshStatus()
        } catch {
            errorMessage = "Undo failed: \(error)"
        }
    }

    /// Flip the board orientation (design D6): 0 = White on the bottom,
    /// 180 = rotated. Display-only: the game state is untouched.
    func flipBoard() {
        boardOrientation = boardOrientation == 0 ? 180 : 0
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
        // Defense in depth (enforce-single-active-game D3): capture the
        // session that computes the move so the commit can verify identity in
        // addition to the generation guard, closing any future code path that
        // could replace the session without bumping the generation.
        let computingSession = self.session
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let result: Result<String, Error>
            do {
                let uci = try computingSession.getCpuMove(difficulty: difficulty.rawValue)
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
                guard self.session === computingSession else { return }
                switch result {
                case .success(let uci):
                    #if DEBUG
                    if self.debugCpuDelay > 0 {
                        // Hold the "thinking" state to observe it (test hook).
                        DispatchQueue.main.asyncAfter(deadline: .now() + self.debugCpuDelay) { [weak self] in
                            self?.applyCpuMove(uci,
                                              generation: generation,
                                              session: computingSession)
                        }
                    } else {
                        self.applyCpuMove(uci,
                                          generation: generation,
                                          session: computingSession)
                    }
                    #else
                    self.applyCpuMove(uci,
                                      generation: generation,
                                      session: computingSession)
                    #endif
                case .failure(let error):
                    self.cpuThinking = false
                    self.errorMessage = "CPU move failed: \(error)"
                }
            }
        }
    }

    /// Apply a computed CPU move through the same internal path a human move
    /// uses, but only if the game generation still matches (design D5) and
    /// the session that computed it is still the active one
    /// (enforce-single-active-game D3): a stale move from a replaced session
    /// is never applied to the session that follows.
    private func applyCpuMove(_ uci: String, generation: Int, session: GameSession) {
        guard cpuGeneration == generation, session === self.session else {
            // A newer game started (or the session was rebuilt) in the
            // meantime: discard the stale move.
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
            markTerminalIfNeeded()
        } catch {
            status = .failed("\(error)")
            errorMessage = "FFI error: \(error)"
        }
    }

    /// Design D2 of add-game-end-dialog: present the game-end dialog exactly
    /// once, the first time the game becomes terminal.
    private func markTerminalIfNeeded() {
        guard !gameEndPresented else { return }
        switch status {
        case .checkmated, .drawn, .resigned:
            gameEndPresented = true
            showGameEndDialog = true
        default:
            break
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
                // Exercises the same New game action the button triggers, so
                // it honors the single-active-game gate too: mid-game it is
                // rejected by the `startGame` guard (no state touched).
                self.newGame()
            } else if token == "cancelpromo" {
                // Exercises the same intent the picker's outside-tap catcher
                // calls (design D5).
                self.cancelPromotion()
            } else if token == "undo" || token == "resign" || token == "flip"
                || token == "done" || token == "playagain" {
                // add-game-end-dialog: drive the new controls and the game-end
                // dialog through the same intent path the buttons call.
                switch token {
                case "undo": self.undo()
                case "resign": self.resign()
                case "flip": self.flipBoard()
                case "done": self.dismissGameEnd()
                default: self.restart()
                }
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
