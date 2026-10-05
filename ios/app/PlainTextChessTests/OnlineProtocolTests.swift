import XCTest
@testable import PlainTextChess

/// Codec tests for the online multiplayer protocol (add-online-multiplayer
/// D2/D5). The wire shape is pinned by the server's `wire_form_is_stable`
/// test; these keep the Swift side in lockstep with it.
final class OnlineProtocolTests: XCTestCase {

    private func jsonObject(_ json: String) throws -> [String: Any] {
        guard let data = json.data(using: .utf8) else {
            fatalError("bad test JSON")
        }
        return try XCTUnwrap(
            JSONSerialization.jsonObject(with: data) as? [String: Any],
            "expected a JSON object"
        )
    }

    /// The server's `room_ready`/`state` snapshot at the start of a game
    /// (same shape as the server's wire-form test sample).
    private let startingState = """
    {"board_fen":"rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR","move_list":[],"side_to_move":"w","status":"playing","your_color":"white","white_rating":1500.0,"black_rating":1500.0,"opponent_online":true,"time_control":"15+10","white_time_ms":900000,"black_time_ms":900000}
    """

    // MARK: - Client messages (encode)

    func testCreateRoomEncoding() throws {
        let json = try OnlineCodec.encodeClient(.createRoom(playerID: "device-a", timeControl: "3+2"))
        let object = try jsonObject(json)
        XCTAssertEqual(object["type"] as? String, "create_room")
        XCTAssertEqual(object["v"] as? Int, 1)
        XCTAssertEqual(object["player_id"] as? String, "device-a")
        XCTAssertEqual(object["time_control"] as? String, "3+2")
    }

    func testCreateRoomWithoutTimeControlOmitsTheField() throws {
        let json = try OnlineCodec.encodeClient(.createRoom(playerID: "device-a", timeControl: nil))
        let object = try jsonObject(json)
        XCTAssertNil(object["time_control"], "an unset control must be absent, not null")
    }

    func testJoinRoomEncoding() throws {
        let json = try OnlineCodec.encodeClient(.joinRoom(playerID: "device-b", roomCode: "AB23CD"))
        let object = try jsonObject(json)
        XCTAssertEqual(object["type"] as? String, "join_room")
        XCTAssertEqual(object["v"] as? Int, 1)
        XCTAssertEqual(object["player_id"] as? String, "device-b")
        XCTAssertEqual(object["room_code"] as? String, "AB23CD")
    }

    func testMoveEncoding() throws {
        for uci in ["e2e4", "g7h8q"] {
            let json = try OnlineCodec.encodeClient(.move(uci: uci))
            let object = try jsonObject(json)
            XCTAssertEqual(object["type"] as? String, "move")
            XCTAssertEqual(object["v"] as? Int, 1)
            XCTAssertEqual(object["uci"] as? String, uci)
        }
    }

    func testResignAndLeaveEncoding() throws {
        let resign = try jsonObject(OnlineCodec.encodeClient(.resign))
        XCTAssertEqual(resign["type"] as? String, "resign")
        XCTAssertEqual(resign["v"] as? Int, 1)

        let leave = try jsonObject(OnlineCodec.encodeClient(.leave))
        XCTAssertEqual(leave["type"] as? String, "leave")
        XCTAssertEqual(leave["v"] as? Int, 1)
    }

    func testClientMessagesRoundTrip() throws {
        for message: OnlineClientMessage in [
            .createRoom(playerID: "p1", timeControl: "3+2"),
            .createRoom(playerID: "p1", timeControl: nil),
            .joinRoom(playerID: "p2", roomCode: "AB23CD"),
            .move(uci: "e2e4"),
            .move(uci: "g7h8q"),
            .resign,
            .leave,
        ] {
            let json = try OnlineCodec.encodeClient(message)
            let data = json.data(using: .utf8)
            XCTAssertEqual(try JSONDecoder().decode(OnlineClientMessage.self, from: data!), message)
        }
    }

    // MARK: - Server messages (decode)

    func testRoomReadyDecoding() throws {
        let json = """
        {"v":1,"type":"room_ready","room_code":"AB23CD","your_color":"white","state":\(startingState)}
        """
        let message = try OnlineCodec.decodeServer(json)
        guard case .roomReady(let code, let color, let state) = message else {
            return XCTFail("expected roomReady, got \(message)")
        }
        XCTAssertEqual(code, "AB23CD")
        XCTAssertEqual(color, .white)
        XCTAssertEqual(state.boardFen, "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR")
        XCTAssertTrue(state.moveList.isEmpty)
        XCTAssertEqual(state.sideToMove, "w")
        guard case .playing = state.status else {
            return XCTFail("expected playing, got \(state.status)")
        }
        XCTAssertEqual(state.yourColor, .white)
        XCTAssertEqual(state.whiteRating, 1500)
        XCTAssertEqual(state.blackRating, 1500)
        XCTAssertTrue(state.opponentOnline)
        XCTAssertEqual(state.timeControl, "15+10")
        XCTAssertEqual(state.whiteTimeMs, 900_000)
        XCTAssertEqual(state.blackTimeMs, 900_000)
    }

    func testStateDecoding() throws {
        let json = """
        {"v":1,"type":"state","state":\(startingState)}
        """
        guard case .state(let state) = try OnlineCodec.decodeServer(json) else {
            return XCTFail("expected state")
        }
        XCTAssertEqual(state.sideToMove, "w")
        XCTAssertTrue(state.opponentOnline)
    }

    func testAllStatusVariantsDecode() throws {
        func decodeStatus(_ fragment: String) throws -> OnlineStatus {
            let state = """
            {"board_fen":"rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR","move_list":[],"side_to_move":"w","status":\(fragment),"your_color":"white","white_rating":1500.0,"black_rating":1500.0,"opponent_online":false,"time_control":"5+0","white_time_ms":300000,"black_time_ms":300000}
            """
            let json = """
            {"v":1,"type":"state","state":\(state)}
            """
            guard case .state(let snapshot) = try OnlineCodec.decodeServer(json) else {
                XCTFail("expected a state message")
                throw OnlineProtocolError.invalidJSON
            }
            return snapshot.status
        }

        guard case .playing = try decodeStatus("\"playing\"") else {
            return XCTFail("expected playing")
        }
        XCTAssertEqual(try decodeStatus("{\"checkmated\":{\"winner\":\"white\"}}"),
                       .checkmated(winner: .white))
        XCTAssertEqual(try decodeStatus("\"drawn\""), .drawn)
        XCTAssertEqual(try decodeStatus("{\"resigned\":{\"winner\":\"black\"}}"),
                       .resigned(winner: .black))
        XCTAssertEqual(try decodeStatus("{\"forfeited\":{\"winner\":\"black\"}}"),
                       .forfeited(winner: .black))
        XCTAssertEqual(try decodeStatus("{\"timed_out\":{\"winner\":\"white\"}}"),
                       .timedOut(winner: .white))
    }

    func testTimeControlPresetsMatchTheServer() {
        XCTAssertEqual(OnlineTimeControl.presets.map(\.label),
                       ["15+10", "10+0", "5+0", "3+2", "1+0"])
        XCTAssertEqual(OnlineTimeControl.default.label, "15+10")
    }

    func testErrorDecodingAllStableCodes() throws {
        for code in OnlineErrorCode.allCases {
            let json = """
            {"v":1,"type":"error","code":"\(code.rawValue)","message":"\(code.displayMessage)"}
            """
            guard case .error(let decoded, let message) = try OnlineCodec.decodeServer(json) else {
                XCTFail("expected error for \(code)")
                continue
            }
            XCTAssertEqual(decoded, code)
            XCTAssertEqual(message, code.displayMessage)
        }
    }

    // MARK: - Protocol failures

    func testUnsupportedVersionIsRejected() {
        let json = """
        {"v":2,"type":"state","state":\(startingState)}
        """
        XCTAssertThrowsError(try OnlineCodec.decodeServer(json)) { error in
            guard case .unsupportedVersion(let version) = error as? OnlineProtocolError else {
                return XCTFail("unexpected error \(error)")
            }
            XCTAssertEqual(version, 2)
        }
    }

    func testUnknownMessageTypeIsRejected() {
        let json = """
        {"v":1,"type":"teleport","state":\(startingState)}
        """
        XCTAssertThrowsError(try OnlineCodec.decodeServer(json)) { error in
            guard case .unknownMessageType(let type) = error as? OnlineProtocolError else {
                return XCTFail("unexpected error \(error)")
            }
            XCTAssertEqual(type, "teleport")
        }
    }

    func testMalformedJSONIsRejected() {
        XCTAssertThrowsError(try OnlineCodec.decodeServer("{not json")) { error in
            guard case .invalidJSON = error as? OnlineProtocolError else {
                return XCTFail("unexpected error \(error)")
            }
        }
    }
}
