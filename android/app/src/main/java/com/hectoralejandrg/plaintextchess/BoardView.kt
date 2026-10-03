package com.hectoralejandrg.plaintextchess

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.PlatformTextStyle
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp

// Lichess-style brown board palette (design D7).
private val LIGHT_SQUARE = Color(red = 240f / 255f, green = 217f / 255f, blue = 181f / 255f)
private val DARK_SQUARE = Color(red = 181f / 255f, green = 136f / 255f, blue = 99f / 255f)

private const val FILES = "abcdefgh"

private fun squareColor(isLight: Boolean): Color = if (isLight) LIGHT_SQUARE else DARK_SQUARE
private fun labelColor(isLight: Boolean): Color = if (isLight) DARK_SQUARE else LIGHT_SQUARE

/** Drawable resource for a FEN piece character ("K" -> wk, "k" -> bk). */
private fun pieceRes(piece: String): Int {
    val black = piece == piece.lowercase()
    return when (piece[0].lowercaseChar()) {
        'k' -> if (black) R.drawable.bk else R.drawable.wk
        'q' -> if (black) R.drawable.bq else R.drawable.wq
        'r' -> if (black) R.drawable.br else R.drawable.wr
        'b' -> if (black) R.drawable.bb else R.drawable.wb
        'n' -> if (black) R.drawable.bn else R.drawable.wn
        else -> if (black) R.drawable.bp else R.drawable.wp
    }
}

/**
 * 8x8 board rendered from the view model's [FenBoard] (design D7). The grid
 * fills the available width edge-to-edge; coordinates are drawn *inside* the
 * edge squares — files `a-h` along the bottom, ranks `8-1` along the left —
 * so no extra row/column is added. Markers: selected (border), legal targets
 * (dot), last move (tint).
 */
@Composable
fun BoardView(vm: GameViewModel, modifier: Modifier = Modifier) {
    BoxWithConstraints(modifier = modifier.aspectRatio(1f)) {
        val cell = maxWidth / 8f
        Column(Modifier.fillMaxSize()) {
            for (row in 0 until 8) {
                Row(Modifier.weight(1f).fillMaxWidth()) {
                    for (col in 0 until 8) {
                        SquareCell(vm, row, col, cell, Modifier.weight(1f).fillMaxHeight())
                    }
                }
            }
        }
    }
}

@Composable
private fun SquareCell(vm: GameViewModel, row: Int, col: Int, cell: Dp, modifier: Modifier) {
    val name = FenBoard.squareName(row, col)
    val piece = vm.board.grid[row][col]
    val isLight = (row + col) % 2 == 0
    val isSelected = vm.selectedSquare == name
    val isLegalTarget = vm.legalTargets.contains(name)
    val isLastMove = vm.lastMove?.let { it.from == name || it.to == name } == true

    Box(
        modifier = modifier
            .background(squareColor(isLight))
            .clickable { vm.select(name) },
        contentAlignment = Alignment.Center,
    ) {
        if (isLastMove) {
            Box(Modifier.fillMaxSize().background(Color.Yellow.copy(alpha = 0.35f)))
        }
        if (piece.isNotEmpty()) {
            Image(
                painter = painterResource(id = pieceRes(piece)),
                contentDescription = null,
                contentScale = ContentScale.Fit,
                modifier = Modifier.fillMaxSize().padding(cell * 0.07f),
            )
        }
        if (isLegalTarget) {
            Box(
                Modifier
                    .size(cell * 0.30f)
                    .clip(CircleShape)
                    .background(Color.Black.copy(alpha = 0.28f))
            )
        }
        // Coordinates: files along the bottom edge (bottom-left corner),
        // ranks along the right edge (top-right corner).
        val labelFont = with(LocalDensity.current) { (cell * 0.20f).toSp() }
        if (row == 7) {
            CoordinateText(
                text = FILES[col].toString(),
                fontSize = labelFont,
                color = labelColor(isLight),
                modifier = Modifier.align(Alignment.BottomStart).padding(cell * 0.06f),
            )
        }
        if (col == 7) {
            CoordinateText(
                text = "${8 - row}",
                fontSize = labelFont,
                color = labelColor(isLight),
                modifier = Modifier.align(Alignment.TopEnd).padding(cell * 0.06f),
            )
        }
        Box(
            Modifier
                .fillMaxSize()
                .border(
                    width = if (isSelected) 3.dp else 0.5.dp,
                    color = if (isSelected) Color.Yellow else Color.Black.copy(alpha = 0.5f),
                )
        )
    }
}

@Composable
private fun CoordinateText(text: String, fontSize: TextUnit, color: Color, modifier: Modifier = Modifier) {
    Text(
        text = text,
        fontSize = fontSize,
        color = color,
        modifier = modifier,
        style = LocalTextStyle.current.copy(
            platformStyle = PlatformTextStyle(includeFontPadding = false),
        ),
    )
}
