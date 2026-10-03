package com.hectoralejandrg.plaintextchess

/**
 * An 8x8 board parsed from the board string returned by the core
 * (design D2: the board state returned by the core is the app-side source of
 * truth; the app never re-implements rules).
 *
 * `getBoardState()` returns the 8-rank position only (no FEN side-to-move
 * field), so the caller supplies [sideToMove] - the view model tracks it
 * locally and flips it after each played move.
 *
 * Layout: `grid[row][col]` with row 0 = rank 8 (top of the screen) and
 * col 0 = file a (left). Pieces are single characters ("K", "q", ...);
 * empty squares are "".
 */
data class FenBoard(
    val grid: List<List<String>>,
    val sideToMove: String,
) {
    companion object {
        const val START = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR"

        private val PIECE_CHARS = "pnbrqkPNBRQK".toSet()

        val start: FenBoard get() = parse(START, "w")

        /** Parses the core's 8-rank board string; throws [FenBoardException]. */
        fun parse(board: String, sideToMove: String): FenBoard {
            if (sideToMove != "w" && sideToMove != "b") {
                throw FenBoardException("malformed side to move: $sideToMove")
            }
            val ranks = board.split("/")
            if (ranks.size != 8) throw FenBoardException("malformed board: $board")
            val grid = ranks.map { rank ->
                val row = ArrayList<String>(8)
                for (ch in rank) {
                    val count = ch.digitToIntOrNull()
                    if (count != null) {
                        repeat(count) { row.add("") }
                    } else if (PIECE_CHARS.contains(ch)) {
                        row.add(ch.toString())
                    } else {
                        throw FenBoardException("malformed board: $board")
                    }
                }
                if (row.size != 8) throw FenBoardException("malformed board: $board")
                row
            }
            return FenBoard(grid, sideToMove)
        }

        /** "e2" from grid coordinates (row 0 = rank 8, col 0 = file a). */
        fun squareName(row: Int, col: Int): String = "${'a' + col}${8 - row}"

        /** Grid coordinates for a square name ("e2"); null when not a square. */
        fun parseSquare(name: String): Pair<Int, Int>? {
            if (name.length != 2) return null
            val col = name[0] - 'a'
            val rank = name[1].digitToIntOrNull() ?: return null
            if (col !in 0..7 || rank !in 1..8) return null
            return (8 - rank) to col
        }
    }
}

/** Raised when the core returns a board string that cannot be rendered. */
class FenBoardException(message: String) : Exception(message)
