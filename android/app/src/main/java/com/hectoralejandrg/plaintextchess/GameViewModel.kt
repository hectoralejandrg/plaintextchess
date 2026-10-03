package com.hectoralejandrg.plaintextchess

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
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
    data class Failed(val message: String) : GameStatus()
}

/**
 * Opponent mode for a game (design D4/D6): two human players, or a human
 * (White) versus the CPU (Black) at a chosen difficulty.
 */
sealed class GameMode {
    object TwoPlayers : GameMode()
    data class Cpu(val difficulty: CpuDifficulty) : GameMode()
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
class GameViewModel {

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

    /** Bumped on every [startGame] so an in-flight CPU move from an older game
     * is discarded instead of applied (design D5). */
    private var cpuGeneration = 0

    /** Game-end dialog bookkeeping (design D2 of add-game-end-dialog): the
     * modal is presented once per game and never re-presented after the
     * player dismisses it; [startGame] resets both flags. */
    private var gameEndPresented = false
    private var gameEndDismissed = false

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
        val square = FenBoard.parseSquare(squareName) ?: return false
        val (row, col) = square
        val piece = board.grid[row][col]
        return piece.isNotEmpty() && isOwn(piece, current.toMove)
    }

    /**
     * Discard the current session and start a fresh game in [mode]
     * (spec: new-game action; design D4/D5/D6).
     */
    fun startGame(mode: GameMode) {
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
                if (gameMode is GameMode.Cpu)
                    if (s.winner == "White") "Checkmate! You won!"
                    else "Checkmate! You lost."
                else "Checkmate! ${s.winner} wins."
            is GameStatus.Resigned ->
                if (gameMode is GameMode.Cpu) {
                    // Only the human resigns against the CPU (the human is
                    // White, design D6 of add-cpu-opponent).
                    "You resigned. You lost."
                } else {
                    val loser = if (s.winner == "White") "Black" else "White"
                    "$loser resigns. ${s.winner} wins."
                }
            is GameStatus.Drawn -> "The game is drawn."
            else -> null
        }

    /** "Play again" in the game-end dialog (design D2): a fresh game in the
     * current mode (and CPU difficulty). */
    fun restart() {
        startGame(gameMode)
    }

    /** "Done" in the game-end dialog (design D2): close the modal without
     * changing the game. The status line keeps showing the result. */
    fun dismissGameEnd() {
        gameEndDismissed = true
        showGameEndDialog = false
    }

    // endregion

    // region Resign / undo / flip (design D4/D5/D6 of add-game-end-dialog)

    /** Whether the game can be resigned or undone right now. */
    val canResign: Boolean
        get() = status is GameStatus.Playing

    val canUndo: Boolean
        get() = moveList.isNotEmpty() && status is GameStatus.Playing

    /**
     * Resign the current game (design D4): an app-level terminal state. In a
     * two-player game the side to move resigns; against the CPU the human
     * (White) resigns. Bumping the generation discards an in-flight CPU move
     * (same mechanism as [startGame], design D5 of add-cpu-opponent).
     */
    fun resign() {
        val current = status
        if (current !is GameStatus.Playing) return
        cpuGeneration += 1
        cpuThinking = false
        clearSelection()
        lastMove = null
        val winner = when (gameMode) {
            is GameMode.TwoPlayers -> if (toMove == "w") "Black" else "White"
            is GameMode.Cpu -> "Black"
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
            if (cpuGeneration != generation) return@launch
            result.onSuccess { uci ->
                applyCpuMove(uci, generation)
            }.onFailure { error ->
                cpuThinking = false
                errorMessage = "CPU move failed: ${describe(error)}"
            }
        }
    }

    /** Apply a computed CPU move through the same internal path a human move
     * uses, but only if the game generation still matches (design D5). */
    private fun applyCpuMove(uci: String, generation: Int) {
        if (cpuGeneration != generation) return
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
        if (s is GameStatus.Checkmated || s is GameStatus.Drawn || s is GameStatus.Resigned) {
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
