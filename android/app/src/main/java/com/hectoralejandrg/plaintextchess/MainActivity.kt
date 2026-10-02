package com.hectoralejandrg.plaintextchess

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import uniffi.chess_core.newGameSession

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    PlaceholderScreen()
                }
            }
        }
    }
}

/** Data captured from a freshly created [uniffi.chess_core.GameSession]. */
private data class SessionInfo(val boardState: String, val rating: Double)

/**
 * Placeholder screen that proves the ChessCore FFI surface works in-app:
 * it creates a game session through the UniFFI Kotlin bindings and shows
 * the initial board state (FEN) and the starting player rating. FFI errors
 * are surfaced in the UI instead of crashing.
 */
@Composable
fun PlaceholderScreen() {
    var info by remember { mutableStateOf<SessionInfo?>(null) }
    var error by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(Unit) {
        try {
            val session = newGameSession(initialRating = 1500.0)
            info = SessionInfo(
                boardState = session.getBoardState(),
                rating = session.getCurrentRating()
            )
        } catch (e: Throwable) {
            error = e.message ?: e.toString()
        }
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(16.dp),
        verticalArrangement = Arrangement.Top
    ) {
        Text("PlainTextChess", style = MaterialTheme.typography.headlineLarge)
        Spacer(modifier = Modifier.height(12.dp))
        when {
            info != null -> {
                val session = info!!
                Text("Board state (FEN)", style = MaterialTheme.typography.titleMedium)
                Text(
                    text = session.boardState,
                    style = MaterialTheme.typography.bodyLarge,
                    fontFamily = FontFamily.Monospace
                )
                Spacer(modifier = Modifier.height(8.dp))
                Text(
                    text = "Player rating ${session.rating}",
                    style = MaterialTheme.typography.titleMedium
                )
            }
            error != null -> {
                Text(
                    text = "FFI error",
                    style = MaterialTheme.typography.titleMedium,
                    color = MaterialTheme.colorScheme.error
                )
                Text(
                    text = error!!,
                    fontFamily = FontFamily.Monospace
                )
            }
            else -> Text("Creating session…")
        }
    }
}
