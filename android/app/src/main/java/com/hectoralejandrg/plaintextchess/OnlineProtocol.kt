package com.hectoralejandrg.plaintextchess

import org.json.JSONException
import org.json.JSONObject

/**
 * Online multiplayer protocol codec (shared spec "Online Multiplayer
 * Protocol", design D2).
 *
 * Mirrors the pinned Rust wire form (`server/src/protocol.rs`, pinned by the
 * `wire_form_is_stable` test): type-tagged `v:1` JSON messages, snake_case
 * fields, and the externally tagged `Status` (unit variants are plain
 * strings, struct variants are `{"checkmated": {"winner": "white"}}`). A
 * codec change that drifts from the Rust test is a protocol change.
 */

/** Protocol version carried by every message. */
const val ONLINE_PROTOCOL_VERSION = 1

/** Online server configuration (add-online-multiplayer D9): the URL is a
 * build-time constant; DEBUG builds may override it with the `online_url`
 * intent extra. */
object OnlineConfig {
    /** Default online server URL. Empty in this change: online play is
     * verified through the DEBUG override. */
    const val DEFAULT_URL: String = ""
}

/** Colors on the wire (`"white"` / `"black"`). */
sealed class OnlineColor(val wire: String, val displayName: String) {
    data object White : OnlineColor("white", "White")
    data object Black : OnlineColor("black", "Black")

    /** The board's side-to-move letter for this color. */
    val sideToMove: String
        get() = if (this == White) "w" else "b"

    companion object {
        fun fromWire(value: String): OnlineColor = when (value) {
            "white" -> White
            "black" -> Black
            else -> throw OnlineProtocolError.InvalidColor(value)
        }
    }
}

/** Game status as reported by the server (externally tagged on the wire). */
sealed class OnlineStatus {
    data object Playing : OnlineStatus()
    data class Checkmated(val winnerColor: OnlineColor) : OnlineStatus()
    data object Drawn : OnlineStatus()
    data class Resigned(val winnerColor: OnlineColor) : OnlineStatus()
    data class Forfeited(val winnerColor: OnlineColor) : OnlineStatus()

    /** Flag fall (add-online-time-controls): a player's clock ran out. */
    data class TimedOut(val winnerColor: OnlineColor) : OnlineStatus()

    val isTerminal: Boolean get() = this !is Playing

    /** The winner of a decisive result (`null` while playing or drawn). */
    val winner: OnlineColor?
        get() = when (this) {
            is Checkmated -> winnerColor
            is Resigned -> winnerColor
            is Forfeited -> winnerColor
            is TimedOut -> winnerColor
            Playing, Drawn -> null
        }

    /** Whether this ending was a flag fall (a win/loss on time). */
    val isFlagFall: Boolean get() = this is TimedOut

    companion object {
        /** Decode the `status` JSON value: a plain string (`"playing"`,
         * `"drawn"`) or an object (`{"checkmated": {"winner": "white"}}`). */
        fun decode(json: Any?): OnlineStatus {
            when (json) {
                is String -> {
                    return when (json) {
                        "playing" -> Playing
                        "drawn" -> Drawn
                        else -> throw OnlineProtocolError.InvalidStatus(json)
                    }
                }
                is JSONObject -> {
                    if (json.has("checkmated")) {
                        return Checkmated(winnerFrom(json.getJSONObject("checkmated")))
                    }
                    if (json.has("resigned")) {
                        return Resigned(winnerFrom(json.getJSONObject("resigned")))
                    }
                    if (json.has("forfeited")) {
                        return Forfeited(winnerFrom(json.getJSONObject("forfeited")))
                    }
                    if (json.has("timed_out")) {
                        return TimedOut(winnerFrom(json.getJSONObject("timed_out")))
                    }
                    throw OnlineProtocolError.InvalidStatus(json.toString())
                }
                else -> throw OnlineProtocolError.InvalidStatus(json.toString())
            }
        }

        private fun winnerFrom(variant: JSONObject): OnlineColor =
            OnlineColor.fromWire(variant.getString("winner"))
    }
}

/** A time-control preset (spec "Server Time Control and Clock"): base time
 * plus a Fischer increment, mirrored from the server's supported set. The
 * [label] is the exact wire string sent in `create_room`. */
data class OnlineTimeControl(
    val label: String,
    val baseSeconds: Int,
    val incrementSeconds: Int,
) {
    companion object {
        val presets: List<OnlineTimeControl> = listOf(
            OnlineTimeControl("15+10", 15 * 60, 10),
            OnlineTimeControl("10+0", 10 * 60, 0),
            OnlineTimeControl("5+0", 5 * 60, 0),
            OnlineTimeControl("3+2", 3 * 60, 2),
            OnlineTimeControl("1+0", 1 * 60, 0),
        )
        val default: OnlineTimeControl = presets[0]
    }
}

/**
 * Full state snapshot: one of these rebuilds the entire game screen (spec
 * scenario "A state snapshot rebuilds the game screen").
 */
data class OnlineState(
    val boardFen: String,
    val moveList: List<String>,
    /** Side to move: `"w"` or `"b"`. */
    val sideToMove: String,
    val status: OnlineStatus,
    val yourColor: OnlineColor,
    val whiteRating: Double,
    val blackRating: Double,
    val opponentOnline: Boolean,
    /** The room's time control label (e.g. `"15+10"`). */
    val timeControl: String,
    /** White's remaining time in milliseconds (server-authoritative). */
    val whiteTimeMs: Int,
    /** Black's remaining time in milliseconds (server-authoritative). */
    val blackTimeMs: Int,
) {
    companion object {
        fun decode(json: JSONObject): OnlineState {
            val moves = ArrayList<String>()
            json.getJSONArray("move_list").let { array ->
                for (i in 0 until array.length()) moves.add(array.getString(i))
            }
            return OnlineState(
                boardFen = json.getString("board_fen"),
                moveList = moves,
                sideToMove = json.getString("side_to_move"),
                status = OnlineStatus.decode(json.get("status")),
                yourColor = OnlineColor.fromWire(json.getString("your_color")),
                whiteRating = json.getDouble("white_rating"),
                blackRating = json.getDouble("black_rating"),
                opponentOnline = json.getBoolean("opponent_online"),
                timeControl = json.getString("time_control"),
                whiteTimeMs = json.getInt("white_time_ms"),
                blackTimeMs = json.getInt("black_time_ms"),
            )
        }
    }
}

/**
 * Incremental update after an accepted action (server "Game Authority"): the
 * applied move, when there is one, plus the fields that can change.
 */
data class OnlineUpdate(
    /** The UCI move just applied; null for a terminal result with no move. */
    val uci: String?,
    val sideToMove: String,
    val status: OnlineStatus,
    val whiteRating: Double,
    val blackRating: Double,
    val opponentOnline: Boolean,
    val whiteTimeMs: Int,
    val blackTimeMs: Int,
) {
    companion object {
        fun decode(json: JSONObject): OnlineUpdate = OnlineUpdate(
            uci = if (json.has("uci") && !json.isNull("uci")) json.getString("uci") else null,
            sideToMove = json.getString("side_to_move"),
            status = OnlineStatus.decode(json.get("status")),
            whiteRating = json.getDouble("white_rating"),
            blackRating = json.getDouble("black_rating"),
            opponentOnline = json.getBoolean("opponent_online"),
            whiteTimeMs = json.getInt("white_time_ms"),
            blackTimeMs = json.getInt("black_time_ms"),
        )
    }
}

/** Stable machine-readable error codes (spec "Errors carry a stable code"). */
enum class OnlineErrorCode(val code: String, val displayMessage: String) {
    ROOM_NOT_FOUND("room_not_found", "No open room has that code"),
    ROOM_FULL("room_full", "That room already has two players"),
    ALREADY_IN_ROOM("already_in_room", "This player is already in a room"),
    INVALID_ROOM_CODE("invalid_room_code", "Room codes are 6 characters from A-Z and 2-9 (no I or O)"),
    NOT_YOUR_TURN("not_your_turn", "It is not your turn"),
    ILLEGAL_MOVE("illegal_move", "That move is not legal in the current position"),
    GAME_OVER("game_over", "The game is already over"),
    NOT_CONNECTED("not_connected", "The game has not started yet"),
    FORFEIT("forfeit", "The opponent did not reconnect in time"),
    USERNAME_TAKEN("username_taken", "That username is already registered"),
    INVALID_CREDENTIALS("invalid_credentials", "That username and password do not match an account"),
    NOT_AUTHENTICATED("not_authenticated", "This connection is not signed in"),
    SESSION_EXPIRED("session_expired", "This session has expired; log in again"),
    INVALID_REQUEST("invalid_request", "A field failed validation"),
    INVALID_DISPLAY_NAME(
        "invalid_display_name",
        "A display name must be 1 to 32 characters with no control characters",
    );

    companion object {
        /** Codes that end the join attempt instead of surfacing in-game:
         * the room is gone or unavailable, so retrying cannot help. */
        val DEFINITIVE_JOIN: Set<OnlineErrorCode> =
            setOf(ROOM_NOT_FOUND, ROOM_FULL, INVALID_ROOM_CODE, ALREADY_IN_ROOM)

        fun fromCode(code: String): OnlineErrorCode? =
            entries.firstOrNull { it.code == code }
    }
}

/** Protocol-level failures (the server closes with 4000 on these; the
 * client treats a malformed inbound frame as a connection error). */
sealed class OnlineProtocolError(override val message: String) : Exception(message) {
    data object InvalidJson : OnlineProtocolError("Invalid JSON frame")
    data class UnknownType(val type: String) : OnlineProtocolError("Unknown message type: $type")
    data class UnsupportedVersion(val version: Int) :
        OnlineProtocolError("Unsupported protocol version: $version")
    data class InvalidStatus(val raw: String) : OnlineProtocolError("Invalid status: $raw")
    data class InvalidColor(val raw: String) : OnlineProtocolError("Invalid color: $raw")
}

/** Server messages: versioned JSON with a `type` tag. */
sealed class ServerMessage {
    /** The seat is ready: room code, our color, and the initial snapshot. */
    data class RoomReady(
        val roomCode: String,
        val yourColor: OnlineColor,
        val state: OnlineState,
    ) : ServerMessage()

    /** Full state snapshot after any accepted action and on re-attach. */
    data class State(val state: OnlineState) : ServerMessage()

    /** Incremental per-move / terminal update (server "Game Authority"). */
    data class Update(val update: OnlineUpdate) : ServerMessage()

    /** Structured business-rule error (the connection stays open). */
    data class Error(val code: String, val message: String) : ServerMessage()

    /** Authentication session issued (register/login). */
    data class Session(val token: String) : ServerMessage()

    /** Profile display name updated (set_profile). */
    data object ProfileUpdated : ServerMessage()

    /** Session revoked (logout). */
    data object SessionOk : ServerMessage()

    companion object {
        /** Decode one server JSON text frame; throws [OnlineProtocolError]
         * for malformed JSON, unknown types, or unsupported versions. */
        fun decode(json: String): ServerMessage {
            val root: JSONObject = try {
                JSONObject(json)
            } catch (e: Exception) {
                throw OnlineProtocolError.InvalidJson
            }
            val type = root.optString("type", "")
            val version = root.optInt("v", -1)
            if (version != ONLINE_PROTOCOL_VERSION) {
                throw OnlineProtocolError.UnsupportedVersion(version)
            }
            return try {
                when (type) {
                    "room_ready" -> RoomReady(
                        roomCode = root.getString("room_code"),
                        yourColor = OnlineColor.fromWire(root.getString("your_color")),
                        state = OnlineState.decode(root.getJSONObject("state")),
                    )
                    "state" -> State(OnlineState.decode(root.getJSONObject("state")))
                    "update" -> Update(OnlineUpdate.decode(root))
                    "error" -> Error(root.getString("code"), root.getString("message"))
                    "session" -> Session(root.getString("token"))
                    "profile_updated" -> ProfileUpdated
                    "session_ok" -> SessionOk
                    else -> throw OnlineProtocolError.UnknownType(type)
                }
            } catch (e: JSONException) {
                // A frame that parses as JSON but is missing a required
                // field (or has a mistyped one) is malformed protocol data.
                throw OnlineProtocolError.InvalidJson
            }
        }
    }
}

/** Encode/decode helpers for client-side messages (design D2). */
object OnlineCodec {
    /** `{"v":1,"type":"create_room","player_id":...,"time_control":...}` */
    fun encodeCreateRoom(playerId: String, timeControl: String?): String {
        val message = base("create_room").put("player_id", playerId)
        if (timeControl != null) {
            message.put("time_control", timeControl)
        }
        return message.toString()
    }

    /** `{"v":1,"type":"join_room","player_id":...,"room_code":...}` */
    fun encodeJoinRoom(playerId: String, roomCode: String): String =
        base("join_room")
            .put("player_id", playerId)
            .put("room_code", roomCode)
            .toString()

    /** `{"v":1,"type":"move","uci":...}` */
    fun encodeMove(uci: String): String =
        base("move").put("uci", uci).toString()

    /** `{"v":1,"type":"resign"}` */
    fun encodeResign(): String = base("resign").toString()

    /** `{"v":1,"type":"leave"}` */
    fun encodeLeave(): String = base("leave").toString()

    /** `{"v":1,"type":"register","username":"...","password":"...",` +
     * `"device_id":"..."}` — `device_id` links the account to this device
     * and is omitted only when unknown (spec "client links the device"). */
    fun encodeRegister(username: String, password: String, deviceId: String? = null): String {
        val message = base("register")
            .put("username", username)
            .put("password", password)
        if (deviceId != null) {
            message.put("device_id", deviceId)
        }
        return message.toString()
    }

    /** `{"v":1,"type":"login","username":"...","password":"...",` +
     * `"device_id":"..."}` (same `device_id` rules as [encodeRegister]). */
    fun encodeLogin(username: String, password: String, deviceId: String? = null): String {
        val message = base("login")
            .put("username", username)
            .put("password", password)
        if (deviceId != null) {
            message.put("device_id", deviceId)
        }
        return message.toString()
    }

    /** `{"v":1,"type":"logout","token":"..."}` */
    fun encodeLogout(token: String): String =
        base("logout").put("token", token).toString()

    /** `{"v":1,"type":"set_profile","token":"...","display_name":"..."}` */
    fun encodeSetProfile(token: String, displayName: String): String =
        base("set_profile")
            .put("token", token)
            .put("display_name", displayName)
            .toString()

    private fun base(type: String): JSONObject =
        JSONObject().put("v", ONLINE_PROTOCOL_VERSION).put("type", type)
}
