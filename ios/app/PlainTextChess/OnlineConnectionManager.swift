import Foundation
import Combine

/// Phase of the online session (task 2.2): idle → connecting →
/// waiting → inGame, with reconnecting on a mid-game drop and a terminal
/// failed state.
enum OnlinePhase: Equatable {
    case idle
    case connecting
    /// We hold a seat in a room that is waiting for the opponent.
    case waiting(code: String)
    case inGame
    /// The socket dropped mid-game; re-attaching to the same seat.
    case reconnecting
    case failed(String)

    var isWaiting: Bool {
        if case .waiting = self {
            return true
        }
        return false
    }
}

/// Connection manager for the online protocol (design D2/D4/D7): one
/// `URLSessionWebSocketTask`, the JSON codec, the phase state machine, and
/// automatic re-attach with capped backoff while the server's reconnect
/// window is open. All state changes and callbacks happen on the main
/// thread (the session's delegate queue is the main queue).
final class OnlineConnectionManager: ObservableObject {
    @Published private(set) var phase: OnlinePhase = .idle
    @Published private(set) var roomCode: String?
    @Published private(set) var yourColor: OnlineColor?
    @Published private(set) var reconnecting = false
    /// Human-readable reason the session failed (for the UI).
    @Published private(set) var failureReason: String?

    /// Every state snapshot, including the initial one in `room_ready`.
    var onSnapshot: ((OnlineState) -> Void)?
    /// Incremental in-game updates (server "Game Authority").
    var onUpdate: ((OnlineUpdate) -> Void)?
    /// Structured errors: join-time room errors and transient in-game errors.
    var onError: ((OnlineErrorCode, String) -> Void)?
    /// Phase changes (the view model mirrors these into its own state).
    var onPhase: ((OnlinePhase) -> Void)?
    /// Auth result (add-auth-ui-clients): `true` carries the token issued by
    /// register/login (`nil` for a logout/profile confirmation); `false`
    /// carries the server's error message (e.g. the generic
    /// `invalid_credentials` text).
    var onAuthResult: ((Bool, String?) -> Void)?

    /// Server reconnect window: the server's default grace is 120 s, so we
    /// keep retrying a little past it before giving up.
    static let maxReconnectWindow: TimeInterval = 150

    private let url: URL
    private let deviceID: String
    private let session: URLSession
    private var task: URLSessionWebSocketTask?
    /// What the open socket should be told to do when it connects.
    private var pendingAction: PendingAction = .none
    /// True once the opponent is seated / the game has started.
    private var activeGame = false
    private var retryWorkItem: DispatchWorkItem?
    private var retryBackoff: TimeInterval = 1
    private var retryDeadline: Date?
    /// Session token issued by register/login; used for logout/set_profile.
    private var token: String? = nil
    /// True while a register/login (or later logout/set_profile) reply is
    /// expected, so unrelated structured errors (e.g. a join failure) are
    /// never reported as auth results.
    private var awaitingAuthResult = false

    private enum PendingAction {
        case none
        case create(String?)
        case join(String)
    }

    init(url: URL, deviceID: String) {
        self.url = url
        self.deviceID = deviceID
        // Delegate queue is the main queue: every completion handler and
        // published change runs on the main thread.
        self.session = URLSession(configuration: .default, delegate: nil, delegateQueue: .main)
    }

    deinit {
        retryWorkItem?.cancel()
        task?.cancel(with: .normalClosure, reason: nil)
    }

    // MARK: - Intents

    /// Connect and create a room (the caller takes the White seat) with the
    /// chosen time-control label.
    func startCreating(timeControl: String?) {
        beginAttempt(phase: .connecting, action: .create(timeControl), reconnect: false)
    }

    /// Connect and join an existing room by code.
    func startJoining(code: String) {
        beginAttempt(phase: .connecting, action: .join(code), reconnect: false, code: code)
    }

    /// Re-attach to an in-progress room after a drop or an app relaunch:
    /// the same device identifier re-enters the seat it held. The first
    /// attempt goes through the retry schedule (1 s) instead of connecting
    /// immediately: that gives the "Reconnecting…" banner a visible window
    /// and avoids racing the server's disconnect bookkeeping.
    func startReattaching(code: String) {
        activeGame = true
        beginAttempt(phase: .reconnecting,
                     action: .join(code),
                     reconnect: true,
                     code: code,
                     immediate: false)
    }

    func sendMove(_ uci: String) {
        send(.move(uci: uci))
    }

    func resign() {
        send(.resign)
    }

    func leave() {
        send(.leave)
    }

    // MARK: - Auth (add-auth-ui-clients)

    /// Open a plain connection with no room, so `register`/`login` can run
    /// before any game. No `create_room`/`join_room` is sent: guest play
    /// (`device_id`) and the seated-flag handshake stay untouched.
    func startAuth() {
        beginAttempt(phase: .connecting, action: .none, reconnect: false)
    }

    /// Send a register request; the reply arrives as `onAuthResult(true,
    /// token)` or `onAuthResult(false, message)` (e.g. `username_taken`).
    func startRegister(username: String, password: String) {
        awaitingAuthResult = true
        send(.register(username: username, password: password, deviceID: deviceID))
    }

    /// Send a login request; the reply arrives as `onAuthResult(true,
    /// token)` or `onAuthResult(false, message)` (`invalid_credentials`
    /// carries the server's generic message).
    func startLogin(username: String, password: String) {
        awaitingAuthResult = true
        send(.login(username: username, password: password, deviceID: deviceID))
    }

    /// Send a logout request; the reply is `session_ok` → `onAuthResult(
    /// true, nil)`. The server only revokes the token here: the socket is
    /// kept open, so guest play can continue on this connection.
    func startLogout(token: String) {
        awaitingAuthResult = true
        send(.logout(token: token))
    }

    /// Send a set_profile request; the reply is `profile_updated` →
    /// `onAuthResult(true, nil)` or a refusal (`not_authenticated`,
    /// `invalid_display_name`) → `onAuthResult(false, message)`. The token
    /// is filled in from the manager's stored value.
    func startSetProfile(displayName: String) {
        awaitingAuthResult = true
        send(.setProfile(token: "", displayName: displayName))
    }

    /// Close the session and clear all state (the caller owns this manager
    /// from here on; callbacks stop).
    func teardown() {
        retryWorkItem?.cancel()
        retryWorkItem = nil
        retryDeadline = nil
        pendingAction = .none
        awaitingAuthResult = false
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
        activeGame = false
        reconnecting = false
        roomCode = nil
        yourColor = nil
        failureReason = nil
        setPhase(.idle)
    }

    // MARK: - Connection lifecycle

    private func beginAttempt(phase newPhase: OnlinePhase,
                              action: PendingAction,
                              reconnect: Bool,
                              code: String? = nil,
                              immediate: Bool = true) {
        // A fresh attempt: clear any previous attempt's state.
        retryWorkItem?.cancel()
        retryWorkItem = nil
        retryBackoff = 1
        retryDeadline = nil
        pendingAction = action
        roomCode = code
        yourColor = nil
        failureReason = nil
        if reconnect {
            reconnecting = true
        }
        setPhase(newPhase)
        if immediate {
            connect()
        } else {
            beginReconnect()
        }
    }

    private func connect() {
        let task = session.webSocketTask(with: URLRequest(url: url))
        self.task = task
        task.resume()
        // Sends are queued until the socket is established, so the intent can
        // be fired right after resume (create on first connect, join on a
        // re-attach attempt).
        switch pendingAction {
        case .create(let timeControl):
            send(.createRoom(playerID: deviceID, timeControl: timeControl))
        case .join(let code):
            send(.joinRoom(playerID: deviceID, roomCode: code))
        case .none:
            break
        }
        receiveLoop(on: task)
    }

    /// `URLSessionWebSocketTask.receive` is one-shot: each completion must
    /// re-arm the next read to keep the frame loop alive.
    private func receiveLoop(on task: URLSessionWebSocketTask) {
        task.receive { [weak self] result in
            // If a newer socket replaced this one (re-attach), this callback
            // belongs to a dead task: ignore it.
            guard let self, self.task === task else { return }
            switch result {
            case .success(let message):
                self.handle(incoming: message)
            case .failure:
                self.handleSocketDrop()
            }
            self.receiveLoop(on: task)
        }
    }

    private func setPhase(_ phase: OnlinePhase) {
        self.phase = phase
        onPhase?(phase)
    }

    // MARK: - Incoming frames

    /// Serial queue for JSON decode, off the main thread; the ordered hop back
    /// to main keeps frame order.
    private let decodeQueue = DispatchQueue(label: "com.plaintextchess.online.decode")

    private func handle(incoming message: URLSessionWebSocketTask.Message) {
        switch message {
        case .string(let json):
            decodeQueue.async { [weak self] in
                guard let self else { return }
                let decoded: OnlineServerMessage?
                do {
                    decoded = try OnlineCodec.decodeServer(json)
                } catch {
                    decoded = nil
                }
                DispatchQueue.main.async {
                    if let decoded {
                        self.handle(serverMessage: decoded)
                    } else {
                        // A malformed frame from the server is a protocol
                        // failure: treat it like a drop.
                        self.handleSocketDrop()
                    }
                }
            }
        default:
            // Binary frames are never sent by the server; a server-side close
            // arrives as a receive failure, handled by the drop path.
            break
        }
    }

    private func handle(serverMessage: OnlineServerMessage) {
        switch serverMessage {
        case .roomReady(let code, let color, let state):
            roomCode = code
            yourColor = color
            reconnecting = false
            failureReason = nil
            retryWorkItem?.cancel()
            retryWorkItem = nil
            retryDeadline = nil
            switch pendingAction {
            case .create:
                setPhase(.waiting(code: code))
            case .join:
                activeGame = true
                setPhase(.inGame)
            case .none:
                break
            }
            onSnapshot?(state)

        case .state(let state):
            reconnecting = false
            yourColor = state.yourColor
            if state.opponentOnline || !state.moveList.isEmpty {
                activeGame = true
                // A started game settles the phase even mid-attach: the
                // server answers a re-attach with this resync snapshot (no
                // room_ready), so waiting for room_ready would leave the
                // "Reconnecting…" banner up forever.
                setPhase(.inGame)
            }
            onSnapshot?(state)

        case .update(let update):
            reconnecting = false
            activeGame = true
            setPhase(.inGame)
            onUpdate?(update)

        case .error(let code, let message):
            switch code {
            case .roomNotFound, .roomFull, .invalidRoomCode, .alreadyInRoom:
                // Definitive for the join in flight: the room is gone or
                // unavailable, so stop retrying and report the error.
                terminate(with: message, code: code)
            default:
                // Transient in-game error: surface it, keep the session.
                onError?(code, message)
            }
            if awaitingAuthResult {
                // A refused register/login (e.g. `invalid_credentials`):
                // report the server's message and stop expecting a reply.
                awaitingAuthResult = false
                onAuthResult?(false, message)
            }
        case .session(let tokenStr):
            self.token = tokenStr
            if awaitingAuthResult {
                awaitingAuthResult = false
                onAuthResult?(true, tokenStr)
            }
        case .profileUpdated:
            if awaitingAuthResult {
                awaitingAuthResult = false
                onAuthResult?(true, nil)
            }
        case .sessionOk:
            if awaitingAuthResult {
                awaitingAuthResult = false
                onAuthResult?(true, nil)
            }
        }
    }

    /// The socket went away (failure, close frame, or a malformed frame).
    private func handleSocketDrop() {
        // Late events after teardown or a definitive failure: ignore.
        if case .idle = phase { return }
        if case .failed = phase { return }
        task?.cancel(with: .normalClosure, reason: nil)
        task = nil
        pendingAction = .none

        if roomCode != nil {
            // A room exists for this device: keep re-attaching while the
            // server's window is open. In a dropped lobby the first retry
            // is answered with `room_not_found` and stops.
            beginReconnect()
        } else {
            terminate(with: "Connection lost", code: .notConnected)
        }
    }

    // MARK: - Reconnect

    private func beginReconnect() {
        reconnecting = true
        setPhase(.reconnecting)
        if retryDeadline == nil {
            retryDeadline = Date().addingTimeInterval(Self.maxReconnectWindow)
        }
        scheduleRetry()
    }

    private func scheduleRetry() {
        retryWorkItem?.cancel()
        guard let code = roomCode else {
            terminate(with: "Connection lost", code: .notConnected)
            return
        }
        let delay = retryBackoff
        retryBackoff = min(retryBackoff * 2, 8)
        let item = DispatchWorkItem { [weak self] in
            guard let self else { return }
            guard case .reconnecting = self.phase else { return }
            guard let deadline = self.retryDeadline, Date() < deadline else {
                self.terminate(with: "Could not reconnect in time", code: .notConnected)
                return
            }
            self.pendingAction = .join(code)
            self.connect()
        }
        retryWorkItem = item
        DispatchQueue.main.asyncAfter(deadline: .now() + delay, execute: item)
    }

    // MARK: - Outgoing frames

    private func send(_ message: OnlineClientMessage) {
        guard let task else { return }
        let msgToSend: OnlineClientMessage = switch message {
        case .logout(let token):
            .logout(token: self.token ?? "")
        case .setProfile(let token, let displayName):
            .setProfile(token: self.token ?? "", displayName: displayName)
        default:
            message
        }
        guard let json = try? OnlineCodec.encodeClient(msgToSend) else { return }
        task.send(.string(json)) { [weak self] error in
            // A send failure surfaces as a drop in the receive loop; nothing
            // to do here besides not crashing on the error value.
            _ = error
            _ = self
        }
    }

    // MARK: - Failure

    private func terminate(with reason: String, code: OnlineErrorCode) {
        retryWorkItem?.cancel()
        retryWorkItem = nil
        retryDeadline = nil
        task?.cancel(with: .normalClosure, reason: nil)
        task = nil
        pendingAction = .none
        awaitingAuthResult = false
        activeGame = false
        reconnecting = false
        roomCode = nil
        failureReason = reason
        setPhase(.failed(reason))
        onError?(code, reason)
    }
}
