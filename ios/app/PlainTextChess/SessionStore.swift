import Foundation
import Security

/// Where the session token lives (spec "Client keeps the session across
/// launches").
///
/// The token is a bearer credential for the account, so it is kept in the
/// system keychain and never in ordinary application storage: not in the
/// preferences, not in a file, not in the app's container.
enum SessionStore {
    private static let service = "com.hectoralejandrg.plaintextchess.session"
    private static let account = "session-token"
    /// The "keep me signed in" choice is a preference, not a secret: it
    /// lives in ordinary preferences.
    private static let keepSessionKey = "plaintextchess.keep_session"

    /// Store the token, replacing any previous one.
    static func save(token: String) {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        // Replace rather than update: the item may not exist yet.
        SecItemDelete(query as CFDictionary)
        var attributes = query
        attributes[kSecValueData as String] = Data(token.utf8)
        attributes[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
        var status = SecItemAdd(attributes as CFDictionary, nil)
        if status != errSecSuccess {
            // A keychain that refuses the write means the session is not
            // stored: the player signs in again next time, which is better
            // than a silent failure.
            NSLog("PlainTextChess: the session could not be stored (keychain error %d)", status)
        }
    }

    /// The stored token, when there is one.
    static func load() -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var item: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess,
              let data = item as? Data,
              let token = String(data: data, encoding: .utf8),
              !token.isEmpty
        else {
            return nil
        }
        return token
    }

    /// Whether the session is kept across launches: the player's choice.
    static func keepSession() -> Bool {
        UserDefaults.standard.object(forKey: keepSessionKey) as? Bool ?? true
    }

    /// Record the player's choice.
    static func setKeepSession(_ keep: Bool) {
        UserDefaults.standard.set(keep, forKey: keepSessionKey)
    }

    /// Forget the stored token.
    static func clear() {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        SecItemDelete(query as CFDictionary)
    }
}
