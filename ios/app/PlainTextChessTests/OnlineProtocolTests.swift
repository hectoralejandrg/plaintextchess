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
            .register(username: "ana", email: "ana@example.com", password: "passw0rd", deviceID: "device-a"),
            .register(username: "ana", email: "ana@example.com", password: "passw0rd", deviceID: nil),
            .login(identifier: "ana", password: "passw0rd", deviceID: "device-a"),
            .login(identifier: "ana", password: "passw0rd", deviceID: nil),
            .logout(token: "tok-123"),
            .setProfile(token: "tok-123", displayName: "Ana"),
        ] {
            let json = try OnlineCodec.encodeClient(message)
            let data = json.data(using: .utf8)
            XCTAssertEqual(try JSONDecoder().decode(OnlineClientMessage.self, from: data!), message)
        }
    }

    // MARK: - Authentication messages (add-auth-ui-clients)

    func testRegisterEncodingCarriesTheDeviceID() throws {
        let json = try OnlineCodec.encodeClient(
            .register(username: "ana", email: "ana@example.com", password: "passw0rd", deviceID: "device-a"))
        let object = try jsonObject(json)
        XCTAssertEqual(object["type"] as? String, "register")
        XCTAssertEqual(object["v"] as? Int, 1)
        XCTAssertEqual(object["username"] as? String, "ana")
        XCTAssertEqual(object["password"] as? String, "passw0rd")
        XCTAssertEqual(object["device_id"] as? String, "device-a")
    }

    func testLoginOmitsAnUnknownDeviceID() throws {
        let json = try OnlineCodec.encodeClient(
            .login(identifier: "ana", password: "passw0rd", deviceID: nil))
        let object = try jsonObject(json)
        XCTAssertEqual(object["type"] as? String, "login")
        XCTAssertEqual(object["identifier"] as? String, "ana")
        XCTAssertNil(object["device_id"], "an unknown device must omit the field, not send null")
    }

    func testSessionDecodingCarriesTheToken() throws {
        let json = """
        {"v":1,"type":"session","account_id":"a1","username":"ana","display_name":"Ana","token":"tok-123","expires_at_ms":1700000000000}
        """
        guard case .session(let token, _, _, _) = try OnlineCodec.decodeServer(json) else {
            return XCTFail("expected session")
        }
        XCTAssertEqual(token, "tok-123")
    }

    /// The spec pins one generic message for every credential failure, on
    /// the server and on the client alike.
    func testInvalidCredentialsCarriesTheGenericMessage() throws {
        let json = """
        {"v":1,"type":"error","code":"invalid_credentials","message":"That username and password do not match an account"}
        """
        guard case .error(let code, let message, _) = try OnlineCodec.decodeServer(json) else {
            return XCTFail("expected error")
        }
        XCTAssertEqual(code, .invalidCredentials)
        XCTAssertEqual(code.displayMessage, "That username and password do not match an account")
        XCTAssertEqual(message, "That username and password do not match an account")
    }

    /// The client encodes `set_profile` with the server's wire format
    /// (`display_name` snake_case, `token`, `v:1`).
    func testSetProfileEncoding() throws {
        let json = try OnlineCodec.encodeClient(.setProfile(token: "t", displayName: "Ana T."))
        let object = try jsonObject(json)
        XCTAssertEqual(object["type"] as? String, "set_profile")
        XCTAssertEqual(object["v"] as? Int, 1)
        XCTAssertEqual(object["token"] as? String, "t")
        XCTAssertEqual(object["display_name"] as? String, "Ana T.")
    }

    /// A `profile_updated` reply decodes to the `.profileUpdated` case
    /// regardless of the extra `display_name` field.
    func testProfileUpdatedDecoding() throws {
        let json = """
        {"v":1,"type":"profile_updated","display_name":"Ana T."}
        """
        guard case .profileUpdated = try OnlineCodec.decodeServer(json) else {
            return XCTFail("expected profileUpdated")
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

    // MARK: - Tournaments (add-tournaments phase 1)

    func testJoinTournamentEncoding() throws {
        let json = try OnlineCodec.encodeClient(.joinTournament(playerID: "device-a", tournamentID: "t1"))
        let object = try jsonObject(json)
        XCTAssertEqual(object["type"] as? String, "join_tournament")
        XCTAssertEqual(object["v"] as? Int, 1)
        XCTAssertEqual(object["player_id"] as? String, "device-a")
        XCTAssertEqual(object["tournament_id"] as? String, "t1")
    }

    func testLeaveTournamentEncoding() throws {
        let json = try OnlineCodec.encodeClient(.leaveTournament(tournamentID: "t1"))
        let object = try jsonObject(json)
        XCTAssertEqual(object["type"] as? String, "leave_tournament")
        XCTAssertEqual(object["tournament_id"] as? String, "t1")
    }

    func testTournamentJoinedDecoding() throws {
        let json = """
        {"v":1,"type":"tournament_joined","tournament_id":"t1"}
        """
        guard case .tournamentJoined(let tournamentID) = try OnlineCodec.decodeServer(json) else {
            return XCTFail("expected tournamentJoined")
        }
        XCTAssertEqual(tournamentID, "t1")
    }

    func testTournamentPairingDecoding() throws {
        let json = """
        {"v":1,"type":"tournament_pairing","tournament_id":"t1","room_code":"AB23CD","your_color":"white","state":\(startingState)}
        """
        guard case .tournamentPairing(let tournamentID, let roomCode, let color, let state) =
            try OnlineCodec.decodeServer(json)
        else {
            return XCTFail("expected tournamentPairing")
        }
        XCTAssertEqual(tournamentID, "t1")
        XCTAssertEqual(roomCode, "AB23CD")
        XCTAssertEqual(color, .white)
        XCTAssertEqual(state.boardFen, "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR")
    }

    func testTournamentStandingsDecoding() throws {
        let json = """
        {"v":1,"type":"tournament_standings","tournament_id":"t1","entries":[{"player_id":"device-a","points":4,"games_played":2}]}
        """
        guard case .tournamentStandings(let tournamentID, let entries) = try OnlineCodec.decodeServer(json) else {
            return XCTFail("expected tournamentStandings")
        }
        XCTAssertEqual(tournamentID, "t1")
        XCTAssertEqual(entries, [OnlineStandingEntry(playerID: "device-a", points: 4, gamesPlayed: 2)])
    }

    func testTournamentEndedDecoding() throws {
        let json = """
        {"v":1,"type":"tournament_ended","tournament_id":"t1","final_standings":[{"player_id":"device-a","points":6,"games_played":3}]}
        """
        guard case .tournamentEnded(let tournamentID, let finalStandings) = try OnlineCodec.decodeServer(json) else {
            return XCTFail("expected tournamentEnded")
        }
        XCTAssertEqual(tournamentID, "t1")
        XCTAssertEqual(finalStandings, [OnlineStandingEntry(playerID: "device-a", points: 6, gamesPlayed: 3)])
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
            guard case .error(let decoded, let message, _) = try OnlineCodec.decodeServer(json) else {
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

    // MARK: - The authentication flows

    func testRegisterCarriesTheEmail() throws {
        let json = try OnlineCodec.encodeClient(
            .register(username: "ana", email: "ana@example.com", password: "correct horse battery", deviceID: "device-a")
        )
        let object = try jsonObject(json)
        XCTAssertEqual(object["type"] as? String, "register")
        XCTAssertEqual(object["username"] as? String, "ana")
        XCTAssertEqual(object["email"] as? String, "ana@example.com")
        XCTAssertEqual(object["password"] as? String, "correct horse battery")
    }

    func testRegisterWithoutAnEmailOmitsTheField() throws {
        let json = try OnlineCodec.encodeClient(
            .register(username: "ana", email: "", password: "correct horse battery", deviceID: nil)
        )
        let object = try jsonObject(json)
        XCTAssertNil(object["email"], "an unset address must be absent, not null")
    }

    func testLoginCarriesTheIdentifier() throws {
        let json = try OnlineCodec.encodeClient(
            .login(identifier: "ana@example.com", password: "correct horse battery", deviceID: nil)
        )
        let object = try jsonObject(json)
        XCTAssertEqual(object["type"] as? String, "login")
        XCTAssertEqual(object["identifier"] as? String, "ana@example.com")
    }

    func testLoginCarriesTheLegacyUsernameToo() throws {
        let json = try OnlineCodec.encodeClient(
            .login(identifier: "ana", password: "correct horse battery", deviceID: nil)
        )
        let object = try jsonObject(json)
        XCTAssertEqual(object["identifier"] as? String, "ana")
        XCTAssertEqual(
            object["username"] as? String, "ana",
            "a server that only knows the legacy field must still be able to decode the frame"
        )
    }

    func testTheEmailFlowsEncode() throws {
        let resume = try jsonObject(try OnlineCodec.encodeClient(.resumeSession(token: "t")))
        XCTAssertEqual(resume["type"] as? String, "resume_session")
        XCTAssertEqual(resume["token"] as? String, "t")

        let forgot = try jsonObject(try OnlineCodec.encodeClient(.forgotPassword(email: "ana@example.com")))
        XCTAssertEqual(forgot["type"] as? String, "forgot_password")

        let reset = try jsonObject(try OnlineCodec.encodeClient(.resetPassword(token: "t", password: "p")))
        XCTAssertEqual(reset["type"] as? String, "reset_password")

        let verify = try jsonObject(try OnlineCodec.encodeClient(.verifyEmail(token: "t")))
        XCTAssertEqual(verify["type"] as? String, "verify_email")

        let resend = try jsonObject(try OnlineCodec.encodeClient(.resendVerification(token: "t")))
        XCTAssertEqual(resend["type"] as? String, "resend_verification")

        let change = try jsonObject(try OnlineCodec.encodeClient(.changeEmail(token: "t", email: "new@example.com")))
        XCTAssertEqual(change["type"] as? String, "change_email")
    }

    func testAnErrorCarriesTheFieldItRefused() throws {
        let json = """
        {"type":"error","v":1,"code":"invalid_request","message":"that is not a valid email address","field":"email"}
        """
        let message = try OnlineCodec.decodeServer(json)
        guard case .error(let code, let text, let field) = message else {
            return XCTFail("expected an error, got \(message)")
        }
        XCTAssertEqual(code, .invalidRequest)
        XCTAssertEqual(text, "that is not a valid email address")
        XCTAssertEqual(field, "email", "the refused field is named machine-readably")
    }

    func testASessionCarriesTheVerificationState() throws {
        let json = """
        {"type":"session","v":1,"account_id":"a","username":"ana","display_name":"ana","token":"t","expires_at_ms":1,"email_verified":true}
        """
        let message = try OnlineCodec.decodeServer(json)
        guard case .session(let token, let username, let displayName, let emailVerified) = message else {
            return XCTFail("expected a session, got \(message)")
        }
        XCTAssertEqual(token, "t")
        XCTAssertEqual(username, "ana")
        XCTAssertEqual(displayName, "ana")
        XCTAssertTrue(emailVerified)
    }

    func testTheEmailRepliesDecode() throws {
        let reset = try OnlineCodec.decodeServer("{\"type\":\"password_reset_requested\",\"v\":1}")
        guard case .passwordResetRequested = reset else {
            return XCTFail("expected a reset acknowledgement, got \(reset)")
        }
        let verified = try OnlineCodec.decodeServer("{\"type\":\"email_verified\",\"v\":1,\"email_verified\":true}")
        guard case .emailVerified(let emailVerified) = verified else {
            return XCTFail("expected a confirmation, got \(verified)")
        }
        XCTAssertTrue(emailVerified)
    }

    // MARK: - The stored session

    func testTheSessionStoreRoundTrips() {
        SessionStore.clear()
        XCTAssertNil(SessionStore.load(), "nothing is stored at first")

        SessionStore.save(token: "a-token")
        XCTAssertEqual(SessionStore.load(), "a-token")

        SessionStore.save(token: "another-token")
        XCTAssertEqual(SessionStore.load(), "another-token", "the last token wins")

        SessionStore.clear()
        XCTAssertNil(SessionStore.load(), "signing out discards the stored session")
    }
}
