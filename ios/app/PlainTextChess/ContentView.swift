import SwiftUI
import UIKit

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
    /// DEBUG: seconds between script tokens (add-game-end-dialog D7): default
    /// 0.5 s; `-PLAINTCHESS_SCRIPT_INTERVAL` widens it so CPU-mode scripts
    /// wait out CPU replies instead of racing the `cpuThinking` guard.
    private let debugScriptInterval: Double
    /// DEBUG: online server URL override (add-online-multiplayer D9); wins
    /// over `OnlineConfig.defaultURL`.
    private let debugOnlineURL: String?

    init() {
        var animScale = 1.0
        var drag: DragTest?
        var mode: GameMode = .twoPlayers
        var cpuDelay = 0.0
        var scriptInterval = 0.5
        var onlineURL: String?
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
        if let i = args.firstIndex(of: "-PLAINTCHESS_SCRIPT_INTERVAL"),
           i + 1 < args.count, let s = Double(args[i + 1]), s > 0 {
            scriptInterval = s
        }
        if let i = args.firstIndex(of: "-PLAINTCHESS_ONLINE_URL"),
           i + 1 < args.count, !args[i + 1].isEmpty {
            onlineURL = args[i + 1]
        }
        #endif
        debugAnimScale = animScale
        debugDrag = drag
        debugMode = mode
        debugCpuDelay = cpuDelay
        debugScriptInterval = scriptInterval
        debugOnlineURL = onlineURL
    }

    /// The online server URL for this build: the DEBUG launch-arg override
    /// wins over the default constant (add-online-multiplayer D9). `nil` when
    /// none is configured: online play is then unavailable.
    private var effectiveOnlineURL: String? {
        let url = debugOnlineURL ?? OnlineConfig.defaultURL
        return url.isEmpty ? nil : url
    }

    var body: some View {
        GeometryReader { proxy in
            // The board always spans the full container width and keeps that
            // exact size in every state. A fixed square frame derived from
            // the screen width can never be squeezed by the dynamic siblings
            // (waiting banner, error footnote, growing move list), so the
            // board never resizes. The chrome around it is budgeted so the
            // worst simultaneous content still fits around a full-width
            // board on an iPhone 16 Pro (472.7 pt available): the move list
            // is capped at 96 pt, the stack spacing is 10 pt, and the
            // waiting banner keeps a 10 pt inset, leaving slack for Dynamic
            // Type / rendering differences.
            let boardSide = proxy.size.width
            VStack(alignment: .leading, spacing: 10) {
                Text("PlainTextChess")
                    .font(.largeTitle.bold())
                    .padding(.horizontal)

                statusRow
                    .padding(.horizontal)

                if vm.isOnlineMode {
                    // Online clocks (add-online-time-controls, design D6): a
                    // fixed-height row so the full-width board keeps its exact
                    // size in every online state.
                    clockRow
                        .padding(.horizontal)
                }

                if case .waiting(let code) = vm.onlinePhase {
                    // Online waiting state (add-online-multiplayer D4): the room
                    // code, with a copy action, while the opponent is still away.
                    waitingBanner(code: code)
                        .padding(.horizontal)
                }

                BoardView(vm: vm,
                          animScale: debugAnimScale,
                          dragTest: debugDrag)
                    .frame(width: boardSide, height: boardSide)
                    .overlay {
                        // Reconnect banner (add-online-multiplayer D7): the last
                        // known position stays on screen behind it.
                        if vm.onlineReconnecting {
                            Text("Reconnecting…")
                                .font(.callout.bold())
                                .padding(.horizontal, 14)
                                .padding(.vertical, 8)
                                .background(.thinMaterial, in: Capsule())
                        }
                    }

                moveList
                    .padding(.horizontal)

                controls
                    .padding(.horizontal)
            }
            .padding(.vertical)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        }
        // The sheet also closes itself once the server confirms an online
        // seat (`onlineSessionReady`); opening it resets the handshake flag.
        .sheet(isPresented: Binding(
            get: { showNewGameSheet && !vm.onlineSessionReady },
            set: { showNewGameSheet = $0 }
        )) {
            NewGameSetupView(
                hasServerURL: effectiveOnlineURL != nil,
                joinError: vm.onlineJoinError
            ) { choice in
                switch choice {
                case .twoPlayers:
                    vm.startGame(.twoPlayers)
                case .cpu(let difficulty):
                    vm.startGame(.cpu(difficulty))
                case .onlineCreate(let timeControl):
                    _ = vm.startOnlineGame(create: true, timeControl: timeControl,
                                           serverURLString: effectiveOnlineURL)
                case .onlineJoin(let code):
                    _ = vm.startOnlineGame(create: false, code: code,
                                           serverURLString: effectiveOnlineURL)
                }
            }
        }
        .onAppear {
            // Relaunch recovery (add-online-multiplayer D8, E2E 5.2): re-attach
            // to a persisted in-progress room within the server's window.
            vm.restoreOnlineSessionIfNeeded(serverURLString: effectiveOnlineURL)
            #if DEBUG
            vm.setDebugCpuDelay(debugCpuDelay)
            vm.setDebugOnlineURL(effectiveOnlineURL)
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
            if vm.onlineReconnecting {
                // Online: the socket dropped mid-game; the last known position
                // stays on screen (add-online-multiplayer D7).
                Text("Reconnecting…")
                    .font(.headline)
                    .foregroundColor(.orange)
            } else if case .waiting = vm.onlinePhase {
                Text("Waiting for opponent to join…")
                    .font(.headline)
            } else if vm.cpuThinking {
                Text("CPU is thinking…")
                    .font(.headline)
            } else {
                switch vm.status {
                case .starting:
                    Text(vm.isOnlineMode ? "Connecting to online server…"
                                         : "Creating session…")
                        .font(.headline)
                case .playing(let toMove, let inCheck):
                    if vm.isOnlineMode, toMove == vm.onlineYourColor {
                        // Clearly mark the player's own turn
                        // (add-online-multiplayer D4).
                        Text("Your move")
                            .font(.headline.bold())
                    } else if vm.isOnlineMode {
                        Text("Opponent to move")
                            .font(.headline)
                    } else {
                        Text(toMove == "w" ? "White to move" : "Black to move")
                            .font(.headline)
                    }
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
                case .resigned(let winner):
                    Text((winner == "White" ? "Black" : "White") + " resigns")
                        .font(.headline)
                case .forfeited(let winner):
                    Text((winner == "White" ? "Black" : "White") + " forfeits")
                        .font(.headline)
                case .timedOut(let winner):
                    Text((winner == "White" ? "Black" : "White") + " ran out of time")
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

    /// Online clocks (add-online-time-controls, design D6): the side on the
    /// top edge of the board first, then the side on the bottom edge, each
    /// labeled by color so the two clocks are never confusable. The
    /// side-to-move clock is emphasized and turns red at ≤ 10 s. A
    /// `TimelineView` interpolates the countdown between server snapshots.
    @ViewBuilder
    private var clockRow: some View {
        if let clock = vm.onlineClock {
            TimelineView(.periodic(from: .now, by: 0.1)) { context in
                let white = clock.remaining(for: "w", now: context.date)
                let black = clock.remaining(for: "b", now: context.date)
                let topColor = vm.boardOrientation == 0 ? "Black" : "White"
                let bottomColor = vm.boardOrientation == 0 ? "White" : "Black"
                HStack(spacing: 8) {
                    clockCell(color: topColor,
                              milliseconds: topColor == "White" ? white : black,
                              active: clock.isRunning && clock.sideToMove == (topColor == "White" ? "w" : "b"))
                    Spacer()
                    Text(vm.onlineTimeControl)
                        .font(.caption.monospaced())
                        .foregroundColor(.secondary)
                    Spacer()
                    clockCell(color: bottomColor,
                              milliseconds: bottomColor == "White" ? white : black,
                              active: clock.isRunning && clock.sideToMove == (bottomColor == "White" ? "w" : "b"))
                }
                .frame(height: 34)
            }
        }
    }

    private func clockCell(color: String, milliseconds: Int, active: Bool) -> some View {
        let low = milliseconds <= 10_000
        return VStack(spacing: 0) {
            Text(color)
                .font(.caption2)
                .foregroundColor(.secondary)
            Text(OnlineClock.format(milliseconds))
                .font(.headline.monospacedDigit())
                .foregroundColor(low ? .red : .primary)
        }
        .frame(minWidth: 74)
        .padding(.vertical, 2)
        .background(
            RoundedRectangle(cornerRadius: 8)
                .fill(active ? Color.accentColor.opacity(0.18) : Color.clear)
        )
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .stroke(active ? Color.accentColor : Color.clear, lineWidth: 1)
        )
    }

    /// Online waiting banner (add-online-multiplayer D4): the 6-character room
    /// code, large and monospaced, with a copy action next to it.
    private func waitingBanner(code: String) -> some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Room code")
                    .font(.caption)
                    .foregroundColor(.secondary)
                Text(code)
                    .font(.title3.monospaced().bold())
            }
            Spacer()
            Button("Copy") {
                UIPasteboard.general.string = code
            }
            .buttonStyle(.bordered)
        }
        .padding(10)
        .background(.thinMaterial, in: RoundedRectangle(cornerRadius: 10))
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
            .frame(maxHeight: 96)
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
        // Game controls (add-game-end-dialog D6): secondary actions above the
        // prominent New game action, with the spec's availability rules.
        HStack(spacing: 8) {
            Button(action: { vm.undo() }) {
                Label("Undo", systemImage: "arrow.uturn.backward")
                    .frame(maxWidth: .infinity)
            }
            .disabled(!vm.canUndo)

            Button(action: { vm.resign() }) {
                Label("Resign", systemImage: "flag")
                    .frame(maxWidth: .infinity)
            }
            .disabled(!vm.canResign)

            Button(action: { vm.flipBoard() }) {
                Label("Flip board", systemImage: "arrow.up.arrow.down")
                    .frame(maxWidth: .infinity)
            }
        }
        .buttonStyle(.bordered)

        // New game is available only when the current game is finished, has
        // no move played yet, or failed (enforce-single-active-game D1): it
        // stays visible but greyed out mid-game and starts nothing when
        // tapped.
        Button(action: {
            // Reset the online handshake so a finished online game's
            // "ready" flag cannot auto-dismiss this opening (D4).
            vm.resetOnlineSheetState()
            showNewGameSheet = true
        }) {
            Label("New game", systemImage: "arrow.counterclockwise")
                .frame(maxWidth: .infinity)
        }
        .buttonStyle(.borderedProminent)
        .disabled(!vm.canStartNewGame)
        .alert("Game over", isPresented: $vm.showGameEndDialog) {
            // Online games offer only "Done" (add-online-multiplayer D4): a
            // rematch would need a new room.
            if !vm.isOnlineMode {
                Button("Play again") { vm.restart() }
            }
            Button("Done", role: .cancel) { vm.dismissGameEnd() }
        } message: {
            Text(vm.gameEndMessage ?? "The game is over.")
        }
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
        vm.playScript(moves, interval: debugScriptInterval)
    }
#endif
}

/// A choice in the new-game setup sheet (add-online-multiplayer D4): the
/// local modes plus the two online intents.
enum NewGameChoice: Equatable {
    case twoPlayers
    case cpu(CpuDifficulty)
    case onlineCreate(timeControl: String)
    case onlineJoin(code: String)
}

/// New-game setup sheet (design D4 + add-online-multiplayer D4): choose the
/// opponent (two players, CPU, or online) and, for the CPU, its difficulty;
/// for online, create a room or join one by code. Two players is
/// pre-selected and keeps the existing behavior.
///
/// Join behavior: the sheet stays open until the server confirms the seat
/// (`ContentView` dismisses it on `onlineSessionReady`) or answers with a
/// room error, which is shown here so the player can fix the code or create
/// a room instead.
struct NewGameSetupView: View {
    enum Mode: Hashable {
        case twoPlayers
        case cpu
        case online
    }

    enum OnlineAction: Hashable {
        case create
        case join
    }

    @State private var mode: Mode = .twoPlayers
    @State private var difficulty: CpuDifficulty = .medium
    @State private var onlineAction: OnlineAction = .create
    @State private var roomCode = ""
    @State private var timeControl: OnlineTimeControl = .default
    /// Whether an online server URL is configured for this build.
    let hasServerURL: Bool
    /// A join-time error from the last attempt (the sheet stays open on it).
    let joinError: String?
    let onStart: (NewGameChoice) -> Void
    @Environment(\.dismiss) private var dismiss

    private var onlineJoinCodeValid: Bool {
        roomCode.uppercased().count == 6
    }

    private var startDisabled: Bool {
        if mode == .online, !hasServerURL {
            return true
        }
        if mode == .online, onlineAction == .join, !onlineJoinCodeValid {
            return true
        }
        return false
    }

    var body: some View {
        VStack(spacing: 20) {
            Text("New game")
                .font(.headline)

            VStack(spacing: 16) {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Opponent")
                        .font(.subheadline)
                        .foregroundColor(.secondary)
                    Picker("Opponent", selection: $mode) {
                        Text("Two players").tag(Mode.twoPlayers)
                        Text("CPU").tag(Mode.cpu)
                        Text("Online").tag(Mode.online)
                    }
                    .pickerStyle(.segmented)
                }

                if mode == .cpu {
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

                if mode == .online {
                    VStack(alignment: .leading, spacing: 6) {
                        Text("Online game")
                            .font(.subheadline)
                            .foregroundColor(.secondary)
                        if hasServerURL {
                            Picker("Online", selection: $onlineAction) {
                                Text("Create").tag(OnlineAction.create)
                                Text("Join").tag(OnlineAction.join)
                            }
                            .pickerStyle(.segmented)
                            if onlineAction == .create {
                                VStack(alignment: .leading, spacing: 6) {
                                    Text("Time control")
                                        .font(.subheadline)
                                        .foregroundColor(.secondary)
                                    Picker("Time control", selection: $timeControl) {
                                        ForEach(OnlineTimeControl.presets) { preset in
                                            Text(preset.label).tag(preset)
                                        }
                                    }
                                    .pickerStyle(.segmented)
                                }
                            }
                            if onlineAction == .join {
                                TextField("Room code (6 characters)", text: $roomCode)
                                    .textFieldStyle(.roundedBorder)
                                    .autocorrectionDisabled()
                                    .textInputAutocapitalization(.characters)
                                if let joinError {
                                    Text(joinError)
                                        .font(.footnote)
                                        .foregroundColor(.red)
                                }
                            }
                        } else {
                            Text("No online server configured for this build.")
                                .font(.footnote)
                                .foregroundColor(.secondary)
                        }
                    }
                }
            }

            Spacer(minLength: 0)

            Button(action: start) {
                Text("Start")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .disabled(startDisabled)

            Button("Cancel") {
                dismiss()
            }
            .font(.footnote)
        }
        .padding()
    }

    private func start() {
        switch mode {
        case .twoPlayers:
            onStart(.twoPlayers)
            dismiss()
        case .cpu:
            onStart(.cpu(difficulty))
            dismiss()
        case .online:
            if onlineAction == .create {
                onStart(.onlineCreate(timeControl: timeControl.label))
                dismiss()
            } else {
                // Stay open: the sheet closes when the server confirms the
                // seat, or shows the join error (see the type's doc).
                onStart(.onlineJoin(code: roomCode.uppercased()))
            }
        }
    }
}
