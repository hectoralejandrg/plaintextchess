package com.hectoralejandrg.plaintextchess

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    GameScreen()
                }
            }
        }
    }
}

/**
 * Playable two-player chess game screen (milestone 1). Mirrors the iOS
 * `ContentView`: status row, 8x8 [BoardView], UCI move list and a New game
 * action. All game state comes from [GameViewModel], backed by the Rust core.
 */
@Composable
fun GameScreen() {
    val vm = remember { GameViewModel() }
    Column(
        modifier = Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(vertical = 16.dp),
        verticalArrangement = Arrangement.Top,
    ) {
        Text(
            "PlainTextChess",
            style = MaterialTheme.typography.headlineLarge,
            modifier = Modifier.padding(horizontal = 16.dp),
        )
        Spacer(Modifier.height(12.dp))
        Box(Modifier.padding(horizontal = 16.dp)) { StatusRow(vm) }
        Spacer(Modifier.height(8.dp))
        BoardView(vm, Modifier.fillMaxWidth())
        Spacer(Modifier.height(8.dp))
        Box(Modifier.padding(horizontal = 16.dp)) { MoveList(vm) }
        Spacer(Modifier.height(8.dp))
        vm.errorMessage?.let { message ->
            Text(
                text = message,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error,
                modifier = Modifier.padding(horizontal = 16.dp),
            )
            Spacer(Modifier.height(8.dp))
        }
        Button(
            onClick = { vm.newGame() },
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp),
        ) {
            Text("New game")
        }
    }
}

@Composable
private fun StatusRow(vm: GameViewModel) {
    when (val status = vm.status) {
        is GameStatus.Starting ->
            Text("Creating session…", style = MaterialTheme.typography.titleMedium)
        is GameStatus.Playing ->
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    text = if (status.toMove == "w") "White to move" else "Black to move",
                    style = MaterialTheme.typography.titleMedium,
                )
                if (status.inCheck) {
                    Spacer(Modifier.width(8.dp))
                    Text(
                        text = "— Check!",
                        style = MaterialTheme.typography.titleMedium,
                        color = MaterialTheme.colorScheme.error,
                    )
                }
            }
        is GameStatus.Checkmated ->
            Text("Checkmate! ${status.winner} wins", style = MaterialTheme.typography.titleMedium)
        is GameStatus.Drawn ->
            Text("Game drawn", style = MaterialTheme.typography.titleMedium)
        is GameStatus.Failed ->
            Text(
                text = "Game unavailable",
                style = MaterialTheme.typography.titleMedium,
                color = MaterialTheme.colorScheme.error,
            )
    }
}

@Composable
private fun MoveList(vm: GameViewModel) {
    if (vm.moveList.isEmpty()) {
        Text("No moves yet", style = MaterialTheme.typography.bodySmall)
        return
    }
    Column {
        vm.moveList.chunked(2).forEachIndexed { index, pair ->
            Row(Modifier.fillMaxWidth()) {
                Text(
                    text = "${index + 1}.",
                    modifier = Modifier.width(28.dp),
                    style = MaterialTheme.typography.bodySmall,
                    fontFamily = FontFamily.Monospace,
                )
                Text(
                    text = pair[0],
                    modifier = Modifier.width(72.dp),
                    style = MaterialTheme.typography.bodySmall,
                    fontFamily = FontFamily.Monospace,
                )
                if (pair.size > 1) {
                    Text(
                        text = pair[1],
                        style = MaterialTheme.typography.bodySmall,
                        fontFamily = FontFamily.Monospace,
                    )
                }
            }
        }
    }
}
