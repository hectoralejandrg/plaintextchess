import Foundation

/// Version of the online multiplayer protocol carried by every message
/// (shared spec "Online Multiplayer Protocol", design D2).
let kOnlineProtocolVersion = 1

/// Online server configuration (add-online-multiplayer D9): the URL is a
/// build-time constant; DEBUG builds may override it with
/// `-PLAINTCHESS_ONLINE_URL <url>`.
enum OnlineConfig {
    /// Default online server URL. Empty in this change: online play is
    /// verified through the DEBUG override.
    static let defaultURL = ""
}

/// Colors on the wire (`"white"` / `"black"`).
enum OnlineColor: String, Codable, Equatable, CaseIterable {
    case white
    case black

    /// The board's side-to-move letter for this color.
    var sideToMoveSquare: String {
        self == .white ? "w" : "b"
    }

    var displayName: String {
        self == .white ? "White" : "Black"
    }
}

/// Game status as reported by the server. Wire form (externally tagged,
/// pinned by the server's `wire_form_is_stable` test): unit variants are
/// plain strings (`"playing"`, `"drawn"`), struct variants are
/// `{"checkmated": {"winner": "white"}}`.
enum OnlineStatus: Equatable {
    case playing
    case checkmated(winner: OnlineColor)
    case drawn
    case resigned(winner: OnlineColor)
    case forfeited(winner: OnlineColor)
    case timedOut(winner: OnlineColor)

    var isTerminal: Bool {
        if case .playing = self {
            return false
        }
        return true
    }

    /// The winner of a decisive result (`nil` while playing or drawn).
    var winner: OnlineColor? {
        switch self {
        case .checkmated(let winner), .resigned(let winner), .forfeited(let winner),
             .timedOut(let winner):
            return winner
        case .playing, .drawn:
            return nil
        }
    }

    /// Whether this ending was a flag fall (a loss/won on time).
    var isFlagFall: Bool {
        if case .timedOut = self {
            return true
        }
        return false
    }
}

extension OnlineStatus: Codable {
    private enum OuterKey: String, CodingKey {
        case playing
        case checkmated
        case drawn
        case resigned
        case forfeited
        case timedOut = "timed_out"
    }

    private enum WinnerKey: CodingKey {
        case winner
    }

    init(from decoder: Decoder) throws {
        // Unit variants are plain strings.
        if let single = try? decoder.singleValueContainer(),
           let name = try? single.decode(String.self)
        {
            switch name {
            case "playing":
                self = .playing
            case "drawn":
                self = .drawn
            default:
                throw OnlineProtocolError.invalidStatus(name)
            }
            return
        }
        // Struct variants: `{"<variant>": {"winner": <color>}}`.
        let container = try decoder.container(keyedBy: OuterKey.self)
        if let winner = try? container.nestedContainer(keyedBy: WinnerKey.self, forKey: .checkmated)
            .decode(OnlineColor.self, forKey: .winner)
        {
            self = .checkmated(winner: winner)
            return
        }
        if let winner = try? container.nestedContainer(keyedBy: WinnerKey.self, forKey: .resigned)
            .decode(OnlineColor.self, forKey: .winner)
        {
            self = .resigned(winner: winner)
            return
        }
        if let winner = try? container.nestedContainer(keyedBy: WinnerKey.self, forKey: .forfeited)
            .decode(OnlineColor.self, forKey: .winner)
        {
            self = .forfeited(winner: winner)
            return
        }
        if let winner = try? container.nestedContainer(keyedBy: WinnerKey.self, forKey: .timedOut)
            .decode(OnlineColor.self, forKey: .winner)
        {
            self = .timedOut(winner: winner)
            return
        }
        if container.contains(.drawn) {
            self = .drawn
            return
        }
        if container.contains(.playing) {
            self = .playing
            return
        }
        throw OnlineProtocolError.invalidStatus("")
    }

    func encode(to encoder: Encoder) throws {
        switch self {
        case .playing:
            var single = encoder.singleValueContainer()
            try single.encode("playing")
        case .drawn:
            var single = encoder.singleValueContainer()
            try single.encode("drawn")
        case .checkmated(let winner):
            var container = encoder.container(keyedBy: OuterKey.self)
            var nested = container.nestedContainer(keyedBy: WinnerKey.self, forKey: .checkmated)
            try nested.encode(winner, forKey: .winner)
        case .resigned(let winner):
            var container = encoder.container(keyedBy: OuterKey.self)
            var nested = container.nestedContainer(keyedBy: WinnerKey.self, forKey: .resigned)
            try nested.encode(winner, forKey: .winner)
        case .forfeited(let winner):
            var container = encoder.container(keyedBy: OuterKey.self)
            var nested = container.nestedContainer(keyedBy: WinnerKey.self, forKey: .forfeited)
            try nested.encode(winner, forKey: .winner)
        case .timedOut(let winner):
            var container = encoder.container(keyedBy: OuterKey.self)
            var nested = container.nestedContainer(keyedBy: WinnerKey.self, forKey: .timedOut)
            try nested.encode(winner, forKey: .winner)
        }
    }
}

/// Full state snapshot: one of these rebuilds the entire game screen
/// (spec scenario "A state snapshot rebuilds the game screen").
struct OnlineState: Codable, Equatable {
    let boardFen: String
    let moveList: [String]
    /// Side to move: `"w"` or `"b"`.
    let sideToMove: String
    let status: OnlineStatus
    let yourColor: OnlineColor
    let whiteRating: Double
    let blackRating: Double
    let opponentOnline: Bool
    /// The room's time control label (e.g. `"15+10"`).
    let timeControl: String
    /// White's remaining time in milliseconds (server-authoritative).
    let whiteTimeMs: Int
    /// Black's remaining time in milliseconds (server-authoritative).
    let blackTimeMs: Int

    private enum CodingKeys: String, CodingKey {
        case boardFen = "board_fen"
        case moveList = "move_list"
        case sideToMove = "side_to_move"
        case status
        case yourColor = "your_color"
        case whiteRating = "white_rating"
        case blackRating = "black_rating"
        case opponentOnline = "opponent_online"
        case timeControl = "time_control"
        case whiteTimeMs = "white_time_ms"
        case blackTimeMs = "black_time_ms"
    }
}

/// Stable machine-readable error codes (spec "Errors carry a stable code").
enum OnlineErrorCode: String, Codable, Equatable, CaseIterable {
    case roomNotFound = "room_not_found"
    case roomFull = "room_full"
    case alreadyInRoom = "already_in_room"
    case invalidRoomCode = "invalid_room_code"
    case notYourTurn = "not_your_turn"
    case illegalMove = "illegal_move"
    case gameOver = "game_over"
    case notConnected = "not_connected"
    case forfeit = "forfeit"
    /// Authentication codes (add-auth-ui-clients); the client must know them
    /// or an auth error frame would fail to decode and drop the connection.
    case usernameTaken = "username_taken"
    case invalidCredentials = "invalid_credentials"
    case notAuthenticated = "not_authenticated"
    case sessionExpired = "session_expired"
    case invalidRequest = "invalid_request"
    case invalidDisplayName = "invalid_display_name"

    /// Human-readable fallback when the server's message is missing. The auth
    /// entries mirror the server's texts (`ErrorCode::message`), so a refused
    /// login shows the same generic message the spec pins.
    var displayMessage: String {
        switch self {
        case .roomNotFound:
            return "No open room has that code"
        case .roomFull:
            return "That room already has two players"
        case .alreadyInRoom:
            return "This player is already in a room"
        case .invalidRoomCode:
            return "Room codes are 6 characters from A-Z and 2-9 (no I or O)"
        case .notYourTurn:
            return "It is not your turn"
        case .illegalMove:
            return "That move is not legal in the current position"
        case .gameOver:
            return "The game is already over"
        case .notConnected:
            return "The game has not started yet"
        case .forfeit:
            return "The opponent did not reconnect in time"
        case .usernameTaken:
            return "That username is already registered"
        case .invalidCredentials:
            return "That username and password do not match an account"
        case .notAuthenticated:
            return "This connection is not signed in"
        case .sessionExpired:
            return "This session has expired; log in again"
        case .invalidRequest:
            return "A field failed validation"
        case .invalidDisplayName:
            return "A display name must be 1 to 32 characters with no control characters"
        }
    }
}

/// Protocol-level failures (spec: the client treats these as connection
/// errors; the server is the one that closes with close code 4000).
enum OnlineProtocolError: Error, Equatable {
    case invalidJSON
    case unknownMessageType(String)
    case unsupportedVersion(Int)
    case invalidStatus(String)
}

/// A time-control preset (spec "Server Time Control and Clock"): base time
/// plus a Fischer increment, mirrored from the server's supported set. The
/// `label` is the exact wire string sent in `create_room`.
struct OnlineTimeControl: Equatable, Hashable, Identifiable {
    let label: String
    let baseSeconds: Int
    let incrementSeconds: Int

    var id: String { label }

    static let presets: [OnlineTimeControl] = [
        OnlineTimeControl(label: "15+10", baseSeconds: 15 * 60, incrementSeconds: 10),
        OnlineTimeControl(label: "10+0", baseSeconds: 10 * 60, incrementSeconds: 0),
        OnlineTimeControl(label: "5+0", baseSeconds: 5 * 60, incrementSeconds: 0),
        OnlineTimeControl(label: "3+2", baseSeconds: 3 * 60, incrementSeconds: 2),
        OnlineTimeControl(label: "1+0", baseSeconds: 1 * 60, incrementSeconds: 0),
    ]

    /// The default selection when the create screen opens.
    static let `default` = presets[0]
}

/// Client messages (spec: "Online Multiplayer Protocol"): versioned JSON
/// with a `type` tag, snake_case fields.
enum OnlineClientMessage: Codable, Equatable {
    case createRoom(playerID: String, timeControl: String?)
    case joinRoom(playerID: String, roomCode: String)
    case move(uci: String)
    case register(username: String, password: String, deviceID: String?)
    case login(username: String, password: String, deviceID: String?)
    case logout(token: String)
    case setProfile(token: String, displayName: String)
    case resign
    case leave

    private enum CodingKeys: String, CodingKey {
        case type
        case v
        case playerID = "player_id"
        case roomCode = "room_code"
        case timeControl = "time_control"
        case username
        case password
        case token
        case displayName = "display_name"
        case deviceID = "device_id"
        case uci
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let type = try container.decode(String.self, forKey: .type)
        let version = try container.decode(Int.self, forKey: .v)
        guard version == kOnlineProtocolVersion else {
            throw OnlineProtocolError.unsupportedVersion(version)
        }
        switch type {
        case "create_room":
            let playerID = try container.decode(String.self, forKey: .playerID)
            let timeControl = try container.decodeIfPresent(String.self, forKey: .timeControl)
            self = .createRoom(playerID: playerID, timeControl: timeControl)
        case "join_room":
            let playerID = try container.decode(String.self, forKey: .playerID)
            let roomCode = try container.decode(String.self, forKey: .roomCode)
            self = .joinRoom(playerID: playerID, roomCode: roomCode)
        case "move":
            let uci = try container.decode(String.self, forKey: .uci)
            self = .move(uci: uci)
        case "resign":
            self = .resign
        case "leave":
            self = .leave
        case "register":
            let username = try container.decode(String.self, forKey: .username)
            let password = try container.decode(String.self, forKey: .password)
            let deviceID = try container.decodeIfPresent(String.self, forKey: .deviceID)
            self = .register(username: username, password: password, deviceID: deviceID)
        case "login":
            let username = try container.decode(String.self, forKey: .username)
            let password = try container.decode(String.self, forKey: .password)
            let deviceID = try container.decodeIfPresent(String.self, forKey: .deviceID)
            self = .login(username: username, password: password, deviceID: deviceID)
        case "logout":
            let token = try container.decode(String.self, forKey: .token)
            self = .logout(token: token)
        case "set_profile":
            let token = try container.decode(String.self, forKey: .token)
            let displayName = try container.decode(String.self, forKey: .displayName)
            self = .setProfile(token: token, displayName: displayName)
        default:
            throw OnlineProtocolError.unknownMessageType(type)
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(kOnlineProtocolVersion, forKey: .v)
        switch self {
        case .createRoom(let playerID, let timeControl):
            try container.encode("create_room", forKey: .type)
            try container.encode(playerID, forKey: .playerID)
            if let timeControl {
                try container.encode(timeControl, forKey: .timeControl)
            }
        case .joinRoom(let playerID, let roomCode):
            try container.encode("join_room", forKey: .type)
            try container.encode(playerID, forKey: .playerID)
            try container.encode(roomCode, forKey: .roomCode)
        case .move(let uci):
            try container.encode("move", forKey: .type)
            try container.encode(uci, forKey: .uci)
        case .resign:
            try container.encode("resign", forKey: .type)
        case .leave:
            try container.encode("leave", forKey: .type)
        case .register(let username, let password, let deviceID):
            try container.encode("register", forKey: .type)
            try container.encode(username, forKey: .username)
            try container.encode(password, forKey: .password)
            if let deviceID {
                try container.encode(deviceID, forKey: .deviceID)
            }
        case .login(let username, let password, let deviceID):
            try container.encode("login", forKey: .type)
            try container.encode(username, forKey: .username)
            try container.encode(password, forKey: .password)
            if let deviceID {
                try container.encode(deviceID, forKey: .deviceID)
            }
        case .logout(let token):
            try container.encode("logout", forKey: .type)
            try container.encode(token, forKey: .token)
        case .setProfile(let token, let displayName):
            try container.encode("set_profile", forKey: .type)
            try container.encode(token, forKey: .token)
            try container.encode(displayName, forKey: .displayName)
        }
    }
}

/// Server messages: versioned JSON with a `type` tag.
enum OnlineServerMessage: Codable, Equatable {
    /// The seat is ready: room code, our color, and the initial snapshot.
    case roomReady(roomCode: String, yourColor: OnlineColor, state: OnlineState)
    /// Full state snapshot after any accepted action and on re-attach.
    case state(OnlineState)
    /// Structured business-rule error (the connection stays open).
    case error(code: OnlineErrorCode, message: String)
    case session(token: String)
    case profileUpdated
    case sessionOk

    private enum CodingKeys: String, CodingKey {
        case type
        case v
        case roomCode = "room_code"
        case yourColor = "your_color"
        case state
        case code
        case message
        case token
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let type = try container.decode(String.self, forKey: .type)
        let version = try container.decode(Int.self, forKey: .v)
        guard version == kOnlineProtocolVersion else {
            throw OnlineProtocolError.unsupportedVersion(version)
        }
        switch type {
        case "room_ready":
            let roomCode = try container.decode(String.self, forKey: .roomCode)
            let yourColor = try container.decode(OnlineColor.self, forKey: .yourColor)
            let state = try container.decode(OnlineState.self, forKey: .state)
            self = .roomReady(roomCode: roomCode, yourColor: yourColor, state: state)
        case "state":
            let state = try container.decode(OnlineState.self, forKey: .state)
            self = .state(state)
        case "error":
            let code = try container.decode(OnlineErrorCode.self, forKey: .code)
            let message = try container.decode(String.self, forKey: .message)
            self = .error(code: code, message: message)
        case "session":
            let token = try container.decode(String.self, forKey: .token)
            self = .session(token: token)
        case "profile_updated":
            self = .profileUpdated
        case "session_ok":
            self = .sessionOk
        default:
            throw OnlineProtocolError.unknownMessageType(type)
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(kOnlineProtocolVersion, forKey: .v)
        switch self {
        case .roomReady(let roomCode, let yourColor, let state):
            try container.encode("room_ready", forKey: .type)
            try container.encode(roomCode, forKey: .roomCode)
            try container.encode(yourColor, forKey: .yourColor)
            try container.encode(state, forKey: .state)
        case .state(let state):
            try container.encode("state", forKey: .type)
            try container.encode(state, forKey: .state)
        case .error(let code, let message):
            try container.encode("error", forKey: .type)
            try container.encode(code, forKey: .code)
            try container.encode(message, forKey: .message)
        case .session(let token):
            try container.encode("session", forKey: .type)
            try container.encode(token, forKey: .token)
        case .profileUpdated:
            try container.encode("profile_updated", forKey: .type)
        case .sessionOk:
            try container.encode("session_ok", forKey: .type)
        }
    }
}

/// Encode/decode helpers over the wire (design D2).
enum OnlineCodec {
    /// Encode one client message to its JSON text form.
    static func encodeClient(_ message: OnlineClientMessage) throws -> String {
        let data = try JSONEncoder().encode(message)
        guard let json = String(data: data, encoding: .utf8) else {
            throw OnlineProtocolError.invalidJSON
        }
        return json
    }

    /// Decode one server JSON text frame; throws `OnlineProtocolError` for
    /// malformed JSON, unknown types, or unsupported versions.
    static func decodeServer(_ json: String) throws -> OnlineServerMessage {
        guard let data = json.data(using: .utf8) else {
            throw OnlineProtocolError.invalidJSON
        }
        do {
            return try JSONDecoder().decode(OnlineServerMessage.self, from: data)
        } catch let error as OnlineProtocolError {
            throw error
        } catch {
            throw OnlineProtocolError.invalidJSON
        }
    }
}
