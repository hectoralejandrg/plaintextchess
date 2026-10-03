import SwiftUI

/// Lichess-style brown board palette (design D7).
private let lightSquare = Color(red: 240.0 / 255.0, green: 217.0 / 255.0, blue: 181.0 / 255.0)
private let darkSquare = Color(red: 181.0 / 255.0, green: 136.0 / 255.0, blue: 99.0 / 255.0)

private let files = Array("abcdefgh")

/// 8x8 board rendered from the view model's `FenBoard` (design D7).
///
/// The grid fills the available width edge-to-edge and coordinates are drawn
/// *inside* the edge squares — files `a–h` along the bottom, ranks `8–1` along
/// the left — so no extra row/column is added. Markers: selected square
/// (border), legal targets (dot), last move (tint).
struct BoardView: View {
    @ObservedObject var vm: GameViewModel

    var body: some View {
        GeometryReader { geo in
            let cell = geo.size.width / 8
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

    // MARK: - Squares

    private func square(row: Int, col: Int, size: CGFloat) -> some View {
        let name = FenBoard.squareName(row: row, col: col)
        let piece = vm.board.grid[row][col]
        let isLight = (row + col).isMultiple(of: 2)
        let isSelected = vm.selectedSquare == name
        let isLegalTarget = vm.legalTargets.contains(name)
        let isLastMove = vm.lastMove?.from == name || vm.lastMove?.to == name

        let font = Font.system(size: size * 0.20, weight: .medium)
        let inset = size * 0.06

        return ZStack {
            Rectangle().fill(squareColor(isLight))
            if isLastMove {
                Rectangle().fill(Color.yellow.opacity(0.35))
            }
            if !piece.isEmpty {
                Image(pieceImageName(piece))
                    .resizable()
                    .interpolation(.high)
                    .scaledToFit()
                    .frame(width: size * 0.88, height: size * 0.88)
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
            // Files along the bottom edge (bottom-left corner).
            if row == 7 {
                Text(String(files[col]))
                    .font(font)
                    .foregroundColor(labelColor(isLight))
                    .padding(inset)
            }
        }
        .overlay(alignment: .topTrailing) {
            // Ranks along the right edge (top-right corner).
            if col == 7 {
                Text("\(8 - row)")
                    .font(font)
                    .foregroundColor(labelColor(isLight))
                    .padding(inset)
            }
        }
        .contentShape(Rectangle())
        .onTapGesture { vm.select(name) }
    }
}
