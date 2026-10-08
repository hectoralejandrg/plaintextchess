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

    enum Route: Hashable {
        case home
        case game
    }

    @State private var path: [Route] = []

    var body: some View {
        NavigationStack(path: $path) {
            LoginView(vm: vm, effectiveOnlineURL: effectiveOnlineURL,
                      onAuth: { path.append(.home) },
                      onGuest: { path.append(.home) })
                .navigationDestination(for: Route.self) { route in
                    switch route {
                    case .home:
                        HomeView(vm: vm, effectiveOnlineURL: effectiveOnlineURL,
                                 onStartGame: { path.append(.game) },
                                 onLogout: {
                                     vm.doLogout(serverURLString: effectiveOnlineURL)
                                     path.removeAll()
                                 },
                                 onGoToLogin: { path.removeAll() })
                    case .game:
                        GameView(vm: vm, effectiveOnlineURL: effectiveOnlineURL)
                    }
                }
                .onAppear {
                    vm.restoreOnlineSessionIfNeeded(serverURLString: effectiveOnlineURL)
                    #if DEBUG
                    vm.setDebugCpuDelay(debugCpuDelay)
                    vm.setDebugOnlineURL(effectiveOnlineURL)
                    if case .cpu = debugMode {
                        vm.startGame(debugMode)
                    }
                    startDebugScriptIfNeeded()
                    #endif
                }
        }
        .onChange(of: vm.authToken) { token in
            if token != nil && path.last != .home {
                path.append(.home)
            }
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
    case register(username: String, password: String)
    case login(username: String, password: String)
    case logout
    case setProfile(displayName: String)
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
    /// Sign-in fields (add-auth-ui-clients): kept while the sheet reports a
    /// failure so the player can correct them without retyping.
    @State private var authUsername = ""
    @State private var authPassword = ""
    /// Profile display-name field (task 3.2): submitted via `set_profile`.
    @State private var authDisplayNameInput = ""
    /// Whether an online server URL is configured for this build.
    let hasServerURL: Bool
    /// A join-time error from the last attempt (the sheet stays open on it).
    let joinError: String?
    /// The session token after a successful register/login (nil = guest).
    let authToken: String?
    /// The display name recorded after a `profile_updated` reply (task 3.2).
    let authDisplayName: String?
    /// A register/login failure from the last attempt (the form stays open
    /// on it, e.g. the server's generic invalid_credentials message).
    let authError: String?
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

            accountSection

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

    /// Optional sign-in section (add-auth-ui-clients): username/password
    /// with Register and Login while a guest; once the server issues a
    /// token, the same section shows the session instead. Failures
    /// (username_taken, invalid_credentials) render here and keep the form
    /// open, mirroring how join errors keep the join form open.
    @ViewBuilder
    private var accountSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Account")
                .font(.subheadline)
                .foregroundColor(.secondary)
            if let authToken {
                HStack(alignment: .top, spacing: 8) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Signed in")
                            .font(.callout.bold())
                        Text(authToken)
                            .font(.caption.monospaced())
                            .textSelection(.enabled)
                    }
                    Spacer()
                    Button("Copy") {
                        UIPasteboard.general.string = authToken
                    }
                    .buttonStyle(.bordered)
                    Button("Log out") {
                        onStart(.logout)
                    }
                    .buttonStyle(.bordered)
                }
                .padding(10)
                .background(.thinMaterial, in: RoundedRectangle(cornerRadius: 10))
                if let authDisplayName {
                    Text("Display name: \(authDisplayName)")
                        .font(.footnote)
                        .foregroundColor(.secondary)
                }
                HStack(spacing: 8) {
                    TextField("Display name", text: $authDisplayNameInput)
                        .textFieldStyle(.roundedBorder)
                        .autocorrectionDisabled()
                    Button("Update") {
                        onStart(.setProfile(displayName: authDisplayNameInput))
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(authDisplayNameInput.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
                if let authError {
                    Text(authError)
                        .font(.footnote)
                        .foregroundColor(.red)
                }
            } else {
                TextField("Username", text: $authUsername)
                    .textFieldStyle(.roundedBorder)
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                SecureField("Password", text: $authPassword)
                    .textFieldStyle(.roundedBorder)
                HStack(spacing: 8) {
                    Button(action: {
                        onStart(.register(username: authUsername,
                                          password: authPassword))
                    }) {
                        Text("Register")
                            .frame(maxWidth: .infinity)
                    }
                    Button(action: {
                        onStart(.login(username: authUsername,
                                       password: authPassword))
                    }) {
                        Text("Login")
                            .frame(maxWidth: .infinity)
                    }
                }
                .buttonStyle(.borderedProminent)
                .disabled(authUsername.isEmpty || authPassword.isEmpty)
                if let authError {
                    Text(authError)
                        .font(.footnote)
                        .foregroundColor(.red)
                }
            }
        }
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

struct LoginView: View {
    @ObservedObject var vm: GameViewModel
    let effectiveOnlineURL: String?
    let onAuth: () -> Void
    let onGuest: () -> Void

    @State private var username = ""
    @State private var password = ""

    var body: some View {
        VStack(spacing: 16) {
            Text("PlainTextChess")
                .font(.largeTitle.bold())
                .padding(.bottom, 20)

            TextField("Username", text: $username)
                .textFieldStyle(.roundedBorder)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)

            SecureField("Password", text: $password)
                .textFieldStyle(.roundedBorder)

            HStack(spacing: 8) {
                Button(action: {
                    vm.doRegister(username: username, password: password, serverURLString: effectiveOnlineURL)
                }) {
                    Text("Register")
                        .frame(maxWidth: .infinity)
                }
                .disabled(username.isEmpty || password.isEmpty)
                .buttonStyle(.borderedProminent)

                Button(action: {
                    vm.doLogin(username: username, password: password, serverURLString: effectiveOnlineURL)
                }) {
                    Text("Login")
                        .frame(maxWidth: .infinity)
                }
                .disabled(username.isEmpty || password.isEmpty)
                .buttonStyle(.borderedProminent)
            }

            Button(action: { onGuest() }) {
                Text("Play as guest")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.bordered)

            if let error = vm.authError {
                Text(error)
                    .font(.footnote)
                    .foregroundColor(.red)
            }
        }
        .padding()
        .onChange(of: vm.authToken) { token in
            if token != nil {
                onAuth()
            }
        }
    }
}

struct HomeView: View {
    @ObservedObject var vm: GameViewModel
    let effectiveOnlineURL: String?
    let onStartGame: () -> Void
    let onLogout: () -> Void
    let onGoToLogin: () -> Void
    @State private var isCpu = false
    @State private var isOnline = false
    @State private var difficulty: CpuDifficulty = .medium
    @State private var roomCode = ""
    @State private var timeControl: OnlineTimeControl = .default
    @State private var authDisplayNameInput = ""

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                if vm.authToken != nil {
                    Text("Signed in")
                        .font(.headline)
                    if let name = vm.authDisplayName {
                        Text("Display name: \(name)")
                            .font(.footnote)
                            .foregroundColor(.secondary)
                    }
                    HStack(spacing: 8) {
                        TextField("Display name", text: $authDisplayNameInput)
                            .textFieldStyle(.roundedBorder)
                            .autocorrectionDisabled()
                        Button("Update") {
                            vm.doSetProfile(displayName: authDisplayNameInput, serverURLString: effectiveOnlineURL)
                        }
                        .disabled(authDisplayNameInput.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        .buttonStyle(.borderedProminent)
                    }
                    Button(action: onLogout) {
                        Text("Logout")
                    }
                    .buttonStyle(.bordered)
                } else {
                    Text("Playing as guest")
                        .font(.headline)
                    Button(action: onGoToLogin) {
                        Text("Login / Register")
                    }
                    .buttonStyle(.bordered)
                }
                if let error = vm.authError {
                    Text(error)
                        .font(.footnote)
                        .foregroundColor(.red)
                }

                Text("Mode selection")
                    .font(.headline)
                Picker("Opponent", selection: $isCpu) {
                    Text("Two players").tag(false)
                    Text("CPU").tag(true)
                }
                .pickerStyle(.segmented)
                .onChange(of: isCpu) { new in if new { isOnline = false } }

                if isCpu {
                    Picker("Difficulty", selection: $difficulty) {
                        ForEach(CpuDifficulty.allCases, id: \.self) { level in
                            Text(level.displayName).tag(level)
                        }
                    }
                    .pickerStyle(.segmented)
                }

                Picker("Online", selection: $isOnline) {
                    Text("Local").tag(false)
                    Text("Online").tag(true)
                }
                .pickerStyle(.segmented)
                .onChange(of: isOnline) { new in if new { isCpu = false } }

                if isOnline {
                    Picker("Time control", selection: $timeControl) {
                        ForEach(OnlineTimeControl.presets) { preset in
                            Text(preset.label).tag(preset)
                        }
                    }
                    .pickerStyle(.segmented)
                    TextField("Room code (6 chars)", text: $roomCode)
                        .textFieldStyle(.roundedBorder)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.characters)
                    HStack(spacing: 8) {
                        Button(action: {
                            // `startOnlineGame` opens the connection and asks the
                            // server for a room; on success move to the Game
                            // section so the player watches the board while the
                            // room is prepared (parity with Android HomeView).
                            if vm.startOnlineGame(create: true, code: nil, timeControl: timeControl.label, serverURLString: effectiveOnlineURL) {
                                onStartGame()
                            }
                        }) {
                            Text("Create room").frame(maxWidth: .infinity)
                        }
                        .buttonStyle(.borderedProminent)
                        Button(action: {
                            let code = roomCode.uppercased()
                            if code.count == 6,
                               vm.startOnlineGame(create: false, code: code, timeControl: nil, serverURLString: effectiveOnlineURL) {
                                onStartGame()
                            }
                        }) {
                            Text("Join room").frame(maxWidth: .infinity)
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(roomCode.uppercased().count != 6)
                    }
                    if let joinErr = vm.onlineJoinError {
                        Text(joinErr).font(.footnote).foregroundColor(.red)
                    }
                } else {
                    Button(action: {
                        if isCpu {
                            vm.startGame(.cpu(difficulty))
                        } else {
                            vm.startGame(.twoPlayers)
                        }
                        onStartGame()
                    }) {
                        Text("Start game").frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.borderedProminent)
                }
            }
            .padding()
        }
    }
}

struct GameView: View {
    @ObservedObject var vm: GameViewModel
    let effectiveOnlineURL: String?

    @State private var showNewGameSheet = false
    private let debugAnimScale = 1.0
    private let debugDrag: DragTest? = nil

    var body: some View {
        GeometryReader { proxy in
            let boardSide = proxy.size.width
            VStack(alignment: .leading, spacing: 10) {
                Text("PlainTextChess")
                    .font(.largeTitle.bold())
                    .padding(.horizontal)

                StatusRowView(vm: vm)
                    .padding(.horizontal)

                if vm.isOnlineMode {
                    ClockRowView(vm: vm)
                        .padding(.horizontal)
                }

                if case .waiting(let code) = vm.onlinePhase {
                    WaitingBannerView(code: code)
                        .padding(.horizontal)
                }

                BoardView(vm: vm,
                          animScale: debugAnimScale,
                          dragTest: debugDrag)
                    .frame(width: boardSide, height: boardSide)
                    .overlay {
                        if vm.onlineReconnecting {
                            Text("Reconnecting…")
                                .font(.callout.bold())
                                .padding(.horizontal, 14)
                                .padding(.vertical, 8)
                                .background(.thinMaterial, in: Capsule())
                        }
                    }

                MoveListView(vm: vm)
                    .padding(.horizontal)

                ControlsView(vm: vm)
                    .padding(.horizontal)
            }
            .padding(.vertical)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        }
    }
}

struct StatusRowView: View {
    @ObservedObject var vm: GameViewModel

    var body: some View {
        HStack(spacing: 8) {
            if vm.onlineReconnecting {
                Text("Reconnecting…").font(.headline).foregroundColor(.orange)
            } else if case .waiting = vm.onlinePhase {
                Text("Waiting for opponent to join…").font(.headline)
            } else if vm.cpuThinking {
                Text("CPU is thinking…").font(.headline)
            } else {
                switch vm.status {
                case .starting:
                    Text(vm.isOnlineMode ? "Connecting to online server…" : "Creating session…").font(.headline)
                case .playing(let toMove, let inCheck):
                    if vm.isOnlineMode, toMove == vm.onlineYourColor {
                        Text("Your move").font(.headline.bold())
                    } else if vm.isOnlineMode {
                        Text("Opponent to move").font(.headline)
                    } else {
                        Text(toMove == "w" ? "White to move" : "Black to move").font(.headline)
                    }
                    if inCheck {
                        Text("— Check!").font(.headline).foregroundColor(.red)
                    }
                case .checkmated(let winner):
                    Text("Checkmate! \(winner) wins").font(.headline)
                case .drawn:
                    Text("Game drawn").font(.headline)
                case .resigned(let winner):
                    Text((winner == "White" ? "Black" : "White") + " resigns").font(.headline)
                case .forfeited(let winner):
                    Text((winner == "White" ? "Black" : "White") + " forfeits").font(.headline)
                case .timedOut(let winner):
                    Text((winner == "White" ? "Black" : "White") + " ran out of time").font(.headline)
                case .failed:
                    Text("Game unavailable").font(.headline).foregroundColor(.red)
                }
            }
            Spacer()
        }
    }
}

struct ClockRowView: View {
    @ObservedObject var vm: GameViewModel

    var body: some View {
        if let clock = vm.onlineClock {
            TimelineView(.periodic(from: .now, by: 0.1)) { context in
                let white = clock.remaining(for: "w", now: context.date)
                let black = clock.remaining(for: "b", now: context.date)
                let topColor = vm.boardOrientation == 0 ? "Black" : "White"
                let bottomColor = vm.boardOrientation == 0 ? "White" : "Black"
                HStack(spacing: 8) {
                    ClockCellView(color: topColor,
                                 milliseconds: topColor == "White" ? white : black,
                                 active: clock.isRunning && clock.sideToMove == (topColor == "White" ? "w" : "b"))
                    Spacer()
                    Text(vm.onlineTimeControl)
                        .font(.caption.monospaced())
                        .foregroundColor(.secondary)
                    Spacer()
                    ClockCellView(color: bottomColor,
                                 milliseconds: bottomColor == "White" ? white : black,
                                 active: clock.isRunning && clock.sideToMove == (bottomColor == "White" ? "w" : "b"))
                }
                .frame(height: 34)
            }
        }
    }
}

struct ClockCellView: View {
    let color: String
    let milliseconds: Int
    let active: Bool

    var body: some View {
        let low = milliseconds <= 10_000
        return VStack(spacing: 0) {
            Text(color).font(.caption2).foregroundColor(.secondary)
            Text(OnlineClock.format(milliseconds)).font(.headline.monospacedDigit()).foregroundColor(low ? .red : .primary)
        }
        .frame(minWidth: 74)
        .padding(.vertical, 2)
        .background(RoundedRectangle(cornerRadius: 8).fill(active ? Color.accentColor.opacity(0.18) : Color.clear))
    }
}

struct WaitingBannerView: View {
    let code: String

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Room code").font(.caption).foregroundColor(.secondary)
                Text(code).font(.title3.monospaced().bold())
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
}

struct MoveListView: View {
    @ObservedObject var vm: GameViewModel

    var body: some View {
        if vm.moveList.isEmpty {
            Text("No moves yet").font(.caption).foregroundColor(.secondary)
        } else {
            ScrollView {
                LazyVGrid(columns: [GridItem(.flexible(), alignment: .leading), GridItem(.flexible(), alignment: .leading)], spacing: 4) {
                    ForEach(Array(vm.moveList.enumerated()), id: \.offset) { index, uci in
                        HStack(spacing: 4) {
                            if index.isMultiple(of: 2) {
                                Text("\(index / 2 + 1).").font(.caption.monospacedDigit())
                            }
                            Text(uci).font(.caption.monospaced())
                        }
                    }
                }
            }
            .frame(maxHeight: 96)
        }
    }
}

struct ControlsView: View {
    @ObservedObject var vm: GameViewModel
    @State private var showNewGameSheet = false

    var body: some View {
        VStack(spacing: 8) {
            if let error = vm.errorMessage {
                Text(error).font(.footnote).foregroundColor(.red)
            }
            HStack(spacing: 8) {
                Button(action: { vm.undo() }) {
                    Label("Undo", systemImage: "arrow.uturn.backward").frame(maxWidth: .infinity)
                }.disabled(!vm.canUndo).buttonStyle(.bordered)

                Button(action: { vm.resign() }) {
                    Label("Resign", systemImage: "flag").frame(maxWidth: .infinity)
                }.disabled(!vm.canResign).buttonStyle(.bordered)

                Button(action: { vm.flipBoard() }) {
                    Label("Flip board", systemImage: "arrow.up.arrow.down").frame(maxWidth: .infinity)
                }.buttonStyle(.bordered)
            }
            Button(action: { showNewGameSheet = true }) {
                Label("New game", systemImage: "arrow.counterclockwise").frame(maxWidth: .infinity)
            }.buttonStyle(.borderedProminent).disabled(!vm.canStartNewGame)
        }
        .alert("Game over", isPresented: $vm.showGameEndDialog) {
            if !vm.isOnlineMode {
                Button("Play again") { vm.restart() }
            }
            Button("Done", role: .cancel) { vm.dismissGameEnd() }
        } message: {
            Text(vm.gameEndMessage ?? "The game is over.")
        }
    }
}
