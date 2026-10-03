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

    /// Shows the new-game setup sheet (design D4).
    @State private var showNewGameSheet = false

    /// DEBUG launch-arg knobs (design D5); inert in release builds.
    private let debugAnimScale: Double
    private let debugDrag: DragTest?
    /// DEBUG: opponent mode to start in; lets the simulator script drive a
    /// CPU game for verification (`-PLAINTCHESS_MODE cpu [-PLAINTCHESS_DIFFICULTY …]`).
    private let debugMode: GameMode
    /// DEBUG: seconds to hold the CPU "thinking" state (test hook for 2.2c).
    private let debugCpuDelay: Double

    init() {
        var animScale = 1.0
        var drag: DragTest?
        var mode: GameMode = .twoPlayers
        var cpuDelay = 0.0
        #if DEBUG
        let args = ProcessInfo.processInfo.arguments
        if let i = args.firstIndex(of: "-PLAINTCHESS_ANIM_SCALE"),
           i + 1 < args.count, let s = Double(args[i + 1]), s > 0 {
            animScale = s
        }
        if let i = args.firstIndex(of: "-PLAINTCHESS_CPU_DELAY"),
           i + 1 < args.count, let s = Double(args[i + 1]), s > 0 {
            cpuDelay = s
        }
        if let i = args.firstIndex(of: "-PLAINTCHESS_DRAG"),
           i + 1 < args.count, !args[i + 1].isEmpty {
            let parts = args[i + 1].split(separator: ",").map { String($0) }
            drag = DragTest(from: parts[0],
                            to: parts.count > 1 ? parts[1] : nil)
        }
        if let i = args.firstIndex(of: "-PLAINTCHESS_MODE"),
           i + 1 < args.count, args[i + 1].lowercased() == "cpu" {
            var level: CpuDifficulty = .medium
            if let j = args.firstIndex(of: "-PLAINTCHESS_DIFFICULTY"),
               j + 1 < args.count {
                level = CpuDifficulty.named(args[j + 1]) ?? .medium
            }
            mode = .cpu(level)
        }
        #endif
        debugAnimScale = animScale
        debugDrag = drag
        debugMode = mode
        debugCpuDelay = cpuDelay
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
        .sheet(isPresented: $showNewGameSheet) {
            NewGameSetupView { mode in
                vm.startGame(mode)
            }
        }
        .onAppear {
            #if DEBUG
            vm.setDebugCpuDelay(debugCpuDelay)
            if case .cpu = debugMode {
                // Start directly in a CPU game for scripted verification.
                vm.startGame(debugMode)
            }
            startDebugScriptIfNeeded()
            #endif
        }
    }

    // MARK: - Status row

    @ViewBuilder
    private var statusRow: some View {
        HStack(spacing: 8) {
            if vm.cpuThinking {
                Text("CPU is thinking…")
                    .font(.headline)
            } else {
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
        Button(action: { showNewGameSheet = true }) {
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

/// New-game setup sheet (design D4): choose the opponent (two players or CPU)
/// and, for the CPU, its difficulty. Two players is pre-selected; confirming
/// starts the game with the chosen mode.
struct NewGameSetupView: View {
    @State private var isCpu = false
    @State private var difficulty: CpuDifficulty = .medium
    let onConfirm: (GameMode) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 20) {
            Text("New game")
                .font(.headline)

            VStack(spacing: 16) {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Opponent")
                        .font(.subheadline)
                        .foregroundColor(.secondary)
                    Picker("Opponent", selection: $isCpu) {
                        Text("Two players").tag(false)
                        Text("CPU").tag(true)
                    }
                    .pickerStyle(.segmented)
                }

                if isCpu {
                    VStack(alignment: .leading, spacing: 6) {
                        Text("Difficulty")
                            .font(.subheadline)
                            .foregroundColor(.secondary)
                        Picker("Difficulty", selection: $difficulty) {
                            ForEach(CpuDifficulty.allCases, id: \.self) { level in
                                Text(level.displayName).tag(level)
                            }
                        }
                        .pickerStyle(.segmented)
                    }
                }
            }

            Spacer(minLength: 0)

            Button(action: {
                onConfirm(isCpu ? .cpu(difficulty) : .twoPlayers)
                dismiss()
            }) {
                Text("Start")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)

            Button("Cancel") {
                dismiss()
            }
            .font(.footnote)
        }
        .padding()
    }
}
