//! The in-memory authentication authority (design D4): `PlayerAuth` satisfies
//! the `Accounts`, `Sessions`, and `ProfileStore` ports against plain state,
//! and `NoopAuthRecorder` drops every durable write so a run without a
//! database behaves identically to the schema-full one.
//!
//! Durability is *not* this module's job. Reads answer from memory, and each
//! write is mirrored to an `AuthRecorder` by the caller; a recorder failure
//! leaves this store correct in memory and only costs durability.

use std::collections::HashMap;
use std::sync::RwLock;

use crate::application::ports::{
    Accounts, AuthError, AuthRecorder, ProfileStore, RecorderError, SessionLookup, Sessions,
};
use crate::domain::account::{username_key, Account, Profile, Session};

/// The three authentication ports over one in-memory store (design D4).
///
/// Everything lives behind a single `RwLock` because the four maps have to
/// stay consistent with each other: an account is either in *both* the
/// username index and the id map or in neither, and a half-written insert
/// would let a username look taken forever. One lock makes that atomic by
/// construction instead of by careful ordering, and the critical sections
/// here are a few map operations with no `await` inside.
#[derive(Default)]
pub struct PlayerAuthStore {
    state: RwLock<AuthState>,
}

#[derive(Default)]
struct AuthState {
    /// Keyed by the case-folded username, so a lookup cannot miss a case
    /// variant (design D3).
    by_username: HashMap<String, Account>,
    accounts: HashMap<String, Account>,
    /// Device identifier to account identifier. The primary key of the
    /// `profiles` table (design D2), so a device follows one account at a
    /// time.
    device_accounts: HashMap<String, String>,
    profiles: HashMap<String, Profile>,
    /// Token hash to session. Only the hash is ever stored (design D6).
    sessions: HashMap<String, Session>,
}

impl PlayerAuthStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, AuthState> {
        // A poisoned lock means another thread panicked while holding it. The
        // data is still structurally valid (every write replaces whole
        // values), so recovering beats taking the server down.
        self.state.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, AuthState> {
        self.state.write().unwrap_or_else(|e| e.into_inner())
    }

    /// Row counts, for the startup log line (task 9.1).
    pub fn account_count(&self) -> usize {
        self.read().accounts.len()
    }

    pub fn device_count(&self) -> usize {
        self.read().profiles.len()
    }

    pub fn session_count(&self) -> usize {
        self.read().sessions.len()
    }
}

impl Accounts for PlayerAuthStore {
    fn account_by_username(&self, username: &str) -> Option<Account> {
        // Trim before folding, the same way `validate_username` does, so a
        // padded login still resolves to the account it names.
        let key = username_key(username.trim());
        self.read().by_username.get(&key).cloned()
    }

    fn account_by_id(&self, account_id: &str) -> Option<Account> {
        self.read().accounts.get(account_id).cloned()
    }

    fn insert_account(&self, account: Account) -> Result<(), AuthError> {
        let key = account.username_key.clone();
        let mut state = self.write();
        if state.by_username.contains_key(&key) {
            return Err(AuthError::DuplicateUsername);
        }
        if state.accounts.contains_key(&account.account_id) {
            return Err(AuthError::DuplicateAccount);
        }
        state.accounts.insert(account.account_id.clone(), account.clone());
        state.by_username.insert(key, account);
        Ok(())
    }

    fn account_id_of_device(&self, device_id: &str) -> Option<String> {
        self.read().device_accounts.get(device_id).cloned()
    }

    fn link_device(&self, device_id: &str, account_id: &str) {
        // The creation mark is carried over so a relink reads as an update.
        let created_at_ms = self
            .read()
            .profiles
            .get(device_id)
            .map_or(0, |existing| existing.created_at_ms);
        self.create_profile(Profile {
            device_id: device_id.to_string(),
            account_id: account_id.to_string(),
            created_at_ms,
            updated_at_ms: 0,
        });
    }

    fn set_display_name(&self, account_id: &str, display_name: &str) {
        let mut state = self.write();
        let updated = match state.accounts.get_mut(account_id) {
            Some(account) => {
                account.display_name = Some(display_name.to_string());
                // Clone before releasing the borrow so the username route can
                // never serve a stale copy of the record.
                Some(account.clone())
            }
            None => None,
        };
        if let Some(account) = updated {
            state
                .by_username
                .insert(account.username_key.clone(), account);
        }
    }
}

impl Sessions for PlayerAuthStore {
    fn session_by_token_hash(&self, token_hash: &str, now_ms: i64) -> SessionLookup {
        match self.read().sessions.get(token_hash) {
            None => SessionLookup::Absent,
            Some(session) if session.is_revoked() => SessionLookup::Revoked,
            // Expiry is evaluated here, at lookup time, rather than being a
            // stored flag: there is no sweeper to disagree with the row
            // (design D7).
            Some(session) if session.is_expired(now_ms) => SessionLookup::Absent,
            Some(_) => SessionLookup::Usable,
        }
    }

    fn usable_session(&self, token_hash: &str, now_ms: i64) -> Option<Session> {
        match self.session_by_token_hash(token_hash, now_ms) {
            SessionLookup::Usable => self.read().sessions.get(token_hash).cloned(),
            // A revoked or expired session resolves to nothing, exactly like
            // an unknown one: callers fall back to guest play and never learn
            // which of the three it was (spec "Guest Play Fallback").
            SessionLookup::Absent | SessionLookup::Revoked => None,
        }
    }

    fn session_of_token_hash(&self, token_hash: &str) -> Option<Session> {
        self.read().sessions.get(token_hash).cloned()
    }

    fn insert_session(&self, session: Session) {
        self.write()
            .sessions
            .insert(session.token_hash.clone(), session);
    }

    fn revoke_session(&self, token_hash: &str, revoked_at_ms: i64) -> bool {
        let mut state = self.write();
        match state.sessions.get_mut(token_hash) {
            Some(session) if !session.is_revoked() => {
                session.revoked_at_ms = Some(revoked_at_ms);
                true
            }
            // Already revoked, or no such session: nothing to do, and saying
            // so would let a caller claim it revoked a live credential.
            _ => false,
        }
    }

    fn purge_expired(&self, now_ms: i64) -> usize {
        let mut state = self.write();
        let before = state.sessions.len();
        state
            .sessions
            .retain(|_, session| !session.is_expired(now_ms));
        before - state.sessions.len()
    }
}

impl ProfileStore for PlayerAuthStore {
    fn profile_of(&self, device_id: &str) -> Option<Profile> {
        self.read().profiles.get(device_id).cloned()
    }

    /// The single operation that binds a device to an account. It fills
    /// *both* the profile row and the device→account index, so the two can
    /// never disagree — which is what lets startup loading rebuild the
    /// bindings from the `profiles` table alone.
    ///
    /// Insert or replace: a device holds one link, so re-registering the same
    /// device moves it rather than failing.
    fn create_profile(&self, profile: Profile) {
        let mut state = self.write();
        state
            .device_accounts
            .insert(profile.device_id.clone(), profile.account_id.clone());
        state.profiles.insert(profile.device_id.clone(), profile);
    }
}

/// Records nothing and always succeeds: the durable side of a run with no
/// database (`Config::for_test`, `App::new`).
#[derive(Default)]
pub struct NoopAuthRecorder;

fn ok<'a>() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), RecorderError>> + Send + 'a>> {
    Box::pin(async { Ok(()) })
}

impl AuthRecorder for NoopAuthRecorder {
    fn record_account_created<'a>(
        &'a self,
        _account: &'a Account,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), RecorderError>> + Send + 'a>>
    {
        ok()
    }

    fn record_display_name_changed<'a>(
        &'a self,
        _account_id: &'a str,
        _display_name: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), RecorderError>> + Send + 'a>>
    {
        ok()
    }

    fn record_device_linked<'a>(
        &'a self,
        _profile: &'a Profile,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), RecorderError>> + Send + 'a>>
    {
        ok()
    }

    fn record_session_issued<'a>(
        &'a self,
        _session: &'a Session,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), RecorderError>> + Send + 'a>>
    {
        ok()
    }

    fn record_session_revoked<'a>(
        &'a self,
        _token_hash: &'a str,
        _revoked_at_ms: i64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), RecorderError>> + Send + 'a>>
    {
        ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn account(account_id: &str, username: &str) -> Account {
        Account {
            account_id: account_id.to_string(),
            username: username.to_string(),
            username_key: username_key(username),
            password_hash: "$argon2id$fake".to_string(),
            display_name: None,
            created_at_ms: 1_000,
        }
    }

    fn session(token_hash: &str, expires_at_ms: i64) -> Session {
        Session {
            session_id: format!("session-{token_hash}"),
            account_id: "account-1".to_string(),
            device_id: "device-a".to_string(),
            token_hash: token_hash.to_string(),
            created_at_ms: 1_000,
            expires_at_ms,
            revoked_at_ms: None,
        }
    }

    // ------------------------------------------------------------------
    // 3.2 Account lookups
    // ------------------------------------------------------------------

    #[test]
    fn an_inserted_account_is_found_by_id_and_by_username() {
        let store = PlayerAuthStore::new();
        store
            .insert_account(account("account-1", "Ana"))
            .expect("first insert succeeds");

        let by_id = store.account_by_id("account-1").expect("by id");
        assert_eq!(by_id.username, "Ana");
        let by_name = store.account_by_username("Ana").expect("by username");
        assert_eq!(by_name.account_id, "account-1");
        assert_eq!(by_id, by_name, "both routes reach the same record");
    }

    #[test]
    fn the_username_lookup_ignores_case() {
        let store = PlayerAuthStore::new();
        store.insert_account(account("account-1", "Ana")).expect("insert");

        for spelling in ["Ana", "ana", "ANA", "aNa", "  Ana  "] {
            assert_eq!(
                store.account_by_username(spelling).map(|a| a.account_id),
                Some("account-1".to_string()),
                "`{spelling}` must resolve to the same account"
            );
        }
    }

    #[test]
    fn an_unknown_account_is_reported_as_absent() {
        let store = PlayerAuthStore::new();
        assert!(store.account_by_username("nobody").is_none());
        assert!(store.account_by_id("account-404").is_none());
        store.insert_account(account("account-1", "Ana")).expect("insert");
        assert!(
            store.account_by_username("Bob").is_none(),
            "a different username is a miss, not a prefix match"
        );
    }

    #[test]
    fn a_display_name_is_stored_and_reported() {
        let store = PlayerAuthStore::new();
        store.insert_account(account("account-1", "Ana")).expect("insert");
        assert_eq!(
            store.account_by_id("account-1").unwrap().reported_name(),
            "Ana",
            "with no display name the username is reported"
        );

        store.set_display_name("account-1", "Ana T.");
        let updated = store.account_by_id("account-1").expect("still there");
        assert_eq!(updated.display_name.as_deref(), Some("Ana T."));
        assert_eq!(updated.reported_name(), "Ana T.");
        // The username index must point at the same updated record, not a
        // stale copy, or the two reads would disagree.
        assert_eq!(
            store.account_by_username("ana").expect("by username").display_name,
            Some("Ana T.".to_string())
        );
    }

    // ------------------------------------------------------------------
    // 3.3 Uniqueness
    // ------------------------------------------------------------------

    #[test]
    fn a_duplicate_username_is_refused_regardless_of_case() {
        let store = PlayerAuthStore::new();
        store.insert_account(account("account-1", "Ana")).expect("insert");

        assert_eq!(
            store.insert_account(account("account-2", "ana")),
            Err(AuthError::DuplicateUsername),
            "the case-folded key is what makes `ana` collide with `Ana`"
        );
        assert_eq!(
            store.insert_account(account("account-3", "ANA")),
            Err(AuthError::DuplicateUsername)
        );
        // The refusal left the original intact and added nothing.
        assert!(store.account_by_id("account-2").is_none());
        assert!(store.account_by_id("account-3").is_none());
        assert_eq!(store.account_by_id("account-1").unwrap().username, "Ana");
    }

    #[test]
    fn a_duplicate_account_id_is_refused() {
        let store = PlayerAuthStore::new();
        store.insert_account(account("account-1", "Ana")).expect("insert");
        assert_eq!(
            store.insert_account(account("account-1", "Bob")),
            Err(AuthError::DuplicateAccount),
            "a reused account_id is refused even under a free username"
        );
        assert!(
            store.account_by_username("Bob").is_none(),
            "the refused insert must not leave a half-written username index entry"
        );
    }

    #[test]
    fn the_device_index_stays_consistent_after_a_refused_insert() {
        let store = PlayerAuthStore::new();
        store.insert_account(account("account-1", "Ana")).expect("insert");
        assert!(store.insert_account(account("account-2", "Ana")).is_err());
        assert!(store.account_by_id("account-2").is_none());
        let state = store.read();
        assert_eq!(state.by_username.len(), 1);
        assert_eq!(state.accounts.len(), 1);
    }

    // ------------------------------------------------------------------
    // 3.2 / 3.3 Device links
    // ------------------------------------------------------------------

    #[test]
    fn a_linked_device_resolves_to_its_account() {
        let store = PlayerAuthStore::new();
        store.insert_account(account("account-1", "Ana")).expect("insert");
        assert_eq!(store.account_id_of_device("device-a"), None);

        store.link_device("device-a", "account-1");
        assert_eq!(
            store.account_id_of_device("device-a").as_deref(),
            Some("account-1")
        );
        let profile = store.profile_of("device-a").expect("the link is a profile row");
        assert_eq!(profile.account_id, "account-1");
        assert_eq!(profile.device_id, "device-a");
    }

    #[test]
    fn a_device_follows_the_account_it_most_recently_signed_in_as() {
        let store = PlayerAuthStore::new();
        store.insert_account(account("account-1", "Ana")).expect("insert");
        store.insert_account(account("account-2", "Bob")).expect("insert");
        store.link_device("device-a", "account-1");
        store.link_device("device-a", "account-2");

        assert_eq!(
            store.account_id_of_device("device-a").as_deref(),
            Some("account-2"),
            "re-linking replaces the previous binding"
        );
        assert_eq!(
            store.profile_of("device-a").unwrap().account_id,
            "account-2",
            "one device holds exactly one profile row"
        );
    }

    #[test]
    fn an_unlinked_device_has_no_profile() {
        let store = PlayerAuthStore::new();
        assert!(store.profile_of("device-z").is_none());
        assert_eq!(store.account_id_of_device("device-z"), None);
    }

    // ------------------------------------------------------------------
    // 3.2 / 3.4 Sessions
    // ------------------------------------------------------------------

    #[test]
    fn a_live_session_resolves_to_its_record() {
        let store = PlayerAuthStore::new();
        store.insert_session(session("hash-1", 5_000));
        assert_eq!(store.session_by_token_hash("hash-1", 1_500), SessionLookup::Usable);
        let resolved = store.usable_session("hash-1", 1_500).expect("usable");
        assert_eq!(resolved.account_id, "account-1");
        assert_eq!(resolved.device_id, "device-a");
        assert_eq!(resolved.token_hash, "hash-1");
    }

    #[test]
    fn an_expired_session_is_reported_as_absent() {
        let store = PlayerAuthStore::new();
        store.insert_session(session("hash-1", 5_000));
        // Expiry is checked against expires_at_ms at lookup time, not stored:
        // the same row answers Usable before the mark and Absent after it,
        // with no write in between.
        assert_eq!(store.session_by_token_hash("hash-1", 4_999), SessionLookup::Usable);
        assert_eq!(store.session_by_token_hash("hash-1", 5_000), SessionLookup::Absent);
        assert_eq!(store.session_by_token_hash("hash-1", 9_999), SessionLookup::Absent);
        assert!(
            store.usable_session("hash-1", 5_000).is_none(),
            "an expired token yields no session, so the caller falls back to guest"
        );
    }

    #[test]
    fn a_revoked_session_is_reported_as_revoked_not_absent() {
        let store = PlayerAuthStore::new();
        store.insert_session(session("hash-1", 9_999_999));
        assert_eq!(store.session_by_token_hash("hash-1", 1_500), SessionLookup::Usable);

        assert!(store.revoke_session("hash-1", 2_000), "a live session revokes");
        assert_eq!(
            store.session_by_token_hash("hash-1", 2_000),
            SessionLookup::Revoked,
            "revocation is distinguishable from expiry, which is Absent"
        );
        assert!(
            store.usable_session("hash-1", 2_000).is_none(),
            "but neither resolves to a usable session"
        );
    }

    #[test]
    fn revoking_touches_only_the_presented_session() {
        let store = PlayerAuthStore::new();
        store.insert_session(session("hash-1", 9_999_999));
        store.insert_session(session("hash-2", 9_999_999));

        assert!(store.revoke_session("hash-1", 2_000));
        assert_eq!(store.session_by_token_hash("hash-1", 2_000), SessionLookup::Revoked);
        assert_eq!(
            store.session_by_token_hash("hash-2", 2_000),
            SessionLookup::Usable,
            "the account's other session survives: phone and tablet stay signed in"
        );
    }

    #[test]
    fn revoking_an_unknown_or_already_revoked_session_reports_no_change() {
        let store = PlayerAuthStore::new();
        assert!(
            !store.revoke_session("hash-unknown", 1),
            "there is nothing to revoke"
        );
        store.insert_session(session("hash-1", 9_999_999));
        assert!(store.revoke_session("hash-1", 1));
        assert!(
            !store.revoke_session("hash-1", 2),
            "a second revoke must not claim to have revoked a live credential"
        );
        let record = store
            .read()
            .sessions
            .get("hash-1")
            .expect("the row is still there, revoked")
            .clone();
        assert_eq!(
            record.revoked_at_ms,
            Some(1),
            "the original revocation mark is kept"
        );
    }

    #[test]
    fn an_unknown_token_is_absent() {
        let store = PlayerAuthStore::new();
        assert_eq!(store.session_by_token_hash("nope", 1), SessionLookup::Absent);
        assert!(store.usable_session("nope", 1).is_none());
    }

    #[test]
    fn purging_removes_only_expired_sessions() {
        let store = PlayerAuthStore::new();
        store.insert_session(session("hash-expired", 1_000));
        store.insert_session(session("hash-live", 1_000_000));
        store.insert_session(session("hash-revoked-but-unexpired", 1_000_000));
        store.revoke_session("hash-revoked-but-unexpired", 1_500);

        assert_eq!(store.purge_expired(2_000), 1, "one row expired");
        assert_eq!(store.session_by_token_hash("hash-expired", 2_000), SessionLookup::Absent);
        assert_eq!(store.session_by_token_hash("hash-live", 2_000), SessionLookup::Usable);
        assert_eq!(
            store.session_by_token_hash("hash-revoked-but-unexpired", 2_000),
            SessionLookup::Revoked,
            "a revoked session is still within its lifetime, so it is kept"
        );
        assert_eq!(store.purge_expired(2_000), 0, "purging again is a no-op");
    }

    #[test]
    fn purging_does_not_touch_accounts_or_device_links() {
        let store = PlayerAuthStore::new();
        store.insert_account(account("account-1", "Ana")).expect("insert");
        store.link_device("device-a", "account-1");
        store.insert_session(session("hash-1", 1_000));

        assert_eq!(store.purge_expired(9_999), 1);
        assert!(
            store.account_by_id("account-1").is_some(),
            "session expiry must never log an account out"
        );
        assert_eq!(store.account_id_of_device("device-a").as_deref(), Some("account-1"));
    }

    // ------------------------------------------------------------------
    // 3.5 The no-op recorder
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn the_noop_recorder_succeeds_and_persists_nothing() {
        let store = PlayerAuthStore::new();
        let recorder = NoopAuthRecorder;

        let account = account("account-1", "Ana");
        recorder.record_account_created(&account).await.expect("noop succeeds");
        store.insert_account(account.clone()).expect("insert");

        let profile = Profile {
            device_id: "device-a".to_string(),
            account_id: "account-1".to_string(),
            created_at_ms: 1_000,
            updated_at_ms: 1_000,
        };
        recorder.record_device_linked(&profile).await.expect("noop succeeds");
        store.link_device("device-a", "account-1");

        let issued = session("hash-1", 9_999_999);
        recorder.record_session_issued(&issued).await.expect("noop succeeds");
        store.insert_session(issued.clone());

        recorder.record_session_revoked("hash-1", 5_000).await.expect("noop succeeds");
        assert!(store.revoke_session("hash-1", 5_000));

        recorder
            .record_display_name_changed("account-1", "Ana T.")
            .await
            .expect("noop succeeds");
        store.set_display_name("account-1", "Ana T.");

        // Every outcome is identical to a run with a real database: the
        // in-memory store is the authority, so the recorder only ever affects
        // durability, never behavior.
        assert_eq!(store.session_by_token_hash("hash-1", 5_000), SessionLookup::Revoked);
        assert_eq!(store.account_by_id("account-1").unwrap().reported_name(), "Ana T.");
        assert_eq!(store.account_id_of_device("device-a").as_deref(), Some("account-1"));
    }

    #[tokio::test]
    async fn the_noop_recorder_accepts_every_call() {
        let recorder = NoopAuthRecorder;
        let account = account("account-1", "Ana");
        let profile = Profile {
            device_id: "device-a".to_string(),
            account_id: "account-1".to_string(),
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        let issued = session("hash-1", 1);
        recorder.record_account_created(&account).await.expect("ok");
        recorder.record_display_name_changed("a", "n").await.expect("ok");
        recorder.record_device_linked(&profile).await.expect("ok");
        recorder.record_session_issued(&issued).await.expect("ok");
        recorder.record_session_revoked("hash-1", 1).await.expect("ok");
    }

    // ------------------------------------------------------------------
    // Concurrency: the store is shared across every connection
    // ------------------------------------------------------------------

    #[test]
    fn concurrent_links_and_inserts_stay_consistent() {
        use std::thread;

        let store = Arc::new(PlayerAuthStore::new());
        store.insert_account(account("account-1", "Ana")).expect("insert");

        let mut handles = Vec::new();
        for i in 0..8 {
            let store = Arc::clone(&store);
            handles.push(thread::spawn(move || {
                store.link_device(&format!("device-{i}"), "account-1");
                store
                    .insert_account(account(&format!("account-extra-{i}"), &format!("user{i}")))
                    .expect("a fresh username is free");
                let _ = store.insert_account(account("account-1", "Ana"));
            }));
        }
        for handle in handles {
            handle.join().expect("thread");
        }

        for i in 0..8 {
            assert_eq!(
                store.account_id_of_device(&format!("device-{i}")).as_deref(),
                Some("account-1")
            );
        }
        assert_eq!(store.read().accounts.len(), 9, "1 + 8 extra accounts");
        assert_eq!(
            store.read().by_username.len(),
            9,
            "every username index entry is still present"
        );
    }
}