import SwiftUI
import UIKit

/// Lichess-style brown board palette (design D7).
private let lightSquare = Color(red: 240.0 / 255.0, green: 217.0 / 255.0, blue: 181.0 / 255.0)
private let darkSquare = Color(red: 181.0 / 255.0, green: 136.0 / 255.0, blue: 99.0 / 255.0)

private let files = Array("abcdefgh")

/// 8x8 board rendered from the view model's `FenBoard` (design D7).
///
/// The grid fills the available width edge-to-edge and coordinates are drawn
/// *inside* the edge squares — files `a–h` along the bottom, ranks `8–1` along
/// the right — so no extra row/column is added. Markers: selected square
/// (border), legal targets (dot), last move (tint). Interactions: two-tap
/// selection, drag & drop (D3), the inline promotion picker (D2) and the move
/// slide animation (D4).
struct BoardView: View {
    @ObservedObject var vm: GameViewModel

    /// DEBUG only (design D5): stretch the slide duration for screenshots.
    var animScale: Double = 1.0
    /// DEBUG only (design D5): simulate a drag without a pointer.
    var dragTest: DragTest? = nil

    /// Drag state (design D3): the lifted square + pointer position in
    /// board-local coordinates. Pure view-layer state; legality comes from
    /// the VM via the same `select` intent the tap path uses.
    @State private var dragFrom: String?
    @State private var dragLocation: CGPoint?

    /// Slide-animation state (design D4): the piece currently sliding and
    /// its animated offset. Render-only; the VM is already at the new
    /// position, so this can never desynchronize game state.
    @State private var slide: MoveSlide?
    @State private var slideOffset: CGSize = .zero

    /// The piece mid-slide (design D4).
    private struct MoveSlide {
        let piece: String
        let from: String
        let to: String
    }

    var body: some View {
        GeometryReader { geo in
            let cell = geo.size.width / 8
            ZStack(alignment: .topLeading) {
                grid(cell: cell)
                dragOverlay(cell: cell)
                slideOverlay(cell: cell)
                promotionOverlay(cell: cell)
            }
            .frame(width: 8 * cell, height: 8 * cell)
            .contentShape(Rectangle())
            .gesture(dragGesture(cell: cell))
            .onChange(of: vm.lastMove) { lastMove in
                startSlideIfNeeded(lastMove, cell: cell)
            }
            .onAppear {
                #if DEBUG
                applyDragTestIfNeeded(cell: cell)
                #endif
            }
        }
        .aspectRatio(1, contentMode: .fit)
    }

    // MARK: - Colors

    private func squareColor(_ isLight: Bool) -> Color {
        isLight ? lightSquare : darkSquare
    }

    private func labelColor(_ isLight: Bool) -> Color {
        isLight ? darkSquare : lightSquare
    }

    /// Asset-catalog image name for a FEN piece character ("K" -> "wK").
    private func pieceImageName(_ piece: String) -> String {
        (piece == piece.uppercased() ? "w" : "b") + piece.uppercased()
    }

    private func pieceImage(_ piece: String, size: CGFloat) -> some View {
        Image(pieceImageName(piece))
            .resizable()
            .interpolation(.high)
            .scaledToFit()
            .frame(width: size, height: size)
    }

    // MARK: - Geometry

    /// Board orientation (add-game-end-dialog D6): 0 = White on the bottom,
    /// 180 = board rotated. All square↔pixel resolution goes through the two
    /// mapping functions below, so taps, drags, highlighting, the promotion
    /// anchor, and the slide animation all follow the orientation for free.
    private var orientation: Int { vm.boardOrientation }

    /// Display cell of an actual square for the current orientation.
    private func displayIndex(row: Int, col: Int) -> (row: Int, col: Int) {
        orientation == 0 ? (row, col) : (7 - row, 7 - col)
    }

    /// Board-local center of a square.
    private func squareCenter(_ name: String, cell: CGFloat) -> CGPoint {
        guard let (row, col) = FenBoard.parseSquare(name) else { return .zero }
        let d = displayIndex(row: row, col: col)
        return CGPoint(x: (CGFloat(d.col) + 0.5) * cell,
                       y: (CGFloat(d.row) + 0.5) * cell)
    }

    /// Square under a board-local point; nil outside the board.
    private func squareName(at point: CGPoint, cell: CGFloat) -> String? {
        guard cell > 0 else { return nil }
        let dCol = Int(point.x / cell)
        let dRow = Int(point.y / cell)
        guard (0..<8).contains(dRow), (0..<8).contains(dCol) else { return nil }
        let a = orientation == 0 ? (dRow, dCol) : (7 - dRow, 7 - dCol)
        return FenBoard.squareName(row: a.0, col: a.1)
    }

    // MARK: - Grid

    private func grid(cell: CGFloat) -> some View {
        VStack(spacing: 0) {
            ForEach(0..<8) { row in
                HStack(spacing: 0) {
                    ForEach(0..<8) { col in
                        square(row: row, col: col, size: cell)
                    }
                }
            }
        }
    }

    // MARK: - Squares

    /// Renders the display cell `(row, col)`; the actual square (board
    /// coordinates) is derived from the orientation (add-game-end-dialog D6).
    private func square(row: Int, col: Int, size: CGFloat) -> some View {
        let aRow = orientation == 0 ? row : 7 - row
        let aCol = orientation == 0 ? col : 7 - col
        let name = FenBoard.squareName(row: aRow, col: aCol)
        let piece = vm.displayedBoard.grid[aRow][aCol]
        let isLight = (aRow + aCol).isMultiple(of: 2)
        let isSelected = vm.selectedSquare == name
        let isLegalTarget = vm.legalTargets.contains(name)
        let isLastMove = vm.lastMove?.from == name || vm.lastMove?.to == name
        // Lifted (D3) or being slid into (D4): the square shows no piece.
        let showPiece = !piece.isEmpty && dragFrom != name && slide?.to != name

        let font = Font.system(size: size * 0.20, weight: .medium)
        let inset = size * 0.06

        return ZStack {
            Rectangle().fill(squareColor(isLight))
            if isLastMove {
                Rectangle().fill(Color.yellow.opacity(0.35))
            }
            if showPiece {
                pieceImage(piece, size: size * 0.88)
            }
            if isLegalTarget {
                Circle()
                    .fill(Color.black.opacity(0.28))
                    .frame(width: size * 0.30, height: size * 0.30)
            }
        }
        .overlay(
            Rectangle()
                .stroke(isSelected ? Color.yellow : Color.black.opacity(0.5),
                        lineWidth: isSelected ? 3 : 0.5)
        )
        .frame(width: size, height: size)
        .overlay(alignment: .bottomLeading) {
            // Files along the bottom edge (bottom-left corner): the display
            // row 7 shows the actual file of the square underneath it.
            if row == 7 {
                Text(String(files[aCol]))
                    .font(font)
                    .foregroundColor(labelColor(isLight))
                    .padding(inset)
            }
        }
        .overlay(alignment: .topTrailing) {
            // Ranks along the right edge (top-right corner): the display
            // column 7 shows the actual rank of the square underneath it.
            if col == 7 {
                Text("\(8 - aRow)")
                    .font(font)
                    .foregroundColor(labelColor(isLight))
                    .padding(inset)
            }
        }
        .contentShape(Rectangle())
        .onTapGesture { vm.select(name) }
    }

    // MARK: - Drag & drop (design D3)

    /// One container gesture: a tap that never exceeds the minimum distance
    /// stays the per-square tap path; anything else is a drag.
    private func dragGesture(cell: CGFloat) -> some Gesture {
        DragGesture(minimumDistance: 8, coordinateSpace: .local)
            .onChanged { value in
                if dragFrom == nil {
                    // Never start a lift while the promotion picker is open.
                    guard vm.pendingPromotion == nil else { return }
                    guard let sq = squareName(at: value.startLocation, cell: cell),
                          vm.canPickup(sq) else { return }
                    vm.select(sq)
                    dragFrom = sq
                }
                dragLocation = value.location
            }
            .onEnded { value in
                guard let from = dragFrom else { return }
                dragFrom = nil
                dragLocation = nil
                drop(from: from, at: value.location, cell: cell)
            }
    }

    /// Shared drop entry point — the gesture's `onEnded` and the DEBUG drag
    /// hook (design D5) both call this.
    private func drop(from: String, at point: CGPoint, cell: CGFloat) {
        guard let dest = squareName(at: point, cell: cell) else { return }
        // Same intent as a tap: legal destination plays (or opens the
        // promotion picker); an illegal one shows "Not a legal move".
        vm.select(dest)
    }

    @ViewBuilder
    private func dragOverlay(cell: CGFloat) -> some View {
        if let from = dragFrom, let loc = dragLocation,
           let (row, col) = FenBoard.parseSquare(from) {
            let piece = vm.displayedBoard.grid[row][col]
            if !piece.isEmpty {
                pieceImage(piece, size: cell * 1.05)
                    .position(loc)
                    .allowsHitTesting(false)
            }
        }
    }

    #if DEBUG
    /// Design D5: `-PLAINTCHESS_DRAG "from[,to]"` forces the in-flight lift
    /// and then runs the same drop entry point the gesture calls.
    private func applyDragTestIfNeeded(cell: CGFloat) {
        guard let test = dragTest, cell > 0 else { return }
        guard vm.canPickup(test.from) else { return }
        vm.select(test.from)
        dragFrom = test.from
        dragLocation = squareCenter(test.from, cell: cell)
        guard let to = test.to else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) {
            withAnimation(.easeInOut(duration: 0.25)) {
                dragLocation = squareCenter(to, cell: cell)
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.35) {
                let location = dragLocation ?? .zero
                dragFrom = nil
                dragLocation = nil
                drop(from: test.from, at: location, cell: cell)
            }
        }
    }
    #endif

    // MARK: - Promotion picker (design D2)

    @ViewBuilder
    private func promotionOverlay(cell: CGFloat) -> some View {
        if let promo = vm.pendingPromotion,
           let (aRow, aCol) = FenBoard.parseSquare(promo.to) {
            let pieceSize = cell * 0.85
            let pad = cell * 0.12
            let cardW = pieceSize + 2 * pad
            let cardH = pieceSize * 4 + 2 * pad
            // Anchor over the destination square in display coordinates
            // (add-game-end-dialog D6); offset into the board interior: the
            // bottom display row gets the card above, the top row below.
            let dRow = orientation == 0 ? aRow : 7 - aRow
            let dCol = orientation == 0 ? aCol : 7 - aCol
            let x = min(max(CGFloat(dCol) * cell + (cell - cardW) / 2, 0),
                        8 * cell - cardW)
            let y: CGFloat = dRow == 7
                ? CGFloat(dRow) * cell - cardH + cell * 0.35
                : CGFloat(dRow) * cell + cell * 0.65

            ZStack(alignment: .topLeading) {
                // Any tap on the board outside the card cancels (D2).
                Color.clear
                    .contentShape(Rectangle())
                    .onTapGesture { vm.cancelPromotion() }

                VStack(spacing: 0) {
                    ForEach(["q", "r", "b", "n"], id: \.self) { piece in
                        // options are full UCI strings ("a2b1q", ...).
                        if promo.options.contains(where: { $0.hasSuffix(piece) }) {
                            pieceImage(promotedPiece(promo, piece: piece),
                                       size: pieceSize)
                                .contentShape(Rectangle())
                                .onTapGesture { vm.confirmPromotion(piece) }
                        }
                    }
                }
                .frame(width: cardW, height: cardH)
                .background(
                    RoundedRectangle(cornerRadius: 8)
                        // Light card so both black and white promotion
                        // pieces stay visible (lichess-style).
                        .fill(Color(red: 0.98, green: 0.96, blue: 0.92)
                            .opacity(0.95))
                )
                .contentShape(RoundedRectangle(cornerRadius: 8))
                .onTapGesture { vm.cancelPromotion() }
                .offset(x: x, y: y)
            }
        }
    }

    /// FEN character of the promoted piece (color from the pawn's side).
    private func promotedPiece(_ promo: GameViewModel.PendingPromotion,
                               piece: String) -> String {
        guard let (row, col) = FenBoard.parseSquare(promo.from) else { return piece }
        let pawn = vm.displayedBoard.grid[row][col]
        return pawn == pawn.uppercased() ? piece.uppercased() : piece
    }

    // MARK: - Slide animation (design D4)

    private func startSlideIfNeeded(_ lastMove: GameViewModel.LastMove?,
                                    cell: CGFloat) {
        slide = nil
        guard let lastMove else { return }
        let reduceMotion = UIAccessibility.isReduceMotionEnabled
        #if DEBUG
        let duration = reduceMotion ? 0 : 0.2 * animScale
        #else
        let duration = reduceMotion ? 0 : 0.2
        #endif
        guard duration > 0,
              let (toRow, toCol) = FenBoard.parseSquare(lastMove.to) else { return }
        let piece = vm.displayedBoard.grid[toRow][toCol]
        guard !piece.isEmpty else { return }

        slide = MoveSlide(piece: piece, from: lastMove.from, to: lastMove.to)
        var noAnim = Transaction()
        noAnim.disablesAnimations = true
        withTransaction(noAnim) {
            slideOffset = .zero
        }
        withAnimation(.easeInOut(duration: duration)) {
            slideOffset = CGSize(
                width: squareCenter(lastMove.to, cell: cell).x
                    - squareCenter(lastMove.from, cell: cell).x,
                height: squareCenter(lastMove.to, cell: cell).y
                    - squareCenter(lastMove.from, cell: cell).y)
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + duration + 0.05) {
            slide = nil
        }
    }

    @ViewBuilder
    private func slideOverlay(cell: CGFloat) -> some View {
        if let s = slide {
            pieceImage(s.piece, size: cell * 0.88)
                .position(x: squareCenter(s.from, cell: cell).x + slideOffset.width,
                          y: squareCenter(s.from, cell: cell).y + slideOffset.height)
                .allowsHitTesting(false)
        }
    }
}
