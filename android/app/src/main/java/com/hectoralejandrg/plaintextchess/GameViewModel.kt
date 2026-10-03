package com.hectoralejandrg.plaintextchess

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import uniffi.chess_core.ChessException
import uniffi.chess_core.GameSession
import uniffi.chess_core.newGameSession

/** Milestone-1 game status (design D6: pulled from the core after each move). */
sealed class GameStatus {
    object Starting : GameStatus()
    data class Playing(val toMove: String, val inCheck: Boolean) : GameStatus()
    data class Checkmated(val winner: String) : GameStatus()
    object Drawn : GameStatus()
    data class Failed(val message: String) : GameStatus()
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

    private var session: GameSession = newGameSession(initialRating = 1500.0)

    /** Side to move, tracked locally (the core's board state is position-only). */
    private var toMove: String = "w"

    /**
     * Full UCI moves of the currently selected piece (design D1): the
     * 5-character ones are the promotion options.
     */
    private var selectedMoves: List<String> = emptyList()

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
        val current = status
        if (current !is GameStatus.Playing) return false
        val square = FenBoard.parseSquare(squareName) ?: return false
        val (row, col) = square
        val piece = board.grid[row][col]
        return piece.isNotEmpty() && isOwn(piece, current.toMove)
    }

    /** Discard the current session and start a fresh game. */
    fun newGame() {
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
        try {
            session.playMove(uciMove = uci)
            toMove = if (toMove == "w") "b" else "w"
            clearSelection()
            lastMove = LastMove(from, to)
            moveList = moveList + uci
            board = FenBoard.parse(session.getBoardState(), toMove)
            refreshStatus()
        } catch (e: Throwable) {
            errorMessage = "Move failed: ${describe(e)}"
        }
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
        } catch (e: Throwable) {
            status = GameStatus.Failed(describe(e))
            errorMessage = "FFI error: ${describe(e)}"
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
