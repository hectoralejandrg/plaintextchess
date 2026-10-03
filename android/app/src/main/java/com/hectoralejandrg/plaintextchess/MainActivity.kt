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
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
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
    var showNewGameSheet by remember { mutableStateOf(false) }
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
        // Game controls (add-game-end-dialog D6): secondary actions above the
        // prominent New game action, with the spec's availability rules.
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            OutlinedButton(
                onClick = { vm.undo() },
                enabled = vm.canUndo,
                modifier = Modifier.weight(1f),
            ) { Text("Undo") }
            OutlinedButton(
                onClick = { vm.resign() },
                enabled = vm.canResign,
                modifier = Modifier.weight(1f),
            ) { Text("Resign") }
            OutlinedButton(
                onClick = { vm.flipBoard() },
                modifier = Modifier.weight(1f),
            ) { Text("Flip board") }
        }
        Spacer(Modifier.height(8.dp))
        Button(
            onClick = { showNewGameSheet = true },
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp),
        ) {
            Text("New game")
        }
        if (showNewGameSheet) {
            NewGameSetupSheet(
                onDismiss = { showNewGameSheet = false },
                onConfirm = { mode ->
                    vm.startGame(mode)
                    showNewGameSheet = false
                },
            )
        }
        // Game-end dialog (design D1/D2 of add-game-end-dialog): presented
        // exactly once per game; "Play again" restarts in the same mode and
        // "Done" just closes the modal.
        if (vm.showGameEndDialog) {
            AlertDialog(
                onDismissRequest = { vm.dismissGameEnd() },
                title = { Text("Game over") },
                text = { Text(vm.gameEndMessage ?: "The game is over.") },
                confirmButton = {
                    TextButton(onClick = { vm.restart() }) { Text("Play again") }
                },
                dismissButton = {
                    TextButton(onClick = { vm.dismissGameEnd() }) { Text("Done") }
                },
            )
        }
    }
}

/**
 * New-game setup sheet (design D4): choose the opponent (two players or CPU)
 * and, for the CPU, its difficulty. Two players is pre-selected; "Start"
 * begins the game with the chosen mode.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun NewGameSetupSheet(
    onDismiss: () -> Unit,
    onConfirm: (GameMode) -> Unit,
) {
    var isCpu by remember { mutableStateOf(false) }
    var difficulty by remember { mutableStateOf(CpuDifficulty.MEDIUM) }
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier
                .padding(horizontal = 16.dp)
                .padding(bottom = 32.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text("New game", style = MaterialTheme.typography.titleLarge)
            Text(
                "Opponent",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                FilterChip(
                    selected = !isCpu,
                    onClick = { isCpu = false },
                    label = { Text("Two players") },
                )
                FilterChip(
                    selected = isCpu,
                    onClick = { isCpu = true },
                    label = { Text("CPU") },
                )
            }
            if (isCpu) {
                Text(
                    "Difficulty",
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    CpuDifficulty.values().forEach { level ->
                        FilterChip(
                            selected = difficulty == level,
                            onClick = { difficulty = level },
                            label = { Text(level.displayName) },
                        )
                    }
                }
            }
            Button(
                onClick = {
                    onConfirm(if (isCpu) GameMode.Cpu(difficulty) else GameMode.TwoPlayers)
                },
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text("Start")
            }
        }
    }
}

@Composable
private fun StatusRow(vm: GameViewModel) {
    if (vm.cpuThinking) {
        Text("CPU is thinking…", style = MaterialTheme.typography.titleMedium)
        return
    }
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
        is GameStatus.Resigned ->
            Text(
                text = if (status.winner == "White") "Black resigns" else "White resigns",
                style = MaterialTheme.typography.titleMedium,
            )
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
