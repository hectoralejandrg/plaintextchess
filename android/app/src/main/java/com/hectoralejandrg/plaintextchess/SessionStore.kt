package com.hectoralejandrg.plaintextchess

import android.content.Context
import android.util.Log
import android.content.SharedPreferences
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey

/**
 * Where the session token lives (spec "Client keeps the session across
 * launches").
 *
 * The token is a bearer credential for the account, so it is kept in
 * encrypted shared preferences and never in ordinary application storage.
 */
object SessionStore {
    private const val PREFS = "plaintextchess.session"
    private const val KEY_TOKEN = "session_token"
    private const val KEY_KEEP = "keep_session"

    /**
     * The encrypted preferences, created once per process.
     *
     * Creating them is expensive (the keystore round trip), so the instance
     * is cached: the view model asks for it on every launch, and a failure
     * in the keystore must not take the app down with it.
     */
    private var cached: SharedPreferences? = null

    private fun prefs(context: Context): SharedPreferences? {
        cached?.let { return it }
        return try {
            val masterKey = MasterKey.Builder(context)
                .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
                .build()
            EncryptedSharedPreferences.create(
                context,
                PREFS,
                masterKey,
                EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
                EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
            ).also { cached = it }
        } catch (e: Exception) {
            // No keystore: the session is not stored, and the player signs in
            // again next time. The app still works.
            Log.w("PlainTextChess", "the session could not be stored: ${e.message}")
            null
        }
    }

    /** Store the token, replacing any previous one. */
    fun save(context: Context, token: String) {
        prefs(context)?.edit()?.putString(KEY_TOKEN, token)?.apply()
    }

    /** The stored token, when there is one. */
    fun load(context: Context): String? =
        prefs(context)?.getString(KEY_TOKEN, null)?.takeIf { it.isNotEmpty() }

    /** Forget the stored token. */
    fun clear(context: Context) {
        prefs(context)?.edit()?.remove(KEY_TOKEN)?.apply()
    }

    /** Whether the session is kept across launches: the player's choice. */
    fun keepSession(context: Context): Boolean =
        prefs(context)?.getBoolean(KEY_KEEP, true) ?: true

    /** Record the player's choice. */
    fun setKeepSession(context: Context, keep: Boolean) {
        prefs(context)?.edit()?.putBoolean(KEY_KEEP, keep)?.apply()
    }
}
