package com.hectoralejandrg.plaintextchess

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import java.util.UUID
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.chess_core.ChessException
import uniffi.chess_core.GameSession
import uniffi.chess_core.newGameSession

/** Milestone-1 game status (design D6: pulled from the core after each move). */
sealed class GameStatus {
    object Starting : GameStatus()
    data class Playing(val toMove: String, val inCheck: Boolean) : GameStatus()
    data class Checkmated(val winner: String) : GameStatus()
    object Drawn : GameStatus()
    /** App-level terminal state (design D4 of add-game-end-dialog): the
     * resigner is the losing side, `winner` the opponent. */
    data class Resigned(val winner: String) : GameStatus()
    /** Online: the opponent's reconnect window expired and the server
     * forfeited the absent player (add-online-multiplayer D8). */
    data class Forfeited(val winner: String) : GameStatus()
    /** Online: a player's clock ran out (flag fall, add-online-time-controls);
     * `winner` is the opponent's color. */
    data class TimedOut(val winner: String) : GameStatus()
    data class Failed(val message: String) : GameStatus()
}

/**
 * The online clock as reported by the latest server snapshot
 * (add-online-time-controls, design D6). [whiteMs]/[blackMs] are the
 * snapshot's authoritative remaining times captured at [referenceMillis];
 * the UI derives the live display value with [remainingMs], which only
 * interpolates the side-to-move countdown (the waiting side is frozen).
 */
data class OnlineClock(
    val whiteMs: Int,
    val blackMs: Int,
    val sideToMove: String,
    val referenceMillis: Long,
    val isRunning: Boolean,
) {
    fun remainingMs(color: String, nowMillis: Long): Int {
        val base = if (color == "w") whiteMs else blackMs
        if (!isRunning || color != sideToMove) return maxOf(0, base)
        val elapsed = (nowMillis - referenceMillis).toInt()
        return maxOf(0, base - elapsed)
    }

    companion object {
        /** `m:ss`. */
        fun format(milliseconds: Int): String {
            val seconds = maxOf(0, milliseconds) / 1000
            return "%d:%02d".format(seconds / 60, seconds % 60)
        }
    }
}

/**
 * Opponent mode for a game (design D4/D6): two human players, a human
 * (White) versus the CPU (Black) at a chosen difficulty, or an online match
 * whose seat color is assigned by the server (add-online-multiplayer D4).
 */
sealed class GameMode {
    object TwoPlayers : GameMode()
    data class Cpu(val difficulty: CpuDifficulty) : GameMode()
    object Online : GameMode()
}

/** CPU difficulty levels, matching the core's search depth (design D2). */
enum class CpuDifficulty(val rawValue: Int, val displayName: String) {
    EASY(1, "Easy"),
    MEDIUM(2, "Medium"),
    HARD(3, "Hard");

    companion object {
        fun fromName(name: String): CpuDifficulty =
            values().firstOrNull { it.name.equals(name, ignoreCase = true) } ?: MEDIUM
    }
}

/**
 * Single source of truth for the Android game screen (design D4).
 *
 * Mirrors the iOS `GameViewModel`: all chess rules live in the Rust core
 * through the UniFFI `GameSession`; this class only renders state and
 * forwards user intents. Held with `remember` in the composable.
 */
class GameViewModel(private val context: Context) {

    data class LastMove(val from: String, val to: String)

    /**
     * A promotion awaiting the player's choice (design D1): the core reports
     * a last-rank destination with one UCI move per promotion piece.
     */
    data class PendingPromotion(val from: String, val to: String, val options: List<String>)

    var board by mutableStateOf(FenBoard.start)
        private set
    var status by mutableStateOf<GameStatus>(GameStatus.Starting)
        private set
    var selectedSquare by mutableStateOf<String?>(null)
        private set
    var legalTargets by mutableStateOf<List<String>>(emptyList())
        private set
    var lastMove by mutableStateOf<LastMove?>(null)
        private set
    var moveList by mutableStateOf<List<String>>(emptyList())
        private set
    var errorMessage by mutableStateOf<String?>(null)
        private set
    var pendingPromotion by mutableStateOf<PendingPromotion?>(null)
        private set
    /** True while the CPU computes its reply (design D5): board input is
     * locked and the status row shows "CPU is thinking…". */
    var cpuThinking by mutableStateOf(false)
        private set

    /** Game-end dialog visibility (design D2 of add-game-end-dialog): the VM
     * owns the presentation so the "appears once" rule has no view-layer race. */
    var showGameEndDialog by mutableStateOf(false)
        private set

    /** Board orientation (design D6 of add-game-end-dialog): 0 = White on the
     * bottom, 180 = board rotated. A display preference: it persists across
     * new games in the session and is never reset by [startGame]. */
    var boardOrientation by mutableStateOf(0)
        private set

    // region Online multiplayer (add-online-multiplayer)

    /** Connection phase mirrored from the [OnlineConnectionManager] (the VM
     * owns the observed state; the manager drives it). */
    var onlinePhase by mutableStateOf<OnlinePhase>(OnlinePhase.Idle)
        private set

    /** True while the socket is dropped and the manager re-attaches; the UI
     * shows a banner and the last known position stays on screen. */
    var onlineReconnecting by mutableStateOf(false)
        private set

    /** The room code of the current online room (shown with a copy action in
     * the waiting state). */
    var onlineRoomCode by mutableStateOf<String?>(null)
        private set

    /** Our color in the online game ("w"/"b"), from the server snapshot. */
    var onlineYourColor by mutableStateOf<String?>(null)
        private set

    /** True once the server confirmed the seat (`room_ready` received): the
     * setup sheet dismisses itself. */
    var onlineSessionReady by mutableStateOf(false)
        private set

    /** A join-time error, shown on the join screen while the sheet stays open. */
    var onlineJoinError by mutableStateOf<String?>(null)
        private set

    /** Whether the opponent is currently connected (the lobby gate). */
    var onlineOpponentOnline by mutableStateOf(false)
        private set

    /** Ratings from the last snapshot, by color (add-online-multiplayer D5).
     * Shown in the terminal-state modal so both clients see the updated
     * ratings in the final state (design D11, step 4). */
    var onlineWhiteRating by mutableStateOf(1500.0)
        private set
    var onlineBlackRating by mutableStateOf(1500.0)
        private set

    /** The server's clock as of the last snapshot (add-online-time-controls,
     * design D6): the snapshot times are authoritative; the UI interpolates
     * the side-to-move countdown between snapshots. */
    var onlineClock by mutableStateOf<OnlineClock?>(null)
        private set

    /** The room's time control label from the last snapshot (lobby included). */
    var onlineTimeControl by mutableStateOf(OnlineTimeControl.default.label)
        private set

    private var online: OnlineConnectionManager? = null

    // endregion

    /**
     * DEBUG-only hook (add-online-multiplayer D9): the online server URL from
     * the `online_url` intent extra; [MainActivity] only passes it in
     * debuggable builds, so it is release-inert.
     */
    var debugOnlineURL: String? = null

    private var session: GameSession = newGameSession(initialRating = 1500.0)

    /** Side to move, tracked locally (the core's board state is position-only). */
    private var toMove: String = "w"

    /**
     * Full UCI moves of the currently selected piece (design D1): the
     * 5-character ones are the promotion options.
     */
    private var selectedMoves: List<String> = emptyList()

    /** Opponent mode for the current game (design D4/D6). */
    private var gameMode: GameMode = GameMode.TwoPlayers

    /** The side the CPU plays (design D6: the CPU is always Black). */
    private val cpuColor = "b"

    /** True when the current game is online (the UI switches to the online
     * wording: "Your move", You won/lost modal, no Play again, no Undo). */
    val isOnlineMode: Boolean
        get() = gameMode is GameMode.Online

    /** Bumped on every [startGame] so an in-flight CPU move from an older game
     * is discarded instead of applied (design D5). */
    private var cpuGeneration = 0

    /** Game-end dialog bookkeeping (design D2 of add-game-end-dialog): the
     * modal is presented once per game and never re-presented after the
     * player dismisses it; [startGame] resets both flags. */
    private var gameEndPresented = false
    private var gameEndDismissed = false

    /**
     * DEBUG-only hook (enforce-single-active-game D6): milliseconds to hold
     * the "CPU is thinking…" state before applying the computed move, so
     * Resign/Undo can deterministically land "while the CPU is thinking" on
     * the emulator (hard replies otherwise land in <80 ms). Mirrors the iOS
     * `-PLAINTCHESS_CPU_DELAY` launch argument: `MainActivity` only passes a
     * non-zero value in debuggable builds, so it is release-inert.
     */
    var debugCpuDelayMs: Long = 0L

    /**
     * VM-scoped coroutine context standing in for `viewModelScope` (the view
     * model is held with `remember`, not an AndroidX `ViewModel`): launches on
     * the main dispatcher; the CPU compute hops to [Dispatchers.Default].
     */
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    init {
        try {
            board = FenBoard.parse(session.getBoardState(), toMove)
            refreshStatus()
        } catch (e: Throwable) {
            board = FenBoard.start
            status = GameStatus.Failed(describe(e))
            errorMessage = "FFI error: ${describe(e)}"
        }
    }

    /**
     * Two-tap selection loop (design D3): select an own piece, then tap a
     * highlighted destination to play. Tapping a non-legal square keeps the
     * selection and shows "not a legal move" feedback.
     */
    fun select(squareName: String) {
        // Locked while the CPU is thinking (design D5): no selection, no move.
        if (cpuThinking) return
        val current = status
        if (current !is GameStatus.Playing) return
        // Online: input is only accepted while the game is in progress and on
        // our own turn (add-online-multiplayer D4). The board stays inert in
        // the lobby (opponent not seated, no move played) and while the
        // opponent is to move; a temporarily disconnected opponent does not
        // lock the board, since the server still accepts our moves.
        if (gameMode is GameMode.Online &&
            (current.toMove != onlineYourColor || (!onlineOpponentOnline && moveList.isEmpty()))
        ) return
        val square = FenBoard.parseSquare(squareName) ?: return
        val (row, col) = square
        val piece = board.grid[row][col]

        val selected = selectedSquare
        if (selected != null) {
            if (selected == squareName) {
                clearSelection()
                return
            }
            if (legalTargets.contains(squareName)) {
                // D1: a last-rank destination with promotion UCI options opens
                // the picker instead of playing.
                val pending = pendingPromotionFor(selected, squareName)
                if (pending != null) {
                    pendingPromotion = pending
                    return
                }
                playMove(selected, squareName, selected + squareName)
                return
            }
        }

        if (piece.isEmpty()) {
            if (selectedSquare != null) errorMessage = "Not a legal move"
            return
        }

        if (isOwn(piece, current.toMove)) {
            selectedSquare = squareName
            // The core returns full UCI strings ("e2e4", "a2b1q", ...). Keep
            // the full strings (promotion detection, D1) and derive the
            // destination squares for highlighting and tap matching
            // (destination is chars 2-3; 5-char promotion UCIs end in the
            // promotion piece, so `takeLast(2)` is wrong for those).
            val uciMoves = try {
                session.getValidMoves(square = squareName)
            } catch (e: Throwable) {
                emptyList()
            }
            selectedMoves = uciMoves.filter { it.startsWith(squareName) }
            legalTargets = selectedMoves.map { it.drop(2).take(2) }
            errorMessage = null
        } else {
            // Enemy piece that is not a legal capture target.
            if (selectedSquare != null) errorMessage = "Not a legal move"
        }
    }

    /**
     * Play the chosen promotion piece (design D1). `piece` is one of q/r/b/n
     * as listed in `pendingPromotion.options`.
     */
    fun confirmPromotion(piece: String) {
        val pending = pendingPromotion ?: return
        pendingPromotion = null
        playMove(pending.from, pending.to, pending.from + pending.to + piece)
    }

    /** Dismiss an open promotion choice without playing (design D2). */
    fun cancelPromotion() {
        pendingPromotion = null
        clearSelection()
    }

    /**
     * Whether the piece on this square may be picked up right now. Used by
     * the drag gesture; the tap path answers the same question inside
     * [select].
     */
    fun canPickup(squareName: String): Boolean {
        // Locked while the CPU is thinking (design D5).
        if (cpuThinking) return false
        val current = status
        if (current !is GameStatus.Playing) return false
        // Same online gate as [select] (add-online-multiplayer D4): the drag
        // gesture asks the same question the tap path answers.
        if (gameMode is GameMode.Online &&
            (current.toMove != onlineYourColor || (!onlineOpponentOnline && moveList.isEmpty()))
        ) return false
        val square = FenBoard.parseSquare(squareName) ?: return false
        val (row, col) = square
        val piece = board.grid[row][col]
        return piece.isNotEmpty() && isOwn(piece, current.toMove)
    }

    /**
     * Whether the "New game" action may be started right now (spec:
     * enforce-single-active-game "Game Mode Selection"): a new game may be
     * started only when the current game is finished (checkmate, draw,
     * resignation), when no move of it has been played yet, or when it is in
     * an error state (recovery). While a game with at least one played move
     * is in progress, New game is unavailable. Single source of truth: the
     * UI button binds to it and [startGame] guards on it (design D1);
     * [restart] needs no special-casing because terminal ⇒ gate open (D2).
     */
    val canStartNewGame: Boolean
        get() = when (status) {
            is GameStatus.Checkmated, is GameStatus.Drawn,
            is GameStatus.Resigned, is GameStatus.Forfeited,
            is GameStatus.TimedOut,
            is GameStatus.Failed -> true
            else -> moveList.isEmpty()
        }

    /**
     * Discard the current session and start a fresh game in [mode]
     * (spec: new-game action; design D4/D5/D6).
     */
    fun startGame(mode: GameMode) {
        // Single-active-game rule (enforce-single-active-game D1): never
        // replace a game that is still in progress.
        if (!canStartNewGame) return
        // A local game replaces any open online session
        // (add-online-multiplayer D7/D8): close the socket and forget the
        // persisted room.
        if (isOnlineMode) {
            teardownOnlineSession()
        }
        gameMode = mode
        cpuGeneration += 1
        cpuThinking = false
        // Game-end dialog: a fresh game gets a fresh dialog (design D2).
        gameEndPresented = false
        gameEndDismissed = false
        showGameEndDialog = false
        session = newGameSession(initialRating = 1500.0)
        toMove = "w"
        clearSelection()
        lastMove = null
        moveList = emptyList()
        try {
            board = FenBoard.parse(session.getBoardState(), toMove)
            refreshStatus()
        } catch (e: Throwable) {
            board = FenBoard.start
            status = GameStatus.Failed(describe(e))
            errorMessage = "FFI error: ${describe(e)}"
            return
        }
        triggerCpuMoveIfNeeded()
    }

    /** Start a two-player game (design D4 convenience). */
    fun newGame() {
        startGame(GameMode.TwoPlayers)
    }

    // region Game-end dialog (design D2/D3 of add-game-end-dialog)

    /**
     * The result message for the game-end modal (design D3), or `null` while
     * the game is not terminal. Computed from [status] + [gameMode] so it can
     * never desynchronize from the status.
     */
    val gameEndMessage: String?
        get() = when (val s = status) {
            is GameStatus.Checkmated ->
                if (gameMode is GameMode.Online)
                    if (wonOnline(s.winner)) "You won.\n$onlineRatingsLine"
                    else "You lost.\n$onlineRatingsLine"
                else if (gameMode is GameMode.Cpu)
                    if (s.winner == "White") "Checkmate! You won!"
                    else "Checkmate! You lost."
                else "Checkmate! ${s.winner} wins."
            is GameStatus.Resigned ->
                if (gameMode is GameMode.Online)
                    if (wonOnline(s.winner)) "You won.\n$onlineRatingsLine"
                    else "You lost.\n$onlineRatingsLine"
                else if (gameMode is GameMode.Cpu) {
                    // Only the human resigns against the CPU (the human is
                    // White, design D6 of add-cpu-opponent).
                    "You resigned. You lost."
                } else {
                    val loser = if (s.winner == "White") "Black" else "White"
                    "$loser resigns. ${s.winner} wins."
                }
            is GameStatus.Forfeited ->
                // Online only (add-online-multiplayer D4): the absent player
                // lost when the server's reconnect window closed.
                if (wonOnline(s.winner)) "You won.\n$onlineRatingsLine"
                else "You lost.\n$onlineRatingsLine"
            is GameStatus.TimedOut ->
                // Online only (add-online-time-controls D4): a flag fall is
                // stated as a win/loss on time.
                if (wonOnline(s.winner)) "You won on time.\n$onlineRatingsLine"
                else "You lost on time.\n$onlineRatingsLine"
            is GameStatus.Drawn ->
                if (gameMode is GameMode.Online) "Draw.\n$onlineRatingsLine"
                else "The game is drawn."
            else -> null
        }

    /** "Play again" in the game-end dialog (design D2): a fresh game in the
     * current mode (and CPU difficulty). */
    fun restart() {
        // Online games have no "Play again" (a rematch would need a new room):
        // the modal offers only "Done" there, and a stray call must not start
        // one.
        if (isOnlineMode) {
            dismissGameEnd()
            return
        }
        startGame(gameMode)
    }

    /** "Done" in the game-end dialog (design D2): close the modal without
     * changing the game. The status line keeps showing the result. */
    fun dismissGameEnd() {
        gameEndDismissed = true
        showGameEndDialog = false
        // Online: release the seat so this device is free for a new room
        // (add-online-multiplayer D4).
        if (isOnlineMode) {
            online?.leave()
        }
    }

    /** Online perspective (add-online-multiplayer D4): does the named winner
     * color match the seat this device holds? */
    private fun wonOnline(winner: String): Boolean =
        (if (winner == "White") "w" else "b") == onlineYourColor

    /** The ratings footer for the online game-end modal (design D11, step 4):
     * our rating first, the opponent's second, rounded to whole points. */
    private val onlineRatingsLine: String
        get() {
            val mine = if (onlineYourColor == "w") onlineWhiteRating else onlineBlackRating
            val theirs = if (onlineYourColor == "w") onlineBlackRating else onlineWhiteRating
            return "Your rating: ${Math.round(mine)} · Opponent: ${Math.round(theirs)}"
        }

    // endregion

    // region Resign / undo / flip (design D4/D5/D6 of add-game-end-dialog)

    /** Whether the game can be resigned or undone right now. */
    val canResign: Boolean
        get() {
            if (status !is GameStatus.Playing) return false
            // Online: only once the game has actually started (the opponent is
            // seated) or a move was played — the lobby answers `not_connected`.
            if (isOnlineMode && !onlineOpponentOnline && moveList.isEmpty()) {
                return false
            }
            return true
        }

    val canUndo: Boolean
        get() {
            if (moveList.isEmpty() || status !is GameStatus.Playing) return false
            // Online: the server owns the move list, so there is nothing to
            // take back locally (add-online-multiplayer D4).
            if (isOnlineMode) return false
            return true
        }

    /**
     * Resign the current game (design D4): an app-level terminal state. In a
     * two-player game the side to move resigns; against the CPU the human
     * (White) resigns. Bumping the generation discards an in-flight CPU move
     * (same mechanism as [startGame], design D5 of add-cpu-opponent).
     */
    fun resign() {
        val current = status
        if (current !is GameStatus.Playing) return
        clearSelection()
        // Online: the server records the resignation and confirms it with a
        // terminal snapshot (add-online-multiplayer D4); nothing changes
        // locally until that snapshot arrives.
        if (isOnlineMode) {
            online?.resign()
            return
        }
        cpuGeneration += 1
        cpuThinking = false
        lastMove = null
        val winner = when (gameMode) {
            is GameMode.TwoPlayers -> if (toMove == "w") "Black" else "White"
            is GameMode.Cpu -> "Black"
            is GameMode.Online -> "Black" // unreachable: the online branch returns above
        }
        status = GameStatus.Resigned(winner)
        errorMessage = null
        markTerminalIfNeeded()
    }

    /**
     * Take back the last move (design D5): the core has no undo, so a fresh
     * session is replayed with the kept UCI moves. Two players: one ply; CPU:
     * the last pair so it is the human's turn again (one ply when the CPU is
     * still thinking its reply, which the generation bump discards).
     */
    fun undo() {
        if (!canUndo) return
        val kept =
            if (gameMode is GameMode.Cpu && moveList.size % 2 == 0) moveList.drop(2)
            else moveList.dropLast(1)
        // Discard any in-flight CPU move (design D5 generation guard).
        cpuGeneration += 1
        cpuThinking = false
        try {
            val rebuilt = newGameSession(initialRating = 1500.0)
            var side = "w"
            for (uci in kept) {
                rebuilt.playMove(uciMove = uci)
                side = if (side == "w") "b" else "w"
            }
            // Replay succeeded: commit the rebuilt session and bookkeeping.
            // (On failure nothing above touched the live state.)
            session = rebuilt
            toMove = side
            moveList = kept
            clearSelection()
            val last = kept.lastOrNull()
            lastMove = if (last != null) LastMove(last.take(2), last.drop(2).take(2)) else null
            board = FenBoard.parse(rebuilt.getBoardState(), toMove)
            refreshStatus()
        } catch (e: Throwable) {
            errorMessage = "Undo failed: ${describe(e)}"
        }
    }

    /**
     * Flip the board orientation (design D6): 0 = White on the bottom,
     * 180 = rotated. Display-only: the game state is untouched.
     */
    fun flipBoard() {
        boardOrientation = if (boardOrientation == 0) 180 else 0
    }

    // endregion

    // region Online multiplayer (add-online-multiplayer D2/D4/D7/D8)

    /**
     * Connect and start an online game: create a fresh room (the caller takes
     * the White seat) or join an existing one by code. The server is
     * authoritative: the seat, the color, and every move come from its
     * snapshots.
     */
    fun startOnlineGame(create: Boolean, code: String? = null,
                        timeControl: String? = null): Boolean {
        if (!canStartNewGame) return false
        val url = resolvedServerURL() ?: run {
            onlineJoinError = "No online server configured."
            return false
        }
        // A fresh attempt replaces any stale session.
        teardownOnlineSession()
        resetForOnlineGame()
        makeOnlineConnection(url)
        if (create) {
            online?.startCreating(timeControl)
        } else {
            code?.let { online?.startJoining(it) }
        }
        return true
    }

    /** The setup sheet is opening: reset the online-join handshake flags so a
     * stale "ready" from a previous attempt cannot auto-dismiss this sheet
     * (add-online-multiplayer D4). The join error is kept on purpose: it is
     * still relevant to the player who is choosing again. */
    fun resetOnlineSheetState() {
        onlineSessionReady = false
    }

    /** App-relaunch recovery (add-online-multiplayer D8, E2E 5.2): when a
     * room was persisted mid-game, re-attach to it on startup so the server's
     * reconnect window can resume the game on this device. */
    fun restoreOnlineSessionIfNeeded() {
        // A persisted in-progress room wins over the fresh local default game
        // (which always has zero moves at startup, design D8).
        if (moveList.isNotEmpty()) return
        val code = GameViewModel.persistedRoomCode(context)
        if (code.isNullOrEmpty()) return
        val url = resolvedServerURL() ?: run {
            // No server available: the persisted room can never be resumed.
            clearOnlineRoomPersistence()
            return
        }
        teardownOnlineSession()
        resetForOnlineGame()
        makeOnlineConnection(url)
        online?.startReattaching(code)
    }

    /** Resolve the online server URL (add-online-multiplayer D9): the DEBUG
     * override wins over the build-time default. */
    private fun resolvedServerURL(): String? =
        (debugOnlineURL ?: OnlineConfig.DEFAULT_URL).trim().takeIf { it.isNotEmpty() }

    /** Fresh local mirror for an online game: the server is authoritative,
     * and this session only replays the move list to answer "is this legal /
     * is check on" locally. */
    private fun resetForOnlineGame() {
        gameMode = GameMode.Online
        cpuGeneration += 1
        cpuThinking = false
        gameEndPresented = false
        gameEndDismissed = false
        showGameEndDialog = false
        session = newGameSession(initialRating = 1500.0)
        toMove = "w"
        clearSelection()
        lastMove = null
        moveList = emptyList()
        status = GameStatus.Starting
        errorMessage = null
        onlineJoinError = null
        onlineSessionReady = false
        onlineYourColor = null
        onlineOpponentOnline = false
        onlineClock = null
        onlineTimeControl = OnlineTimeControl.default.label
        runCatching {
            board = FenBoard.parse(session.getBoardState(), toMove)
        }
    }

    private fun makeOnlineConnection(url: String) {
        val manager = OnlineConnectionManager(url, GameViewModel.deviceID(context))
        manager.onSnapshot = { state -> applyOnlineSnapshot(state) }
        manager.onError = { code, message -> handleOnlineError(code, message) }
        manager.onPhase = { phase -> handleOnlinePhase(phase) }
        online = manager
    }

    /** Apply a server snapshot (add-online-multiplayer D2/D3): new moves are
     * replayed into the mirror session, and the UI (board, move list, last
     * move, status) is driven from the server's data. */
    private fun applyOnlineSnapshot(state: OnlineState) {
        onlineYourColor = state.yourColor.sideToMove
        onlineOpponentOnline = state.opponentOnline
        onlineWhiteRating = state.whiteRating
        onlineBlackRating = state.blackRating
        onlineRoomCode = online?.roomCode
        onlineTimeControl = state.timeControl
        // The clock runs while the game is in progress; in the waiting lobby
        // (no opponent, no moves) the clocks show the base time, inactive.
        val clockRunning = state.status == OnlineStatus.Playing &&
            (state.opponentOnline || state.moveList.isNotEmpty())
        onlineClock = OnlineClock(
            whiteMs = state.whiteTimeMs,
            blackMs = state.blackTimeMs,
            sideToMove = state.sideToMove,
            referenceMillis = System.currentTimeMillis(),
            isRunning = clockRunning,
        )

        // Replay any new moves into the mirror session.
        if (state.moveList.size > moveList.size) {
            val fresh = state.moveList.drop(moveList.size)
            val replayed = runCatching {
                for (uci in fresh) session.playMove(uciMove = uci)
            }
            if (replayed.isFailure) {
                // The server confirmed a move the core rejects: desync.
                status = GameStatus.Failed("Lost sync with the server")
                errorMessage = "Lost sync with the server"
                return
            }
            moveList = state.moveList
            toMove = state.sideToMove
            state.moveList.lastOrNull()?.let { lastUci ->
                // 5-char promotion UCIs included: destination is chars 2-3.
                lastMove = LastMove(lastUci.take(2), lastUci.drop(2).take(2))
            }
            clearSelection()
            errorMessage = null
        }

        // Board: the snapshot carries the authoritative placement.
        runCatching {
            board = FenBoard.parse(state.boardFen, state.sideToMove)
        }

        status = when (val s = state.status) {
            is OnlineStatus.Playing -> {
                val inCheck = runCatching { session.isCheck() }.getOrDefault(false)
                GameStatus.Playing(state.sideToMove, inCheck)
            }
            is OnlineStatus.Checkmated -> GameStatus.Checkmated(s.winnerColor.displayName)
            is OnlineStatus.Drawn -> GameStatus.Drawn
            is OnlineStatus.Resigned -> GameStatus.Resigned(s.winnerColor.displayName)
            is OnlineStatus.Forfeited -> GameStatus.Forfeited(s.winnerColor.displayName)
            is OnlineStatus.TimedOut -> GameStatus.TimedOut(s.winnerColor.displayName)
        }

        if (state.status.isTerminal) {
            // The game is over: the room will not be resumed on relaunch.
            clearOnlineRoomPersistence()
        }
        onlineSessionReady = true
        markTerminalIfNeeded()
    }

    /** Structured server errors (add-online-multiplayer D5): room-level
     * errors land on the join screen; in-game rejections keep the server
     * position and clear the selection. */
    private fun handleOnlineError(code: OnlineErrorCode?, message: String) {
        when (code) {
            OnlineErrorCode.ROOM_NOT_FOUND,
            OnlineErrorCode.ROOM_FULL,
            OnlineErrorCode.INVALID_ROOM_CODE,
            OnlineErrorCode.ALREADY_IN_ROOM ->
                // The sheet (if open) shows this; it stays open so the player
                // can fix the code or create a room instead.
                onlineJoinError = message

            OnlineErrorCode.NOT_YOUR_TURN,
            OnlineErrorCode.ILLEGAL_MOVE -> {
                errorMessage = message
                clearSelection()
            }

            else -> errorMessage = message
        }
    }

    /** Mirror the connection phase into the view model's own observed state
     * and maintain the persisted-room bookkeeping. */
    private fun handleOnlinePhase(phase: OnlinePhase) {
        onlinePhase = phase
        onlineReconnecting = phase is OnlinePhase.Reconnecting
        onlineRoomCode = online?.roomCode
        when (phase) {
            is OnlinePhase.Waiting ->
                // A seat was confirmed: remember the room for relaunch recovery.
                persistOnlineRoom(phase.code)

            is OnlinePhase.InGame ->
                online?.roomCode?.let { persistOnlineRoom(it) }

            is OnlinePhase.Failed -> {
                onlineSessionReady = false
                when (status) {
                    is GameStatus.Playing -> {
                        status = GameStatus.Failed("Connection to the online server was lost")
                        errorMessage = "Connection lost"
                    }
                    is GameStatus.Starting -> errorMessage = "Connection lost"
                    else -> {}
                }
            }

            else -> {}
        }
    }

    /** Close and forget the online session (add-online-multiplayer D7/D8). */
    private fun teardownOnlineSession() {
        online?.teardown()
        online = null
        onlinePhase = OnlinePhase.Idle
        onlineReconnecting = false
        onlineRoomCode = null
        onlineYourColor = null
        onlineJoinError = null
        onlineSessionReady = false
        onlineOpponentOnline = false
        onlineClock = null
        clearOnlineRoomPersistence()
    }

    private fun persistOnlineRoom(code: String) {
        GameViewModel.persistRoomCode(context, code)
    }

    private fun clearOnlineRoomPersistence() {
        GameViewModel.clearPersistedRoom(context)
    }

    // endregion

    companion object {
        private const val PREFS_NAME = "plaintextchess"
        private const val KEY_DEVICE_ID = "plaintextchess.device_id"
        private const val KEY_ONLINE_ROOM_CODE = "plaintextchess.online_room_code"

        /** Persistent device identifier (add-online-multiplayer D8): one room
         * per device across relaunches; the server re-binds the seat by it. */
        fun deviceID(context: Context): String {
            val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            prefs.getString(KEY_DEVICE_ID, null)?.let { return it }
            val id = UUID.randomUUID().toString()
            prefs.edit().putString(KEY_DEVICE_ID, id).apply()
            return id
        }

        fun persistedRoomCode(context: Context): String? =
            context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
                .getString(KEY_ONLINE_ROOM_CODE, null)

        fun persistRoomCode(context: Context, code: String) {
            context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
                .edit().putString(KEY_ONLINE_ROOM_CODE, code).apply()
        }

        fun clearPersistedRoom(context: Context) {
            context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
                .edit().remove(KEY_ONLINE_ROOM_CODE).apply()
        }
    }

    /**
     * D1: promotions are the 5-character UCI moves. Group them by
     * destination and order the options q/r/b/n.
     */
    private fun pendingPromotionFor(from: String, to: String): PendingPromotion? {
        val promos = selectedMoves.filter {
            it.length == 5 && it.startsWith(from) && it.drop(2).take(2) == to
        }
        if (promos.isEmpty()) return null
        val options = listOf("q", "r", "b", "n").mapNotNull { piece ->
            promos.firstOrNull { it.endsWith(piece) }
        }
        if (options.isEmpty()) return null
        return PendingPromotion(from, to, options)
    }

    private fun playMove(from: String, to: String, uci: String) {
        if (gameMode is GameMode.Online) {
            // The server is authoritative (add-online-multiplayer D4/D7): send
            // the move and wait for the confirming snapshot; the board only
            // changes when the server confirms. A rejected move answers with a
            // structured error and the position stays as it is.
            clearSelection()
            online?.sendMove(uci)
            return
        }
        try {
            session.playMove(uciMove = uci)
            toMove = if (toMove == "w") "b" else "w"
            clearSelection()
            lastMove = LastMove(from, to)
            moveList = moveList + uci
            board = FenBoard.parse(session.getBoardState(), toMove)
            refreshStatus()
            // After any successful move (human or CPU), let the CPU answer if
            // it is its turn (design D5/D6). A no-op after a CPU move.
            triggerCpuMoveIfNeeded()
        } catch (e: Throwable) {
            errorMessage = "Move failed: ${describe(e)}"
        }
    }

    /**
     * Run the CPU driver when the CPU is to move (design D5/D6): mode is cpu,
     * it is the CPU's color, the game is still playing, and no move is in
     * flight. The move is computed off the main thread (Dispatchers.Default)
     * and applied on the main dispatcher through the same internal path a
     * human move uses, but only if the generation still matches (a newer game
     * discards the stale result).
     */
    private fun triggerCpuMoveIfNeeded() {
        val mode = gameMode
        if (mode !is GameMode.Cpu) return
        val current = status
        if (current !is GameStatus.Playing) return
        if (current.toMove != cpuColor) return
        if (cpuThinking) return

        cpuThinking = true
        val generation = cpuGeneration
        val difficulty = mode.difficulty
        val sess = session
        scope.launch {
            // Compute off the main thread; getCpuMove throws ChessException
            // on error (invalid difficulty / no legal move).
            val result: Result<String> = withContext(Dispatchers.Default) {
                runCatching { sess.getCpuMove(difficulty = difficulty.rawValue) }
            }
            // Back on the main dispatcher (the scope's context).
            // Defense in depth (enforce-single-active-game D3): in addition
            // to the generation guard, the session that computed the move
            // must still be the active one.
            if (cpuGeneration != generation || session !== sess) return@launch
            result.onSuccess { uci ->
                // DEBUG hook (enforce-single-active-game D6): hold the
                // thinking state so Resign/Undo can deterministically land
                // "while the CPU is thinking". If the game changes in the
                // meantime, applyCpuMove still discards the move.
                if (debugCpuDelayMs > 0L) delay(debugCpuDelayMs)
                applyCpuMove(uci, generation, sess)
            }.onFailure { error ->
                cpuThinking = false
                errorMessage = "CPU move failed: ${describe(error)}"
            }
        }
    }

    /** Apply a computed CPU move through the same internal path a human move
     * uses, but only if the game generation still matches (design D5) and
     * the session that computed it is still the active one
     * (enforce-single-active-game D3): a stale move from a replaced session
     * is never applied to the session that follows. */
    private fun applyCpuMove(uci: String, generation: Int, session: GameSession) {
        if (cpuGeneration != generation || session !== this.session) return
        cpuThinking = false
        // 5-char promotion UCIs included: destination is chars 2-3.
        val from = uci.take(2)
        val to = uci.drop(2).take(2)
        playMove(from, to, uci)
    }

    private fun refreshStatus() {
        try {
            status = when {
                session.isCheckmate() ->
                    // The side to move is mated, so the opponent wins.
                    GameStatus.Checkmated(if (board.sideToMove == "w") "Black" else "White")
                session.isDraw() -> GameStatus.Drawn
                session.isCheck() -> GameStatus.Playing(board.sideToMove, inCheck = true)
                else -> GameStatus.Playing(board.sideToMove, inCheck = false)
            }
            errorMessage = null
            markTerminalIfNeeded()
        } catch (e: Throwable) {
            status = GameStatus.Failed(describe(e))
            errorMessage = "FFI error: ${describe(e)}"
        }
    }

    /** Design D2 of add-game-end-dialog: present the game-end dialog exactly
     * once, the first time the game becomes terminal. */
    private fun markTerminalIfNeeded() {
        if (gameEndPresented) return
        val s = status
        if (s is GameStatus.Checkmated || s is GameStatus.Drawn ||
            s is GameStatus.Resigned || s is GameStatus.Forfeited ||
            s is GameStatus.TimedOut
        ) {
            gameEndPresented = true
            showGameEndDialog = true
        }
    }

    private fun clearSelection() {
        selectedSquare = null
        legalTargets = emptyList()
        selectedMoves = emptyList()
        pendingPromotion = null
    }

    private fun isOwn(piece: String, side: String): Boolean =
        if (side == "w") piece == piece.uppercase() else piece == piece.lowercase()
}

/**
 * Renders an FFI failure. The UniFFI `ChessException` subclasses expose empty
 * `message`s, so fall back to the concrete class name.
 */
internal fun describe(e: Throwable): String =
    if (e is ChessException) e::class.simpleName ?: "ChessException"
    else e.message ?: e.toString()
