package com.hectoralejandrg.plaintextchess

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener

/**
 * Phase of the online session (task 3.2, mirrors the iOS `OnlinePhase`):
 * idle → connecting → waiting → inGame, with reconnecting on a mid-game drop
 * and a terminal failed state.
 */
sealed class OnlinePhase {
    object Idle : OnlinePhase()
    object Connecting : OnlinePhase()
    /** We hold a seat in a room that is waiting for the opponent. */
    data class Waiting(val code: String) : OnlinePhase()
    object InGame : OnlinePhase()
    /** The socket dropped mid-game; re-attaching to the same seat. */
    object Reconnecting : OnlinePhase()
    data class Failed(val reason: String) : OnlinePhase()
}

/**
 * OkHttp WebSocket connection manager for the online protocol (design
 * D2/D4/D7): one long-lived socket, the JSON codec, the phase state machine,
 * and automatic re-attach with capped backoff while the server's reconnect
 * window is open. All state changes and callbacks happen on the main thread.
 */
class OnlineConnectionManager(
    private val url: String,
    private val deviceID: String,
) {
    /** Every state snapshot, including the initial one in `room_ready`. */
    var onSnapshot: ((OnlineState) -> Unit)? = null
    /** Structured errors: join-time room errors and transient in-game
     * errors; `code` is null for pure connection-loss failures. */
    var onError: ((OnlineErrorCode?, String) -> Unit)? = null
    /** Phase changes (the view model mirrors these into its own state). */
    var onPhase: ((OnlinePhase) -> Unit)? = null

    @Volatile var phase: OnlinePhase = OnlinePhase.Idle
        private set
    @Volatile var roomCode: String? = null
        private set
    @Volatile var yourColor: OnlineColor? = null
        private set
    @Volatile var reconnecting: Boolean = false
        private set
    @Volatile var failureReason: String? = null
        private set

    /** What the open socket should be told to do when it connects. */
    private sealed class PendingAction {
        object None : PendingAction()
        data class Create(val timeControl: String?) : PendingAction()
        data class Join(val code: String) : PendingAction()
    }

    private val client = OkHttpClient.Builder().build()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var webSocket: WebSocket? = null
    private var pendingAction: PendingAction = PendingAction.None
    /** True once the opponent is seated / the game has started. */
    private var activeGame = false
    private var retryJob: Job? = null
    private var retryBackoffMs = 1_000L
    private var retryDeadlineMs: Long? = null

    /** Intents */

    /** Connect and create a room (the caller takes the White seat) with the
     * chosen time-control label. */
    fun startCreating(timeControl: String?) {
        beginAttempt(OnlinePhase.Connecting, PendingAction.Create(timeControl), reconnect = false)
    }

    /** Connect and join an existing room by code. */
    fun startJoining(code: String) {
        beginAttempt(OnlinePhase.Connecting, PendingAction.Join(code), reconnect = false, code = code)
    }

    /** Re-attach to an in-progress room after a drop or an app relaunch: the
     * same device identifier re-enters the seat it held. The first attempt
     * goes through the retry schedule (1 s) instead of connecting
     * immediately: that gives the "Reconnecting…" banner a visible window and
     * avoids racing the server's disconnect bookkeeping. */
    fun startReattaching(code: String) {
        activeGame = true
        beginAttempt(OnlinePhase.Reconnecting, PendingAction.Join(code), reconnect = true, code = code, immediate = false)
    }

    fun sendMove(uci: String) {
        send(OnlineCodec.encodeMove(uci))
    }

    fun resign() {
        send(OnlineCodec.encodeResign())
    }

    fun leave() {
        send(OnlineCodec.encodeLeave())
    }

    /** Close the session and clear all state (the caller owns this manager
     * from here on; callbacks stop). */
    fun teardown() {
        retryJob?.cancel()
        retryJob = null
        retryDeadlineMs = null
        pendingAction = PendingAction.None
        webSocket?.close(1001, null)
        webSocket = null
        activeGame = false
        reconnecting = false
        roomCode = null
        yourColor = null
        failureReason = null
        client.dispatcher.executorService.shutdown()
        client.connectionPool.evictAll()
        setPhase(OnlinePhase.Idle)
    }

    /** Connection lifecycle */

    private fun beginAttempt(
        newPhase: OnlinePhase,
        action: PendingAction,
        reconnect: Boolean,
        code: String? = null,
        immediate: Boolean = true,
    ) {
        // A fresh attempt: clear any previous attempt's state.
        retryJob?.cancel()
        retryJob = null
        retryBackoffMs = 1_000L
        retryDeadlineMs = null
        pendingAction = action
        roomCode = code
        yourColor = null
        failureReason = null
        if (reconnect) {
            reconnecting = true
        }
        setPhase(newPhase)
        if (immediate) {
            connect()
        } else {
            beginReconnect()
        }
    }

    private fun connect() {
        val request = Request.Builder().url(url).build()
        val socket = client.newWebSocket(request, listener)
        webSocket = socket
        // OkHttp queues text frames until the handshake completes, so the
        // intent can be fired right after newWebSocket (create on first
        // connect, join on a re-attach attempt).
        when (val action = pendingAction) {
            is PendingAction.Create ->
                send(OnlineCodec.encodeCreateRoom(deviceID, action.timeControl))
            is PendingAction.Join -> send(OnlineCodec.encodeJoinRoom(deviceID, action.code))
            is PendingAction.None -> Unit
        }
    }

    private val listener = object : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) {
            // Frames are decoded as they arrive (onMessage); nothing to do.
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            onMain {
                // Ignore late frames from a replaced socket (re-attach).
                if (webSocket !== this@OnlineConnectionManager.webSocket) return@onMain
                try {
                    handleServerMessage(ServerMessage.decode(text))
                } catch (e: OnlineProtocolError) {
                    // A malformed frame from the server is a protocol
                    // failure: treat it like a drop.
                    handleSocketDrop()
                }
            }
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(code, null)
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            onMain { handleSocketDrop() }
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            onMain { handleSocketDrop() }
        }
    }

    /** Hop to the main dispatcher: OkHttp delivers listener callbacks on
     * its own executor threads. */
    private fun onMain(block: () -> Unit) {
        scope.launch { block() }
    }

    private fun setPhase(newPhase: OnlinePhase) {
        phase = newPhase
        onPhase?.invoke(newPhase)
    }

    /** Incoming frames */

    private fun handleServerMessage(message: ServerMessage) {
        when (message) {
            is ServerMessage.RoomReady -> {
                roomCode = message.roomCode
                yourColor = message.yourColor
                reconnecting = false
                failureReason = null
                retryJob?.cancel()
                retryJob = null
                retryDeadlineMs = null
                when (pendingAction) {
                    is PendingAction.Create -> setPhase(OnlinePhase.Waiting(message.roomCode))
                    is PendingAction.Join -> {
                        activeGame = true
                        setPhase(OnlinePhase.InGame)
                    }
                    is PendingAction.None -> Unit
                }
                onSnapshot?.invoke(message.state)
            }
            is ServerMessage.State -> {
                reconnecting = false
                yourColor = message.state.yourColor
                if (message.state.opponentOnline || message.state.moveList.isNotEmpty()) {
                    activeGame = true
                    // A started game settles the phase even mid-attach: the
                    // server answers a re-attach with this resync snapshot
                    // (no room_ready), so waiting for room_ready would leave
                    // the "Reconnecting…" banner up forever.
                    setPhase(OnlinePhase.InGame)
                }
                onSnapshot?.invoke(message.state)
            }
            is ServerMessage.Error -> {
                val code = OnlineErrorCode.fromCode(message.code)
                if (code != null && code in OnlineErrorCode.DEFINITIVE_JOIN) {
                    // Definitive for the join in flight: the room is gone or
                    // unavailable, so stop retrying and report the error.
                    terminate(message.message, code)
                } else {
                    // Transient in-game error: surface it, keep the session.
                    onError?.invoke(code, message.message)
                }
            }
        }
    }

    /** The socket went away (failure, close frame, or a malformed frame). */
    private fun handleSocketDrop() {
        // Late events after teardown or a definitive failure: ignore.
        if (phase is OnlinePhase.Idle) return
        if (phase is OnlinePhase.Failed) return
        webSocket?.close(1000, null)
        webSocket = null
        pendingAction = PendingAction.None

        if (roomCode != null) {
            // A room exists for this device: keep re-attaching while the
            // server's window is open. In a dropped lobby the first retry is
            // answered with `room_not_found` and stops.
            beginReconnect()
        } else {
            terminate("Connection lost", OnlineErrorCode.NOT_CONNECTED)
        }
    }

    /** Reconnect */

    private fun beginReconnect() {
        reconnecting = true
        setPhase(OnlinePhase.Reconnecting)
        if (retryDeadlineMs == null) {
            // Server reconnect window: the server's default grace is 120 s,
            // so keep retrying a little past it before giving up.
            retryDeadlineMs = System.currentTimeMillis() + MAX_RECONNECT_WINDOW_MS
        }
        scheduleRetry()
    }

    private fun scheduleRetry() {
        retryJob?.cancel()
        val code = roomCode ?: run {
            terminate("Connection lost", OnlineErrorCode.NOT_CONNECTED)
            return
        }
        val delayMs = retryBackoffMs
        retryBackoffMs = minOf(retryBackoffMs * 2, 8_000L)
        retryJob = scope.launch {
            delay(delayMs)
            if (phase !is OnlinePhase.Reconnecting) return@launch
            val deadline = retryDeadlineMs ?: return@launch
            if (System.currentTimeMillis() >= deadline) {
                terminate("Could not reconnect in time", OnlineErrorCode.NOT_CONNECTED)
                return@launch
            }
            pendingAction = PendingAction.Join(code)
            connect()
        }
    }

    /** Outgoing frames */

    private fun send(json: String) {
        // A send failure surfaces as a drop in the socket callbacks; nothing
        // to do besides not crashing on a dead socket.
        runCatching { webSocket?.send(json) }
    }

    /** Failure */

    private fun terminate(reason: String, code: OnlineErrorCode?) {
        retryJob?.cancel()
        retryJob = null
        retryDeadlineMs = null
        webSocket?.close(1000, null)
        webSocket = null
        pendingAction = PendingAction.None
        activeGame = false
        reconnecting = false
        roomCode = null
        failureReason = reason
        setPhase(OnlinePhase.Failed(reason))
        onError?.invoke(code, reason)
    }

    companion object {
        /** Upper bound of the client-side retry window (ms): a little past
         * the server's default 120 s grace. */
        const val MAX_RECONNECT_WINDOW_MS = 150_000L
    }
}
