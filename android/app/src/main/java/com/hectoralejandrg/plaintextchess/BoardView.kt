package com.hectoralejandrg.plaintextchess

import android.provider.Settings
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.PlatformTextStyle
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay

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

/** The piece mid-slide (design D4): render-only overlay state. */
private data class MoveSlide(val piece: String, val from: String, val to: String)

/**
 * Board-local center of a square in px. Board orientation
 * (add-game-end-dialog D6): 0 = White on the bottom, 180 = board rotated;
 * the square is placed in its *display* cell.
 */
private fun squareCenterPx(name: String, cellPx: Float, orientation: Int): Offset {
    val square = FenBoard.parseSquare(name) ?: return Offset.Zero
    val (aRow, aCol) = square
    val row = if (orientation == 0) aRow else 7 - aRow
    val col = if (orientation == 0) aCol else 7 - aCol
    return Offset((col + 0.5f) * cellPx, (row + 0.5f) * cellPx)
}

/**
 * Square under a board-local point (px); null outside the board. The point's
 * display cell is mapped back to the board square for the orientation
 * (add-game-end-dialog D6).
 */
private fun squareAt(point: Offset, cellPx: Float, orientation: Int): String? {
    if (cellPx <= 0f) return null
    val x = point.x
    val y = point.y
    if (x < 0f || y < 0f || x >= 8f * cellPx || y >= 8f * cellPx) return null
    val dCol = (x / cellPx).toInt()
    val dRow = (y / cellPx).toInt()
    val row = if (orientation == 0) dRow else 7 - dRow
    val col = if (orientation == 0) dCol else 7 - dCol
    return FenBoard.squareName(row, col)
}

/**
 * 8x8 board rendered from the view model's [FenBoard] (design D7). The grid
 * fills the available width edge-to-edge; coordinates are drawn *inside* the
 * edge squares — files `a-h` along the bottom, ranks `8-1` along the right —
 * so no extra row/column is added. Markers: selected (border), legal targets
 * (dot), last move (tint). Interactions: two-tap selection, drag & drop (D3),
 * the inline promotion picker (D2) and the move slide animation (D4).
 */
@Composable
fun BoardView(vm: GameViewModel, modifier: Modifier = Modifier) {
    BoxWithConstraints(modifier = modifier.aspectRatio(1f)) {
        val cell = maxWidth / 8f
        val density = LocalDensity.current
        val cellPx = with(density) { cell.toPx() }

        // Board orientation (add-game-end-dialog D6): a display preference
        // owned by the VM; all square<->pixel resolution flows through it.
        val orientation = vm.boardOrientation

        // D3: drag state (design D3): the lifted square + pointer position in
        // board-local px. Pure view-layer state; the lift/drop go through the
        // same [GameViewModel.select] intent as a tap.
        var dragFrom by remember { mutableStateOf<String?>(null) }
        var dragPos by remember { mutableStateOf<Offset?>(null) }

        // D4: slide-animation state (design D4): the piece currently sliding
        // and the 0..1 progress of the slide. Render-only; the VM is already
        // at the new position, so this can never desynchronize the game
        // state.
        var slide by remember { mutableStateOf<MoveSlide?>(null) }
        val slideProgress = remember { Animatable(0f) }

        // D4: honor the system animation scale — the Android equivalent of
        // iOS Reduce Motion (design D5): a scale of 0 skips the slide and a
        // scale > 1 stretches it (also how the 0.2 s slide is screenshot
        // verifiable on the emulator).
        val context = LocalContext.current
        val animScale =
            Settings.Global.getFloat(
                context.contentResolver,
                Settings.Global.ANIMATOR_DURATION_SCALE,
                1f,
            )
        val animationsEnabled = animScale != 0f

        // D4: start the slide when the last move changes.
        LaunchedEffect(vm.lastMove) {
            val last = vm.lastMove
            slide = null
            val lastMove = last ?: return@LaunchedEffect
            if (!animationsEnabled) return@LaunchedEffect
            val square = FenBoard.parseSquare(lastMove.to) ?: return@LaunchedEffect
            val (row, col) = square
            val piece = vm.displayedBoard.grid[row][col]
            if (piece.isEmpty()) return@LaunchedEffect
            slide = MoveSlide(piece, lastMove.from, lastMove.to)
            slideProgress.snapTo(0f)
            // 0.2 s base duration; the system animator_duration_scale
            // stretches it (also how the slide is screenshot-verifiable on
            // the emulator). (This toolchain's Compose has no `withAnimation`,
            // so the slide is driven by an `Animatable` with the same
            // 200 ms tween.)
            slideProgress.animateTo(
                targetValue = 1f,
                animationSpec = tween(200, easing = FastOutSlowInEasing),
            )
            delay(50)
            slide = null
        }

        Column(
            Modifier
                .fillMaxSize()
                .pointerInput(cellPx, orientation) {
                    // D3: one container gesture coexisting with the per-square
                    // tap path (the official drag detector built on
                    // awaitEachGesture/awaitFirstDown). A tap (movement under
                    // the system slop) stays the clickable path; movement past
                    // the slop lifts an own piece, and the release runs the
                    // same drop intent a tap would.
                    detectDragGestures(
                        onDragStart = { start ->
                            val square = squareAt(start, cellPx, orientation)
                            if (square != null && vm.canPickup(square)) {
                                // Same intent as a tap: select the lifted piece.
                                vm.select(square)
                                dragFrom = square
                                dragPos = start
                            }
                        },
                        onDragEnd = {
                            val from = dragFrom
                            if (from != null) {
                                // Shared drop entry point: legal destination
                                // plays (or opens the promotion picker); an
                                // illegal one shows "Not a legal move" without
                                // changing the position.
                                val dest = dragPos?.let { squareAt(it, cellPx, orientation) }
                                if (dest != null) vm.select(dest)
                            }
                            dragFrom = null
                            dragPos = null
                        },
                        onDragCancel = {
                            dragFrom = null
                            dragPos = null
                        },
                    ) { change, _ ->
                        dragPos = change.position
                    }
                },
        ) {
            for (row in 0 until 8) {
                Row(Modifier.weight(1f).fillMaxWidth()) {
                    for (col in 0 until 8) {
                        // Display cell (row, col) -> actual square for the
                        // orientation (add-game-end-dialog D6).
                        val aRow = if (orientation == 0) row else 7 - row
                        val aCol = if (orientation == 0) col else 7 - col
                        val name = FenBoard.squareName(aRow, aCol)
                        // Lifted (D3) or being slid into (D4): show no piece.
                        val hidePiece = name == dragFrom || slide?.to == name
                        SquareCell(
                            vm = vm,
                            row = aRow,
                            col = aCol,
                            cell = cell,
                            modifier = Modifier.weight(1f).fillMaxHeight(),
                            hidePiece = hidePiece,
                            displayRow = row,
                            displayCol = col,
                        )
                    }
                }
            }
        }

        // D3: the lifted piece follows the pointer.
        val liftFrom = dragFrom
        val liftPos = dragPos
        if (liftFrom != null && liftPos != null) {
            FenBoard.parseSquare(liftFrom)?.let { (row, col) ->
                val piece = vm.displayedBoard.grid[row][col]
                if (piece.isNotEmpty()) {
                    val liftSize = cell * 1.05f
                    Image(
                        painter = painterResource(id = pieceRes(piece)),
                        contentDescription = null,
                        contentScale = ContentScale.Fit,
                        modifier = Modifier
                            .offset(
                                x = with(density) { (liftPos.x - liftSize.toPx() / 2f).toDp() },
                                y = with(density) { (liftPos.y - liftSize.toPx() / 2f).toDp() },
                            )
                            .size(liftSize),
                    )
                }
            }
        }

        // D4: the sliding piece (the destination square shows no piece while
        // the slide is in flight).
        slide?.let { s ->
            val fromCenter = squareCenterPx(s.from, cellPx, orientation)
            val toCenter = squareCenterPx(s.to, cellPx, orientation)
            val progress = slideProgress.value
            val center = Offset(
                fromCenter.x + (toCenter.x - fromCenter.x) * progress,
                fromCenter.y + (toCenter.y - fromCenter.y) * progress,
            )
            val size = cell * 0.88f
            Image(
                painter = painterResource(id = pieceRes(s.piece)),
                contentDescription = null,
                contentScale = ContentScale.Fit,
                modifier = Modifier
                    .offset(
                        x = with(density) { (center.x - size.toPx() / 2f).toDp() },
                        y = with(density) { (center.y - size.toPx() / 2f).toDp() },
                    )
                    .size(size),
            )
        }

        // D2: the promotion picker. A full-board tap catcher cancels the move;
        // the card is drawn on top of it.
        vm.pendingPromotion?.let { promo ->
            Box(
                Modifier
                    .fillMaxSize()
                    .clickable { vm.cancelPromotion() },
            )
            PromotionCard(vm, promo, cell)
        }
    }
}

/**
 * Inline promotion picker (design D2): a semi-opaque card of the cburnett
 * promotion pieces anchored over the destination square, offset into the
 * board interior (below it for white promotions on the top row, above it for
 * black promotions on the bottom row).
 */
@Composable
private fun PromotionCard(vm: GameViewModel, promo: GameViewModel.PendingPromotion, cell: Dp) {
    // Anchor over the destination square in *display* coordinates
    // (add-game-end-dialog D6); the card keeps its board-interior offset
    // regardless of the orientation.
    val (aRow, aCol) = FenBoard.parseSquare(promo.to) ?: return
    val orientation = vm.boardOrientation
    val row = if (orientation == 0) aRow else 7 - aRow
    val col = if (orientation == 0) aCol else 7 - aCol
    // Tap targets must stay >= 48 dp on the smaller phones.
    val base = cell * 0.85f
    val pieceSize = if (base < 48.dp) 48.dp else base
    val pad = cell * 0.12f
    val cardW = pieceSize + pad * 2f
    val cardH = pieceSize * 4f + pad * 2f
    val boardW = cell * 8f
    // Clamp the card into the board (no Dp.coerceIn in this toolchain).
    var x = cell * col + (cell - cardW) / 2f
    if (x < 0.dp) x = 0.dp
    if (x > boardW - cardW) x = boardW - cardW
    val y = if (row == 7) cell * row - cardH + cell * 0.35f else cell * row + cell * 0.65f

    val isWhite = isWhitePromotion(vm, promo)

    Column(
        modifier = Modifier
            .offset(x = x, y = y)
            .size(width = cardW, height = cardH)
            .background(
                color = Color(red = 0.98f, green = 0.96f, blue = 0.92f).copy(alpha = 0.95f),
                shape = RoundedCornerShape(8.dp),
            )
            // Tapping the card background cancels, like a tap outside it.
            .clickable { vm.cancelPromotion() },
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        // Options are full UCI strings ("a2b1q", ...); one button per
        // promotion piece, ordered q/r/b/n.
        listOf("q", "r", "b", "n").forEach { piece ->
            if (promo.options.any { it.endsWith(piece) }) {
                val fenPiece = if (isWhite) piece.uppercase() else piece
                Image(
                    painter = painterResource(id = pieceRes(fenPiece)),
                    contentDescription = null,
                    contentScale = ContentScale.Fit,
                    modifier = Modifier
                        .size(pieceSize)
                        .clickable { vm.confirmPromotion(piece) },
                )
            }
        }
    }
}

/** Whether the promoting pawn is white (color of the piece on `from`). */
private fun isWhitePromotion(vm: GameViewModel, promo: GameViewModel.PendingPromotion): Boolean {
    val square = FenBoard.parseSquare(promo.from) ?: return true
    val piece = vm.displayedBoard.grid[square.first][square.second]
    return piece.isNotEmpty() && piece == piece.uppercase()
}

@Composable
private fun SquareCell(
    vm: GameViewModel,
    row: Int,
    col: Int,
    cell: Dp,
    modifier: Modifier,
    hidePiece: Boolean = false,
    displayRow: Int = row,
    displayCol: Int = col,
) {
    val name = FenBoard.squareName(row, col)
    val piece = vm.displayedBoard.grid[row][col]
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
        if (piece.isNotEmpty() && !hidePiece) {
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
        // ranks along the right edge (top-right corner). The conditions use
        // the display cell; the labels name the actual square underneath
        // (add-game-end-dialog D6).
        val labelFont = with(LocalDensity.current) { (cell * 0.20f).toSp() }
        if (displayRow == 7) {
            CoordinateText(
                text = FILES[col].toString(),
                fontSize = labelFont,
                color = labelColor(isLight),
                modifier = Modifier.align(Alignment.BottomStart).padding(cell * 0.06f),
            )
        }
        if (displayCol == 7) {
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
