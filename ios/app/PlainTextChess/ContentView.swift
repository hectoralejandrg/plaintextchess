import SwiftUI

/// Playable two-player chess game screen (milestone 1).
///
/// Replaces the placeholder screen: status row, 8x8 board, move list and
/// controls. All game state comes from the view model, which is backed by
/// the Rust core through the UniFFI Swift bindings.
/// DEBUG-only drag simulation (design D5): force the in-flight lift on
/// `from` and, when `to` is set, run the same drop entry point the gesture
/// calls. `xcrun simctl launch <sim> <bundle> -PLAINTCHESS_DRAG "e2[,e4]"`.
struct DragTest {
    let from: String
    let to: String?
}

struct ContentView: View {
    @StateObject private var vm = GameViewModel()

    /// DEBUG launch-arg knobs (design D5); inert in release builds.
    private let debugAnimScale: Double
    private let debugDrag: DragTest?

    init() {
        var animScale = 1.0
        var drag: DragTest?
        #if DEBUG
        let args = ProcessInfo.processInfo.arguments
        if let i = args.firstIndex(of: "-PLAINTCHESS_ANIM_SCALE"),
           i + 1 < args.count, let s = Double(args[i + 1]), s > 0 {
            animScale = s
        }
        if let i = args.firstIndex(of: "-PLAINTCHESS_DRAG"),
           i + 1 < args.count, !args[i + 1].isEmpty {
            let parts = args[i + 1].split(separator: ",").map { String($0) }
            drag = DragTest(from: parts[0],
                            to: parts.count > 1 ? parts[1] : nil)
        }
        #endif
        debugAnimScale = animScale
        debugDrag = drag
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("PlainTextChess")
                .font(.largeTitle.bold())
                .padding(.horizontal)

            statusRow
                .padding(.horizontal)

            BoardView(vm: vm,
                      animScale: debugAnimScale,
                      dragTest: debugDrag)
                .frame(maxWidth: .infinity)

            moveList
                .padding(.horizontal)

            controls
                .padding(.horizontal)
        }
        .padding(.vertical)
        .onAppear {
            #if DEBUG
            startDebugScriptIfNeeded()
            #endif
        }
    }

    // MARK: - Status row

    @ViewBuilder
    private var statusRow: some View {
        HStack(spacing: 8) {
            switch vm.status {
            case .starting:
                Text("Creating session…")
                    .font(.headline)
            case .playing(let toMove, let inCheck):
                Text(toMove == "w" ? "White to move" : "Black to move")
                    .font(.headline)
                if inCheck {
                    Text("— Check!")
                        .font(.headline)
                        .foregroundColor(.red)
                }
            case .checkmated(let winner):
                Text("Checkmate! \(winner) wins")
                    .font(.headline)
            case .drawn:
                Text("Game drawn")
                    .font(.headline)
            case .failed:
                Text("Game unavailable")
                    .font(.headline)
                    .foregroundColor(.red)
            }
            Spacer()
        }
    }

    // MARK: - Move list (design D5: UCI strings as returned by the core)

    @ViewBuilder
    private var moveList: some View {
        if vm.moveList.isEmpty {
            Text("No moves yet")
                .font(.caption)
                .foregroundColor(.secondary)
        } else {
            ScrollView {
                LazyVGrid(
                    columns: [
                        GridItem(.flexible(), alignment: .leading),
                        GridItem(.flexible(), alignment: .leading),
                    ],
                    spacing: 4
                ) {
                    ForEach(Array(vm.moveList.enumerated()), id: \.offset) { index, uci in
                        HStack(spacing: 4) {
                            if index.isMultiple(of: 2) {
                                Text("\(index / 2 + 1).")
                                    .font(.caption.monospacedDigit())
                            }
                            Text(uci)
                                .font(.caption.monospaced())
                        }
                    }
                }
            }
            .frame(maxHeight: 120)
        }
    }

    // MARK: - Controls

    @ViewBuilder
    private var controls: some View {
        if let error = vm.errorMessage {
            Text(error)
                .font(.footnote)
                .foregroundColor(.red)
        }
        Button(action: { vm.newGame() }) {
            Label("New game", systemImage: "arrow.counterclockwise")
                .frame(maxWidth: .infinity)
        }
        .buttonStyle(.borderedProminent)
    }

#if DEBUG
    /// Test hook: `xcrun simctl launch <sim> <bundle> -PLAINTCHESS_SCRIPT "e2e4 e7e5"`
    /// plays the scripted sequence through the view model for smoke testing.
    private func startDebugScriptIfNeeded() {
        let args = ProcessInfo.processInfo.arguments
        guard
            let index = args.firstIndex(of: "-PLAINTCHESS_SCRIPT"),
            index + 1 < args.count
        else { return }
        let moves = args[index + 1].split(separator: " ").map { String($0) }
        vm.playScript(moves)
    }
#endif
}
