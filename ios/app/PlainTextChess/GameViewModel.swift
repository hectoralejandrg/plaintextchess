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
    /// Online terminal state (add-online-multiplayer D4): the absent player
    /// lost by the server's reconnect-window forfeit; `winner` is the
    /// connected player's color.
    case forfeited(winner: String)
    /// Online terminal state (add-online-time-controls): a player's clock ran
    /// out (flag fall); `winner` is the opponent's color. A flag fall with
    /// insufficient material ends as `.drawn`, not here.
    case timedOut(winner: String)
    case failed(String)
}

/// Opponent mode for a game (design D4/D6): two human players, a human
/// (White) versus the CPU (Black) at a chosen difficulty, or an online game
/// against another device (add-online-multiplayer D4/D7).
enum GameMode: Equatable {
    case twoPlayers
    case cpu(CpuDifficulty)
    case online
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

/// The online clock as reported by the latest server snapshot
/// (add-online-time-controls, design D6). `whiteMs`/`blackMs` are the
/// snapshot's authoritative remaining times captured at `reference`; the
/// view derives the live display value with `remaining(for:now:)`, which only
/// interpolates the side-to-move countdown (the waiting side is frozen).
struct OnlineClock: Equatable {
    let whiteMs: Int
    let blackMs: Int
    let sideToMove: String
    let reference: Date
    let isRunning: Bool

    func remaining(for color: String, now: Date) -> Int {
        let base = color == "w" ? whiteMs : blackMs
        guard isRunning, color == sideToMove else { return max(0, base) }
        let elapsed = Int(now.timeIntervalSince(reference) * 1000)
        return max(0, base - elapsed)
    }

    /// `m:ss` (drops to `m:ss` at ≥ 10 s, matching the platform clock style).
    static func format(_ milliseconds: Int) -> String {
        let seconds = max(0, milliseconds) / 1000
        return String(format: "%d:%02d", seconds / 60, seconds % 60)
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

    // MARK: Online multiplayer state (add-online-multiplayer D4/D7/D8).

    /// Connection phase mirrored from the `OnlineConnectionManager` (SwiftUI
    /// observes the view model, so the manager's state is mirrored here from
    /// its callbacks).
    @Published private(set) var onlinePhase: OnlinePhase = .idle
    /// True while the socket is dropped and the manager re-attaches; the UI
    /// shows a banner and the last known position stays on screen.
    @Published private(set) var onlineReconnecting = false
    /// The room code of the current online room (shown with a copy action in
    /// the waiting state).
    @Published private(set) var onlineRoomCode: String?
    /// Our color in the online game ("w"/"b"), from the server snapshot.
    @Published private(set) var onlineYourColor: String?
    /// True once the server confirmed the seat (`room_ready` received): the
    /// setup sheet dismisses itself.
    @Published private(set) var onlineSessionReady = false
    /// A join-time error, shown on the join screen while the sheet stays open.
    @Published private(set) var onlineJoinError: String?
    /// Whether the opponent's seat is currently connected, per the last
    /// snapshot: false in the lobby (our room, no opponent yet), which gates
    /// input and resign until the game has actually started.
    @Published private(set) var onlineOpponentOnline = false
    /// Ratings from the last snapshot, by color (add-online-multiplayer D5).
    /// Shown in the terminal-state modal so both clients see the updated
    /// ratings in the final state (design D11, step 4).
    @Published private(set) var onlineWhiteRating: Double = 1500
    @Published private(set) var onlineBlackRating: Double = 1500

    /// The server's clock as of the last snapshot (add-online-time-controls,
    /// design D6): the snapshot times are authoritative; the view interpolates
    /// the side-to-move countdown between snapshots for a smooth display.
    @Published private(set) var onlineClock: OnlineClock?
    /// The room's time control label from the last snapshot (lobby included).
    @Published private(set) var onlineTimeControl: String = OnlineTimeControl.default.label

    // MARK: - Auth state (add-auth-ui-clients)

    /// Session token issued by a successful register/login; `nil` while the
    /// player is a guest (`device_id` play stays the default identity).
    @Published private(set) var authToken: String?
    /// The profile display name the signed-in player last set via
    /// `set_profile` (iOS task 3.2); `nil` until one is accepted.
    @Published private(set) var authDisplayName: String?
    /// The last register/login failure, shown in the new-game sheet (e.g.
    /// the server's generic `invalid_credentials` message).
    @Published private(set) var authError: String?
    /// The auth reply expected next: register/login store the issued token,
    /// logout clears it (task 3.1), set_profile records the display name
    /// (task 3.2).
    private enum AuthPendingAction { case login, register, logout, setProfile }
    private var pendingAuthAction: AuthPendingAction = .login
    /// The trimmed `display_name` submitted for the `set_profile` in flight,
    /// recorded on `profile_updated` (task 3.2).
    private var pendingDisplayName: String?

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
    /// Live online session (add-online-multiplayer D7); `nil` for local games.
    private var online: OnlineConnectionManager?

    /// Persistent device identifier (add-online-multiplayer D8): the server
    /// tracks players by it, so a re-attach after a drop or an app relaunch
    /// re-enters the same seat.
    static let deviceIDKey = "plaintextchess.device_id"
    /// Persisted room code of an in-progress online game, so an app relaunch
    /// can re-attach within the server's reconnect window.
    static let onlineRoomKey = "plaintextchess.online_room_code"

    /// The stable per-device identifier: created once, reused forever.
    static var deviceID: String {
        if let existing = UserDefaults.standard.string(forKey: deviceIDKey),
           !existing.isEmpty
        {
            return existing
        }
        let id = UUID().uuidString
        UserDefaults.standard.set(id, forKey: deviceIDKey)
        return id
    }

    /// True when the current game is online (the UI switches to the online
    /// wording: "Your move", You won/lost modal, no Play again, no Undo).
    var isOnlineMode: Bool {
        if case .online = gameMode {
            return true
        }
        return false
    }

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

    /// DEBUG test hook (add-online-multiplayer): the online server URL from
    /// `-PLAINTCHESS_ONLINE_URL`; the `onlinecreate` and `onlinejoin:<code>`
    /// script tokens use it to drive a live server from the script.
    private var debugOnlineURL: String?

    func setDebugOnlineURL(_ url: String?) {
        debugOnlineURL = url
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
        // Online: input is only accepted while the game is in progress and on
        // our own turn (add-online-multiplayer D4). The board stays inert in
        // the lobby (opponent not seated, no move played) and while the
        // opponent is to move; a temporarily disconnected opponent does not
        // lock the board, since the server still accepts our moves.
        if case .online = gameMode,
           toMove != onlineYourColor || (!onlineOpponentOnline && moveList.isEmpty) { return }
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
        // Online: only our own color may be picked up, once the game has
        // started (add-online-multiplayer D4) — same rule as `select`.
        if case .online = gameMode,
           (!onlineOpponentOnline && moveList.isEmpty) || toMove != onlineYourColor { return false }
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
        case .checkmated, .drawn, .resigned, .forfeited, .timedOut, .failed:
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
        // A local game replaces any open online session
        // (add-online-multiplayer D7/D8): close the socket and forget the
        // persisted room.
        if isOnlineMode {
            teardownOnlineSession()
        }
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
            if case .online = gameMode {
                return wonOnline(winner) ? "You won.\n\(onlineRatingsLine)"
                                        : "You lost.\n\(onlineRatingsLine)"
            }
            if case .cpu = gameMode {
                return winner == "White" ? "Checkmate! You won!" : "Checkmate! You lost."
            }
            return "Checkmate! \(winner) wins."
        case .resigned(let winner):
            if case .online = gameMode {
                return wonOnline(winner) ? "You won.\n\(onlineRatingsLine)"
                                        : "You lost.\n\(onlineRatingsLine)"
            }
            if case .cpu = gameMode {
                // Only the human resigns against the CPU (design D4/D6 of
                // add-cpu-opponent: the human is White).
                return "You resigned. You lost."
            }
            let loser = winner == "White" ? "Black" : "White"
            return "\(loser) resigns. \(winner) wins."
        case .forfeited(let winner):
            // Online only (add-online-multiplayer D4): the absent player lost
            // when the server's reconnect window closed.
            return wonOnline(winner) ? "You won.\n\(onlineRatingsLine)"
                                     : "You lost.\n\(onlineRatingsLine)"
        case .timedOut(let winner):
            // Online only (add-online-time-controls D4): a flag fall is
            // stated as a win/loss on time.
            return wonOnline(winner) ? "You won on time.\n\(onlineRatingsLine)"
                                     : "You lost on time.\n\(onlineRatingsLine)"
        case .drawn:
            if case .online = gameMode {
                return "Draw.\n\(onlineRatingsLine)"
            }
            return "The game is drawn."
        default:
            return nil
        }
    }

    /// The ratings footer for the online game-end modal (design D11, step 4):
    /// our rating first, the opponent's second, rounded to whole points.
    private var onlineRatingsLine: String {
        let (mine, theirs) = onlineYourColor == "w"
            ? (onlineWhiteRating, onlineBlackRating)
            : (onlineBlackRating, onlineWhiteRating)
        return "Your rating: \(Int(mine.rounded())) · Opponent: \(Int(theirs.rounded()))"
    }

    /// Online perspective (add-online-multiplayer D4): does the named winner
    /// color match the seat this device holds?
    private func wonOnline(_ winner: String) -> Bool {
        let square = winner == "White" ? "w" : "b"
        return square == onlineYourColor
    }

    /// "Play again" in the game-end dialog (design D2): a fresh game in the
    /// current mode (and CPU difficulty).
    func restart() {
        // Online games have no "Play again" (a rematch would need a new room):
        // the modal offers only "Done" there, and a stray call must not start
        // one.
        if isOnlineMode {
            dismissGameEnd()
            return
        }
        startGame(gameMode)
    }

    /// "Done" in the game-end dialog (design D2): close the modal without
    /// changing the game. The status line keeps showing the result.
    func dismissGameEnd() {
        gameEndDismissed = true
        showGameEndDialog = false
        // Online: release the seat so this device is free for a new room
        // (add-online-multiplayer D4).
        if isOnlineMode {
            online?.leave()
        }
    }

    // MARK: - Resign / undo / flip (design D4/D5/D6 of add-game-end-dialog)

    /// Whether the game can be resigned or undone right now.
    var canResign: Bool {
        guard case .playing = status else { return false }
        // Online: only once the game has actually started (the opponent is
        // seated) or a move was played — the lobby answers `not_connected`.
        if isOnlineMode, !onlineOpponentOnline, moveList.isEmpty {
            return false
        }
        return true
    }

    var canUndo: Bool {
        guard !moveList.isEmpty else { return false }
        guard case .playing = status else { return false }
        // Online: the server owns the move list, so there is nothing to take
        // back locally (add-online-multiplayer D4).
        if isOnlineMode { return false }
        return true
    }

    /// Resign the current game (design D4): an app-level terminal state. In a
    /// two-player game the side to move resigns; against the CPU the human
    /// (White) resigns. Bumping the generation discards an in-flight CPU move
    /// (same mechanism as `startGame`, design D5 of add-cpu-opponent).
    func resign() {
        guard case .playing = status else { return }
        clearSelection()
        // Online: the server records the resignation and confirms it with a
        // terminal snapshot (add-online-multiplayer D4); nothing changes
        // locally until that snapshot arrives.
        if isOnlineMode {
            online?.resign()
            return
        }
        cpuGeneration += 1
        cpuThinking = false
        lastMove = nil
        let winner: String
        switch gameMode {
        case .twoPlayers:
            winner = toMove == "w" ? "Black" : "White"
        case .cpu:
            winner = "Black"
        case .online:
            // Unreachable: the online branch returns above.
            return
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

    // MARK: - Online multiplayer (add-online-multiplayer D4/D7/D8)

    /// Start an online game: `create` takes the White seat of a new room;
    /// otherwise join the room `code` (Black seat). Returns `false` without
    /// touching the game when the New game gate is closed or no server is
    /// configured (the setup sheet stays open and shows the reason).
    @discardableResult
    func startOnlineGame(create: Bool, code: String? = nil,
                         timeControl: String? = nil,
                         serverURLString: String?) -> Bool {
        guard canStartNewGame else { return false }
        guard let url = Self.onlineServerURL(serverURLString) else {
            onlineJoinError = "No online server configured."
            return false
        }
        // A fresh attempt replaces any stale session.
        teardownOnlineSession()
        resetForOnlineGame()
        makeOnlineConnection(url: url)
        if create {
            online?.startCreating(timeControl: timeControl)
        } else if let code {
            online?.startJoining(code: code)
        }
        return true
    }

    /// The setup sheet is opening: reset the online-join handshake flags so a
    /// stale "ready" from a previous attempt cannot auto-dismiss this sheet
    /// (add-online-multiplayer D4). The join error is kept on purpose: it is
    /// still relevant to the player who is choosing again.
    func resetOnlineSheetState() {
        onlineSessionReady = false
    }

    /// App-relaunch recovery (add-online-multiplayer D8, E2E 5.2): when a room
    /// was persisted mid-game, re-attach to it on startup so the server's
    /// reconnect window can resume the game on this device.
    func restoreOnlineSessionIfNeeded(serverURLString: String?) {
        // A persisted in-progress room wins over the fresh local default game
        // (which always has zero moves at startup, design D8).
        guard moveList.isEmpty else { return }
        guard let code = UserDefaults.standard.string(forKey: Self.onlineRoomKey),
              !code.isEmpty
        else { return }
        guard let url = Self.onlineServerURL(serverURLString) else {
            // No server available: the persisted room can never be resumed.
            clearOnlineRoomPersistence()
            return
        }
        teardownOnlineSession()
        resetForOnlineGame()
        makeOnlineConnection(url: url)
        online?.startReattaching(code: code)
    }

    private static func onlineServerURL(_ string: String?) -> URL? {
        guard let trimmed = string?.trimmingCharacters(in: .whitespaces),
              !trimmed.isEmpty,
              let url = URL(string: trimmed)
        else { return nil }
        return url
    }

    /// Fresh local mirror for an online game: the server is authoritative, and
    /// this session only replays the move list to answer "is this legal / is
    /// check on" locally.
    private func resetForOnlineGame() {
        gameMode = .online
        cpuGeneration += 1
        cpuThinking = false
        gameEndPresented = false
        gameEndDismissed = false
        showGameEndDialog = false
        session = newGameSession(initialRating: 1500.0)
        toMove = "w"
        clearSelection()
        lastMove = nil
        moveList = []
        status = .starting
        errorMessage = nil
        onlineJoinError = nil
        onlineSessionReady = false
        onlineYourColor = nil
        onlineOpponentOnline = false
        onlineClock = nil
        onlineTimeControl = OnlineTimeControl.default.label
        do {
            board = try FenBoard(board: session.getBoardState(), sideToMove: toMove)
        } catch {
            board = try! FenBoard(board: FenBoard.startBoard, sideToMove: "w")
        }
    }

    private func makeOnlineConnection(url: URL) {
        let manager = OnlineConnectionManager(url: url, deviceID: Self.deviceID)
        manager.onSnapshot = { [weak self] state in
            self?.applyOnlineSnapshot(state)
        }
        manager.onUpdate = { [weak self] update in
            self?.applyOnlineUpdate(update)
        }
        manager.onError = { [weak self] errorCode, message in
            self?.handleOnlineError(code: errorCode, message: message)
        }
        manager.onPhase = { [weak self] phase in
            self?.handleOnlinePhase(phase)
        }
        manager.onAuthResult = { [weak self] success, payload in
            self?.handleAuthResult(success: success, payload: payload)
        }
        online = manager
    }

    // MARK: - Auth (add-auth-ui-clients)

    /// Log in with an existing account (task 2.3): the server answers
    /// `Session` (token) or `invalid_credentials` carrying the generic
    /// message. No-op while already signed in; guest play is unaffected.
    func doLogin(username: String, password: String, serverURLString: String?) {
        guard authToken == nil else { return }
        guard ensureAuthConnection(serverURLString: serverURLString) else {
            authError = "No online server configured."
            return
        }
        authError = nil
        pendingAuthAction = .login
        online?.startLogin(username: username, password: password)
    }

    /// Register a new account (task 2.1): the server answers `Session`
    /// (token) or `username_taken`. Same connection rules as `doLogin`.
    func doRegister(username: String, password: String, serverURLString: String?) {
        guard authToken == nil else { return }
        guard ensureAuthConnection(serverURLString: serverURLString) else {
            authError = "No online server configured."
            return
        }
        authError = nil
        pendingAuthAction = .register
        online?.startRegister(username: username, password: password)
    }

    /// Log out (task 3.1): sends the token, expects `session_ok`, and
    /// clears the token on success. The server only revokes the token —
    /// the connection stays open, so guest play continues on the same
    /// socket. A refusal (`session_expired`) keeps the token and shows the
    /// server's message in the sheet.
    func doLogout(serverURLString: String?) {
        guard let token = authToken else { return }
        guard ensureAuthConnection(serverURLString: serverURLString) else {
            authError = "No online server configured."
            return
        }
        authError = nil
        pendingAuthAction = .logout
        online?.startLogout(token: token)
    }

    /// Update the profile display name (task 3.2): validates locally with
    /// the server's rules (trim, 1...32 characters, no control characters),
    /// then sends `set_profile` and expects `profile_updated`. A refusal
    /// (`not_authenticated`, `invalid_display_name`) keeps the token and
    /// shows the server's message in the sheet.
    func doSetProfile(displayName: String, serverURLString: String?) {
        guard authToken != nil else { return }
        let trimmed = displayName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, trimmed.count <= 32 else {
            authError = "Display name must be 1–32 characters long."
            return
        }
        guard !trimmed.unicodeScalars.contains(where: {
            CharacterSet.controlCharacters.contains($0)
        }) else {
            authError = "Display name cannot contain control characters."
            return
        }
        guard ensureAuthConnection(serverURLString: serverURLString) else {
            authError = "No online server configured."
            return
        }
        authError = nil
        pendingAuthAction = .setProfile
        pendingDisplayName = trimmed
        online?.startSetProfile(displayName: trimmed)
    }

    /// Apply the reply of the auth action in flight: register/login store
    /// the issued token; logout clears it; any refusal keeps the connection
    /// open (guest play continues) and surfaces the server's message in
    /// the sheet.
    private func handleAuthResult(success: Bool, payload: String?) {
        switch pendingAuthAction {
        case .login, .register:
            if success {
                if let token = payload {
                    authToken = token
                }
                authError = nil
            } else {
                authError = payload ?? "Authentication failed"
            }
        case .logout:
            if success {
                authToken = nil
                authError = nil
            } else {
                authError = payload ?? "Logout failed"
            }
        case .setProfile:
            if success {
                authDisplayName = pendingDisplayName
                pendingDisplayName = nil
                authError = nil
            } else {
                pendingDisplayName = nil
                authError = payload ?? "Could not update profile"
            }
        }
    }

    /// A usable socket for authentication: reuse the open online connection
    /// when there is one, otherwise open a plain connection with no room.
    /// Game state is untouched either way: the local board, the guest
    /// `device_id`, and any persisted room for relaunch recovery all stay as
    /// they are (auth-ui spec "Existing clients remain unaffected").
    private func ensureAuthConnection(serverURLString: String?) -> Bool {
        if let manager = online {
            switch manager.phase {
            case .connecting, .waiting, .inGame, .reconnecting:
                // A live connection already carries the auth callbacks
                // (`makeOnlineConnection` wires them).
                return true
            case .idle, .failed:
                break
            }
        }
        guard let url = Self.onlineServerURL(serverURLString) else { return false }
        // Replace a dead connection. Room persistence is kept: it is
        // game-recovery bookkeeping, not auth state.
        online?.teardown()
        let manager = OnlineConnectionManager(url: url, deviceID: Self.deviceID)
        manager.onAuthResult = { [weak self] success, payload in
            self?.handleAuthResult(success: success, payload: payload)
        }
        manager.onError = { [weak self] _, message in
            // Auth-only connection: a drop or refusal is an auth problem,
            // and the game-level callbacks are deliberately not wired so a
            // failed sign-in can never mark the local game as failed.
            self?.authError = message
        }
        online = manager
        manager.startAuth()
        return true
    }

    /// Apply a server snapshot (add-online-multiplayer D2/D3): new moves are
    /// replayed into the mirror session, and the UI (board, move list, last
    /// move, status) is driven from the server's data.
    private func applyOnlineSnapshot(_ state: OnlineState) {
        let color = state.yourColor.sideToMoveSquare
        if onlineYourColor != color {
            // A new online game (or a re-attach): seat the player's own color
            // at the bottom so the board reads from their side. Later
            // snapshots leave a manual Flip in place.
            onlineYourColor = color
            boardOrientation = color == "b" ? 180 : 0
        }
        onlineOpponentOnline = state.opponentOnline
        onlineWhiteRating = state.whiteRating
        onlineBlackRating = state.blackRating
        onlineTimeControl = state.timeControl
        // The clock runs while the game is in progress; in the waiting lobby
        // (no opponent, no moves) the clocks show the base time, inactive.
        let clockRunning = state.status == .playing
            && (state.opponentOnline || !state.moveList.isEmpty)
        onlineClock = OnlineClock(
            whiteMs: state.whiteTimeMs,
            blackMs: state.blackTimeMs,
            sideToMove: state.sideToMove,
            reference: Date(),
            isRunning: clockRunning
        )
        if let code = online?.roomCode {
            onlineRoomCode = code
        }

        // Replay any new moves into the mirror session.
        if state.moveList.count > moveList.count {
            let fresh = state.moveList.dropFirst(moveList.count)
            do {
                for uci in fresh {
                    try session.playMove(uciMove: uci)
                }
            } catch {
                // The server confirmed a move the core rejects: desync.
                status = .failed("Lost sync with the server")
                errorMessage = "Lost sync with the server"
                return
            }
            moveList = state.moveList
            toMove = state.sideToMove
            if let lastUci = state.moveList.last {
                // 5-char promotion UCIs included: destination is chars 2-3.
                lastMove = LastMove(from: String(lastUci.prefix(2)),
                                    to: String(lastUci.dropFirst(2).prefix(2)))
            }
            clearSelection()
            errorMessage = nil
        }

        // Board: the snapshot carries the authoritative placement.
        if let rebuilt = try? FenBoard(board: state.boardFen, sideToMove: state.sideToMove) {
            board = rebuilt
        }

        switch state.status {
        case .playing:
            let inCheck = (try? session.isCheck()) ?? false
            status = .playing(toMove: state.sideToMove, inCheck: inCheck)
        case .checkmated(let winner):
            status = .checkmated(winner: winner.displayName)
        case .drawn:
            status = .drawn
        case .resigned(let winner):
            status = .resigned(winner: winner.displayName)
        case .forfeited(let winner):
            status = .forfeited(winner: winner.displayName)
        case .timedOut(let winner):
            status = .timedOut(winner: winner.displayName)
        }

        if state.status.isTerminal {
            // The game is over: the room will not be resumed on relaunch.
            clearOnlineRoomPersistence()
        }
        onlineSessionReady = true
        markTerminalIfNeeded()
    }

    /// Apply an incremental update (server "Game Authority"): replay the move
    /// into the mirror session and refresh the mutable fields, without the
    /// full snapshot's board FEN or move list.
    private func applyOnlineUpdate(_ update: OnlineUpdate) {
        onlineOpponentOnline = update.opponentOnline
        onlineWhiteRating = update.whiteRating
        onlineBlackRating = update.blackRating
        let clockRunning = update.status == .playing
            && (update.opponentOnline || !moveList.isEmpty)
        onlineClock = OnlineClock(
            whiteMs: update.whiteTimeMs,
            blackMs: update.blackTimeMs,
            sideToMove: update.sideToMove,
            reference: Date(),
            isRunning: clockRunning
        )

        if let uci = update.uci {
            do {
                try session.playMove(uciMove: uci)
            } catch {
                status = .failed("Lost sync with the server")
                errorMessage = "Lost sync with the server"
                return
            }
            moveList.append(uci)
            lastMove = LastMove(from: String(uci.prefix(2)),
                                to: String(uci.dropFirst(2).prefix(2)))
            clearSelection()
            errorMessage = nil
        }

        toMove = update.sideToMove
        if let rebuilt = try? FenBoard(board: session.getBoardState(), sideToMove: update.sideToMove) {
            board = rebuilt
        }
        switch update.status {
        case .playing:
            let inCheck = (try? session.isCheck()) ?? false
            status = .playing(toMove: update.sideToMove, inCheck: inCheck)
        case .checkmated(let winner):
            status = .checkmated(winner: winner.displayName)
        case .drawn:
            status = .drawn
        case .resigned(let winner):
            status = .resigned(winner: winner.displayName)
        case .forfeited(let winner):
            status = .forfeited(winner: winner.displayName)
        case .timedOut(let winner):
            status = .timedOut(winner: winner.displayName)
        }
        if update.status.isTerminal {
            clearOnlineRoomPersistence()
        }
        onlineSessionReady = true
        markTerminalIfNeeded()
    }

    /// Structured server errors (add-online-multiplayer D5): room-level errors
    /// land on the join screen; in-game rejections keep the server position
    /// and clear the selection.
    private func handleOnlineError(code: OnlineErrorCode, message: String) {
        switch code {
        case .roomNotFound, .roomFull, .invalidRoomCode, .alreadyInRoom:
            // The sheet (if open) shows this; it stays open so the player can
            // fix the code or create a room instead.
            onlineJoinError = message
        case .notYourTurn, .illegalMove:
            errorMessage = message
            clearSelection()
        default:
            errorMessage = message
        }
    }

    /// Mirror the connection phase into the view model's own published state
    /// and maintain the persisted-room bookkeeping.
    private func handleOnlinePhase(_ phase: OnlinePhase) {
        onlinePhase = phase
        onlineReconnecting = (phase == .reconnecting)
        if let code = online?.roomCode {
            onlineRoomCode = code
        }
        switch phase {
        case .waiting(let code):
            // A seat was confirmed: remember the room for relaunch recovery.
            persistOnlineRoom(code)
        case .inGame:
            if let code = online?.roomCode {
                persistOnlineRoom(code)
            }
        case .failed:
            onlineSessionReady = false
            switch status {
            case .playing:
                status = .failed("Connection to the online server was lost")
                errorMessage = "Connection lost"
            case .starting:
                errorMessage = "Connection lost"
            default:
                break
            }
        default:
            break
        }
    }

    /// Close and forget the online session (add-online-multiplayer D7/D8).
    private func teardownOnlineSession() {
        online?.teardown()
        online = nil
        onlinePhase = .idle
        onlineReconnecting = false
        onlineRoomCode = nil
        onlineYourColor = nil
        onlineJoinError = nil
        onlineSessionReady = false
        onlineOpponentOnline = false
        onlineClock = nil
        clearOnlineRoomPersistence()
    }

    private func persistOnlineRoom(_ code: String) {
        UserDefaults.standard.set(code, forKey: Self.onlineRoomKey)
    }

    private func clearOnlineRoomPersistence() {
        UserDefaults.standard.removeObject(forKey: Self.onlineRoomKey)
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
        if isOnlineMode {
            // The server is authoritative (add-online-multiplayer D4/D7): send
            // the move and wait for the confirming snapshot; the board only
            // changes when the server confirms. A rejected move answers with a
            // structured error and the position stays as it is.
            clearSelection()
            online?.sendMove(uci)
            return
        }
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
        case .checkmated, .drawn, .resigned, .forfeited, .timedOut:
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
            } else if token == "onlinecreate" {
                // add-online-multiplayer: start an online game as the room
                // creator (White) against the -PLAINTCHESS_ONLINE_URL server.
                _ = self.startOnlineGame(create: true,
                                         serverURLString: self.debugOnlineURL)
            } else if token.hasPrefix("onlinecreate:") {
                // `onlinecreate:3+2`: create a room at that time control.
                let control = String(token.dropFirst("onlinecreate:".count))
                _ = self.startOnlineGame(create: true, timeControl: control,
                                         serverURLString: self.debugOnlineURL)
            } else if token.hasPrefix("onlinejoin:") {
                // `onlinejoin:AB23CD`: join that room as Black.
                let code = String(token.dropFirst("onlinejoin:".count))
                _ = self.startOnlineGame(create: false, code: code,
                                         serverURLString: self.debugOnlineURL)
            } else if token.hasPrefix("forcemove:") {
                // E2E rejection check (task 2.4): send a raw UCI to the server
                // bypassing the turn gating, to exercise the structured-error
                // path (not_your_turn / illegal_move → message + selection
                // cleared, server position kept).
                let uci = String(token.dropFirst("forcemove:".count))
                self.online?.sendMove(uci)
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
                let confirmed = self.moveList.contains(token)
                if self.isOnlineMode, !confirmed, let color = self.onlineYourColor {
                    // The script lists both sides' moves in game order. Skip a
                    // token that belongs to the opponent (ply parity vs the
                    // color the server assigned this device), so the run no
                    // longer assumes the creator is White; our own move is
                    // attempted until the server confirms it.
                    let plyIsWhite = self.moveList.count.isMultiple(of: 2)
                    if plyIsWhite != (color == "w") {
                        self.playScriptNow(Array(moves.dropFirst()), interval: interval)
                        return
                    }
                }
                // Drive the same intent path as a real user: select the
                // origin, then the destination. An illegal destination
                // exercises the "not a legal move" rejection path. A
                // 5-character token (e.g. "a2a1q") additionally confirms the
                // promotion through the same intent the picker button calls;
                // a 4-char token landing on a promotion destination stops
                // with the picker open (screenshot-able, design D5).
                let from = String(token.prefix(2))
                let to = String(token.dropFirst(2).prefix(2))
                // Online: the move is only "confirmed" when the server's
                // snapshot lands (moveList gains this exact UCI). Attempt it
                // while unconfirmed and retry the same token on later ticks
                // (turn gating inside `select` makes early attempts no-ops);
                // advance only once confirmed. Local games apply the move
                // synchronously, so they keep advancing unconditionally.
                // A UCI move can never repeat within one game, so "present in
                // the list" is a safe confirmation. (`.last` is wrong: by the
                // next tick the opponent has usually already replied, so the
                // last entry is their move, not ours.)
                if !confirmed {
                    self.select(from)
                    self.select(to)
                    if token.count == 5 {
                        self.confirmPromotion(String(token.suffix(1)))
                    }
                }
                let advance = confirmed || !self.isOnlineMode
                self.playScriptNow(advance ? Array(moves.dropFirst()) : moves,
                                   interval: interval)
                return
            }
            self.playScriptNow(Array(moves.dropFirst()), interval: interval)
        }
    }
#endif
}
