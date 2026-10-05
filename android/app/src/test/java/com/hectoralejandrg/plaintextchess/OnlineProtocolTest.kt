package com.hectoralejandrg.plaintextchess

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/**
 * Codec tests for the online protocol (task 3.2), mirroring the iOS
 * `OnlineProtocolTests` and the server's `wire_form_is_stable` pin: every
 * message type and error code round-trips against the exact JSON shapes the
 * Rust server produces.
 */
class OnlineProtocolTest {

    private fun stateJson(): JSONObject = JSONObject(
        """
        {
          "board_fen": "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR",
          "move_list": ["e2e4"],
          "side_to_move": "b",
          "status": "playing",
          "your_color": "white",
          "white_rating": 1500.0,
          "black_rating": 1499.0,
          "opponent_online": true,
          "time_control": "15+10",
          "white_time_ms": 900000,
          "black_time_ms": 890000
        }
        """
    )

    // MARK: - Client wire forms

    @Test
    fun clientCreateRoomWireForm() {
        val json = JSONObject(OnlineCodec.encodeCreateRoom("p1", "3+2"))
        assertEquals("create_room", json.getString("type"))
        assertEquals(1, json.getInt("v"))
        assertEquals("p1", json.getString("player_id"))
        assertEquals("3+2", json.getString("time_control"))
    }

    @Test
    fun clientCreateRoomWithoutTimeControlOmitsTheField() {
        val json = JSONObject(OnlineCodec.encodeCreateRoom("p1", null))
        assertFalse("an unset control must be absent, not null", json.has("time_control"))
    }

    @Test
    fun clientJoinRoomWireForm() {
        val json = JSONObject(OnlineCodec.encodeJoinRoom("p2", "AB23CD"))
        assertEquals("join_room", json.getString("type"))
        assertEquals(1, json.getInt("v"))
        assertEquals("p2", json.getString("player_id"))
        assertEquals("AB23CD", json.getString("room_code"))
    }

    @Test
    fun clientMoveWireFormInclPromotion() {
        for (uci in listOf("e2e4", "g7h8q")) {
            val json = JSONObject(OnlineCodec.encodeMove(uci))
            assertEquals("move", json.getString("type"))
            assertEquals(1, json.getInt("v"))
            assertEquals(uci, json.getString("uci"))
        }
    }

    @Test
    fun clientResignAndLeaveWireForms() {
        for (pair in listOf(OnlineCodec.encodeResign() to "resign", OnlineCodec.encodeLeave() to "leave")) {
            val json = JSONObject(pair.first)
            assertEquals(pair.second, json.getString("type"))
            assertEquals(1, json.getInt("v"))
            assertFalse(json.has("uci"))
            assertFalse(json.has("player_id"))
            assertEquals(2, json.length())
        }
    }

    // MARK: - Server messages

    @Test
    fun serverRoomReadyDecodes() {
        val json = JSONObject(
            """
            {"v":1,"type":"room_ready","room_code":"AB23CD","your_color":"black","state":${stateJson().toString()}}
            """
        ).toString()
        val message = ServerMessage.decode(json)
        val roomReady = message as? ServerMessage.RoomReady
        assertNotNull(roomReady)
        roomReady!!
        assertEquals("AB23CD", roomReady.roomCode)
        assertEquals(OnlineColor.Black, roomReady.yourColor)
        assertStateDecoded(roomReady.state)
    }

    @Test
    fun serverStateDecodes() {
        val json = JSONObject(
            """{"v":1,"type":"state","state":${stateJson().toString()}}"""
        ).toString()
        val state = (ServerMessage.decode(json) as ServerMessage.State).state
        assertStateDecoded(state)
    }

    @Test
    fun serverErrorDecodesForEveryStableCode() {
        for (code in OnlineErrorCode.entries) {
            val message = JSONObject(
                """{"v":1,"type":"error","code":"${code.code}","message":"${code.displayMessage}"}"""
            ).toString()
            val error = (ServerMessage.decode(message) as ServerMessage.Error)
            assertEquals(code.code, error.code)
            assertEquals(code.displayMessage, error.message)
        }
    }

    @Test
    fun everyErrorCodeHasAStableWireForm() {
        for (code in OnlineErrorCode.entries) {
            assertEquals(code, OnlineErrorCode.fromCode(code.code))
        }
        assertNull(OnlineErrorCode.fromCode("teleport"))
    }

    // MARK: - Status (externally tagged)

    @Test
    fun statusUnitVariantsArePlainStrings() {
        assertEquals(OnlineStatus.Playing, OnlineStatus.decode("playing"))
        assertEquals(OnlineStatus.Drawn, OnlineStatus.decode("drawn"))
    }

    @Test
    fun statusStructVariantsDecode() {
        assertEquals(
            OnlineStatus.Checkmated(OnlineColor.Black),
            OnlineStatus.decode(JSONObject("""{"checkmated":{"winner":"black"}}""")),
        )
        assertEquals(
            OnlineStatus.Resigned(OnlineColor.White),
            OnlineStatus.decode(JSONObject("""{"resigned":{"winner":"white"}}""")),
        )
        assertEquals(
            OnlineStatus.Forfeited(OnlineColor.Black),
            OnlineStatus.decode(JSONObject("""{"forfeited":{"winner":"black"}}""")),
        )
        assertEquals(
            OnlineStatus.TimedOut(OnlineColor.White),
            OnlineStatus.decode(JSONObject("""{"timed_out":{"winner":"white"}}""")),
        )
    }

    @Test
    fun timeControlPresetsMatchTheServer() {
        assertEquals(
            listOf("15+10", "10+0", "5+0", "3+2", "1+0"),
            OnlineTimeControl.presets.map { it.label },
        )
        assertEquals("15+10", OnlineTimeControl.default.label)
    }

    @Test
    fun statusWinnerAndTerminal() {
        assertEquals(OnlineColor.White, OnlineStatus.Checkmated(OnlineColor.White).winner)
        assertNull(OnlineStatus.Playing.winner)
        assertNull(OnlineStatus.Drawn.winner)
        assertTrue(OnlineStatus.Forfeited(OnlineColor.Black).isTerminal)
        assertTrue(!OnlineStatus.Playing.isTerminal)
    }

    // MARK: - Rejections

    @Test
    fun unknownTypeThrows() {
        assertThrows<OnlineProtocolError.UnknownType> {
            ServerMessage.decode("""{"v":1,"type":"teleport"}""")
        }
    }

    @Test
    fun unsupportedVersionThrows() {
        assertThrows<OnlineProtocolError.UnsupportedVersion> {
            ServerMessage.decode("""{"v":2,"type":"state","state":${stateJson().toString()}}""")
        }
    }

    @Test
    fun malformedFramesThrow() {
        for (frame in listOf(
            "not json",
            """["state"]""",
            """{"v":1,"type":"state"}""",
            """{"v":1}""",
        )) {
            assertThrows<OnlineProtocolError> { ServerMessage.decode(frame) }
        }
    }

    @Test
    fun invalidColorAndStatusThrow() {
        assertThrows<OnlineProtocolError.InvalidColor> {
            OnlineColor.fromWire("blue")
        }
        assertThrows<OnlineProtocolError.InvalidStatus> {
            OnlineStatus.decode("exploded")
        }
        assertThrows<OnlineProtocolError.InvalidStatus> {
            OnlineStatus.decode(JSONObject("""{"exploded":{}}"""))
        }
    }

    // MARK: - Helpers

    private fun assertStateDecoded(state: OnlineState) {
        assertEquals("rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR", state.boardFen)
        assertEquals(listOf("e2e4"), state.moveList)
        assertEquals("b", state.sideToMove)
        assertEquals(OnlineStatus.Playing, state.status)
        assertEquals(OnlineColor.White, state.yourColor)
        assertEquals(1500.0, state.whiteRating, 0.0)
        assertEquals(1499.0, state.blackRating, 0.0)
        assertEquals(true, state.opponentOnline)
        assertEquals("15+10", state.timeControl)
        assertEquals(900000, state.whiteTimeMs)
        assertEquals(890000, state.blackTimeMs)
    }

    private inline fun <reified T : Throwable> assertThrows(block: () -> Unit) {
        try {
            block()
            fail("Expected ${T::class.simpleName}")
        } catch (e: Throwable) {
            if (e is T) return
            fail("Expected ${T::class.simpleName} but got ${e::class.java.simpleName}: ${e.message}")
        }
    }
}
