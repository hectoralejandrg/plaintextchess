package com.hectoralejandrg.plaintextchess

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.pm.ApplicationInfo
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import kotlinx.coroutines.delay

class MainActivity : ComponentActivity() {

    /**
     * DEBUG-only verification hook (enforce-single-active-game D6):
     * milliseconds to hold the CPU "thinking" state before applying the
     * computed move, so Resign/Undo taps can deterministically land "while
     * the CPU is thinking" on the emulator. Release-inert (non-debuggable
     * builds always get 0): the module does not enable `BuildConfig`, so the
     * debuggable flag is the gate.
     * `adb shell am start -n com.hectoralejandrg.plaintextchess/.MainActivity
     * --ei cpu_delay_ms 3000`
     */
    private val debugCpuDelayMs: Long
        get() {
            val debuggable =
                (applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE) != 0
            if (!debuggable) return 0L
            return intent.getIntExtra("cpu_delay_ms", 0).toLong().coerceAtLeast(0L)
        }

    /**
     * DEBUG-only override (add-online-multiplayer D9): the online server URL
     * from the `online_url` intent extra; only passed in debuggable builds,
     * so it is release-inert. On the emulator the host is reached through
     * the loopback alias:
     * `adb shell am start -n com.hectoralejandrg.plaintextchess/.MainActivity
     * --es online_url ws://10.0.2.2:8765/ws`
     */
    private val onlineUrl: String?
        get() {
            val debuggable =
                (applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE) != 0
            if (!debuggable) return null
            return intent.getStringExtra("online_url")?.trim()?.ifEmpty { null }
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    AppNavHost(debugCpuDelayMs = debugCpuDelayMs, onlineUrl = onlineUrl)
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
fun GameScreen(vm: GameViewModel = GameViewModel(LocalContext.current)) {
    val context = LocalContext.current
    var showNewGameSheet by remember { mutableStateOf(false) }
    // The setup sheet auto-dismisses once the server confirms an online seat
    // (add-online-multiplayer D4); a join error keeps it open instead.
    LaunchedEffect(vm.onlineSessionReady) {
        if (vm.onlineSessionReady) {
            showNewGameSheet = false
        }
    }
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
        if (vm.isOnlineMode && vm.onlineYourColor != null) {
            Text(
                text = "You are " + if (vm.onlineYourColor == "w") "White" else "Black",
                style = MaterialTheme.typography.titleSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 16.dp),
            )
        }
        Spacer(Modifier.height(12.dp))
        Box(Modifier.padding(horizontal = 16.dp)) { StatusRow(vm) }
        Spacer(Modifier.height(8.dp))
        if (vm.isOnlineMode) {
            // Online clocks (add-online-time-controls, design D6): a fixed
            // height row so the full-width board keeps its exact size.
            OnlineClockRow(vm)
            Spacer(Modifier.height(8.dp))
        }
        // Online waiting banner (add-online-multiplayer D4): the 6-character
        // room code, large and monospaced, with a copy action next to it.
        (vm.onlinePhase as? OnlinePhase.Waiting)?.let { waiting ->
            WaitingBanner(
                code = waiting.code,
                onCopy = { copyToClipboard(context, waiting.code) },
            )
            Spacer(Modifier.height(8.dp))
        }
        // The board always spans the full screen width; the reconnection
        // badge overlays it without taking layout space (add-online-multiplayer
        // D7): the last known position stays on screen while the manager
        // re-attaches.
        Box(Modifier.fillMaxWidth()) {
            BoardView(vm, Modifier.fillMaxWidth())
            if (vm.onlineReconnecting) {
                Text(
                    text = "Reconnecting…",
                    style = MaterialTheme.typography.titleMedium,
                    color = MaterialTheme.colorScheme.onSurface,
                    modifier = Modifier
                        .align(Alignment.Center)
                        .background(
                            color = MaterialTheme.colorScheme.primary.copy(alpha = 0.92f),
                            shape = RoundedCornerShape(8.dp),
                        )
                        .padding(horizontal = 14.dp, vertical = 8.dp),
                )
            }
        }
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
        // New game is available only when the current game is finished, has
        // no move played yet, or failed (enforce-single-active-game D1): it
        // stays visible but greyed out mid-game and starts nothing when
        // tapped.
        Button(
            onClick = {
                // Reset the online-join handshake flags so a stale "ready"
                // from a previous attempt cannot auto-dismiss this sheet
                // (add-online-multiplayer D4).
                vm.resetOnlineSheetState()
                showNewGameSheet = true
            },
            enabled = vm.canStartNewGame,
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp),
        ) {
            Text("New game")
        }
        if (showNewGameSheet) {
            NewGameSetupSheet(
                vm = vm,
                onDismiss = { showNewGameSheet = false },
                onConfirm = { mode ->
                    vm.startGame(mode)
                    showNewGameSheet = false
                },
                onOnline = { create, code, timeControl ->
                    if (vm.startOnlineGame(create = create, code = code,
                                           timeControl = timeControl) && create) {
                        // Creating closes the sheet immediately; a join stays
                        // open until the server confirms the seat (auto-close
                        // above) or shows the join error inside the sheet.
                        showNewGameSheet = false
                    }
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
                    if (vm.isOnlineMode) {
                        // Online: the room is finished, so the modal offers
                        // only "Done" (no Play again, add-online-multiplayer D4).
                        TextButton(onClick = { vm.dismissGameEnd() }) { Text("Done") }
                    } else {
                        TextButton(onClick = { vm.restart() }) { Text("Play again") }
                    }
                },
                dismissButton = {
                    if (!vm.isOnlineMode) {
                        TextButton(onClick = { vm.dismissGameEnd() }) { Text("Done") }
                    }
                },
            )
        }
    }
}

/**
 * New-game setup sheet (design D4 + add-online-multiplayer D4): choose the
 * opponent (two players, CPU, or online) and, for the CPU, its difficulty;
 * for online, create a room or join one by code. Two players is
 * pre-selected; "Start" begins the game with the chosen mode. The join error
 * is shown inside the sheet, which stays open for it (a create/join success
 * dismisses the sheet from the caller).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun NewGameSetupSheet(
    vm: GameViewModel,
    onDismiss: () -> Unit,
    onConfirm: (GameMode) -> Unit,
    onOnline: (create: Boolean, code: String?, timeControl: String?) -> Unit,
) {
    var isCpu by remember { mutableStateOf(false) }
    var isOnline by remember { mutableStateOf(false) }
    var difficulty by remember { mutableStateOf(CpuDifficulty.MEDIUM) }
    var roomCode by remember { mutableStateOf("") }
    var timeControl by remember { mutableStateOf(OnlineTimeControl.default) }
    var isAuth by remember { mutableStateOf(false) }
    var authChoice by remember { mutableStateOf<AuthChoice?>(null) }
    var authUsername by remember { mutableStateOf("") }
    var authPassword by remember { mutableStateOf("") }
    var authDisplayName by remember { mutableStateOf("") }
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
            // Horizontally scrollable: on narrow screens the auth chips
            // (Register/Login/Logout) would otherwise be pushed off-screen.
            Row(
                modifier = Modifier.horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                FilterChip(
                    selected = !isCpu && !isOnline && !isAuth,
                    onClick = {
                        isCpu = false
                        isOnline = false
                        isAuth = false
                        authChoice = null
                    },
                    label = { Text("Two players") },
                )
                FilterChip(
                    selected = isCpu,
                    onClick = {
                        isCpu = true
                        isOnline = false
                    },
                    label = { Text("CPU") },
                )
                FilterChip(
                    selected = isOnline,
                    onClick = {
                        isOnline = true
                        isCpu = false
                        isAuth = false
                        authChoice = null
                    },
                    label = { Text("Online") },
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
            if (isOnline) {
                Text(
                    "Time control",
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    OnlineTimeControl.presets.forEach { preset ->
                        FilterChip(
                            selected = timeControl == preset,
                            onClick = { timeControl = preset },
                            label = { Text(preset.label) },
                        )
                    }
                }
                Text(
                    "Room code",
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                OutlinedTextField(
                    value = roomCode,
                    onValueChange = { roomCode = it.uppercase() },
                    label = { Text("Code to join (optional)") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                vm.onlineJoinError?.let { error ->
                    Text(
                        text = error,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(
                        onClick = { onOnline(true, null, timeControl.label) },
                        modifier = Modifier.weight(1f),
                    ) { Text("Create room") }
                    Button(
                        onClick = { onOnline(false, roomCode, null) },
                        enabled = roomCode.isNotEmpty(),
                        modifier = Modifier.weight(1f),
                    ) { Text("Join room") }
                }
            }
            if (!isOnline && !isAuth) {
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
}

/**
 * Online clocks (add-online-time-controls, design D6): the side on the top
 * edge of the board first, then the side on the bottom edge, each labeled by
 * color so the two clocks are never confusable. The side-to-move clock is
 * emphasized and turns red at ≤ 10 s. A 100 ms tick interpolates the
 * countdown between authoritative server snapshots.
 */
@Composable
private fun OnlineClockRow(vm: GameViewModel) {
    val clock = vm.onlineClock ?: return
    var nowMillis by remember { mutableStateOf(System.currentTimeMillis()) }
    LaunchedEffect(clock) {
        while (true) {
            nowMillis = System.currentTimeMillis()
            delay(100)
        }
    }
    val white = clock.remainingMs("w", nowMillis)
    val black = clock.remainingMs("b", nowMillis)
    val topColor = if (vm.boardOrientation == 0) "Black" else "White"
    val bottomColor = if (vm.boardOrientation == 0) "White" else "Black"
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        ClockCell(
            color = topColor,
            milliseconds = if (topColor == "White") white else black,
            active = clock.isRunning && clock.sideToMove == if (topColor == "White") "w" else "b",
        )
        Text(
            text = vm.onlineTimeControl,
            style = MaterialTheme.typography.labelMedium,
            fontFamily = FontFamily.Monospace,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        ClockCell(
            color = bottomColor,
            milliseconds = if (bottomColor == "White") white else black,
            active = clock.isRunning && clock.sideToMove == if (bottomColor == "White") "w" else "b",
        )
    }
}

@Composable
private fun ClockCell(color: String, milliseconds: Int, active: Boolean) {
    val low = milliseconds <= 10_000
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        modifier = Modifier
            .background(
                color = if (active) {
                    MaterialTheme.colorScheme.primary.copy(alpha = 0.18f)
                } else {
                    Color.Transparent
                },
                shape = RoundedCornerShape(8.dp),
            )
            .padding(horizontal = 12.dp, vertical = 4.dp),
    ) {
        Text(
            text = color,
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(
            text = OnlineClock.format(milliseconds),
            style = MaterialTheme.typography.titleMedium,
            fontFamily = FontFamily.Monospace,
            color = if (low) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurface,
        )
    }
}

/**
 * Online waiting banner (add-online-multiplayer D4): the 6-character room
 * code, large and monospaced, with a copy action next to it.
 */
@Composable
private fun WaitingBanner(code: String, onCopy: () -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp)
            .padding(vertical = 10.dp)
            .background(
                color = MaterialTheme.colorScheme.surfaceVariant,
                shape = RoundedCornerShape(10.dp),
            )
            .padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Column {
            Text(
                "Room code",
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                code,
                style = MaterialTheme.typography.titleLarge,
                fontFamily = FontFamily.Monospace,
            )
        }
        TextButton(onClick = onCopy) { Text("Copy") }
    }
}

/** Copy the room code to the clipboard (add-online-multiplayer D4). */
private fun copyToClipboard(context: Context, text: String) {
    val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
    clipboard.setPrimaryClip(ClipData.newPlainText("Room code", text))
}

@Composable
private fun StatusRow(vm: GameViewModel) {
    when {
        vm.onlineReconnecting ->
            // Online: the socket dropped mid-game; the last known position
            // stays on screen (add-online-multiplayer D7).
            Text(
                "Reconnecting…",
                style = MaterialTheme.typography.titleMedium,
                color = Color(0xFFFF9500), // system orange, as on iOS
            )
        vm.onlinePhase is OnlinePhase.Waiting ->
            Text("Waiting for opponent to join…", style = MaterialTheme.typography.titleMedium)
        vm.cpuThinking ->
            Text("CPU is thinking…", style = MaterialTheme.typography.titleMedium)
        else -> when (val status = vm.status) {
            is GameStatus.Starting ->
                Text(
                    text = if (vm.isOnlineMode) "Connecting to online server…"
                    else "Creating session…",
                    style = MaterialTheme.typography.titleMedium,
                )
            is GameStatus.Playing ->
                Row(verticalAlignment = Alignment.CenterVertically) {
                    val onlineOwnTurn =
                        vm.isOnlineMode && status.toMove == vm.onlineYourColor
                    val onlineOpponentTurn = vm.isOnlineMode && !onlineOwnTurn
                    Text(
                        text = when {
                            onlineOwnTurn -> "Your move"
                            onlineOpponentTurn -> "Opponent to move"
                            else -> if (status.toMove == "w") "White to move" else "Black to move"
                        },
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
            is GameStatus.Forfeited ->
                Text(
                    text = if (status.winner == "White") "Black forfeits" else "White forfeits",
                    style = MaterialTheme.typography.titleMedium,
                )
            is GameStatus.TimedOut ->
                Text(
                    text = if (status.winner == "White") "Black ran out of time"
                    else "White ran out of time",
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

@Composable
fun AppNavHost(debugCpuDelayMs: Long = 0L, onlineUrl: String? = null) {
    val navController = rememberNavController()
    val context = LocalContext.current
    val vm = remember {
        val model = GameViewModel(context)
        model.debugCpuDelayMs = debugCpuDelayMs
        model.debugOnlineURL = onlineUrl
        model.restoreOnlineSessionIfNeeded()
        model
    }

    NavHost(navController = navController, startDestination = "login") {
        composable("login") {
            LoginScreen(
                vm = vm,
                onLoginSuccess = { navController.navigate("home") { popUpTo("login") { inclusive = true } } },
                onGuest = { navController.navigate("home") { popUpTo("login") { inclusive = true } } }
            )
        }
        composable("home") {
            HomeScreen(
                vm = vm,
                onStartGame = { navController.navigate("game") },
                onGoToLogin = { navController.navigate("login") { popUpTo("home") { inclusive = true } } }
            )
        }
        composable("game") {
            GameDestination(vm = vm)
        }
    }
}

@Composable
fun LoginScreen(
    vm: GameViewModel,
    onLoginSuccess: () -> Unit,
    onGuest: () -> Unit
) {
    var username by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(16.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally
    ) {
        Text(
            "PlainTextChess",
            style = MaterialTheme.typography.headlineLarge,
            modifier = Modifier.padding(bottom = 32.dp)
        )
        OutlinedTextField(
            value = username,
            onValueChange = { username = it },
            label = { Text("Username") },
            singleLine = true,
            modifier = Modifier.fillMaxWidth()
        )
        Spacer(Modifier.height(8.dp))
        OutlinedTextField(
            value = password,
            onValueChange = { password = it },
            label = { Text("Password") },
            singleLine = true,
            visualTransformation = PasswordVisualTransformation(),
            modifier = Modifier.fillMaxWidth()
        )
        Spacer(Modifier.height(16.dp))
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(8.dp)
        ) {
            Button(
                onClick = { vm.doRegister(username, password) },
                enabled = username.isNotBlank() && password.isNotEmpty(),
                modifier = Modifier.weight(1f)
            ) { Text("Register") }
            Button(
                onClick = { vm.doLogin(username, password) },
                enabled = username.isNotBlank() && password.isNotEmpty(),
                modifier = Modifier.weight(1f)
            ) { Text("Login") }
        }
        Spacer(Modifier.height(8.dp))
        OutlinedButton(
            onClick = { onGuest() },
            modifier = Modifier.fillMaxWidth()
        ) { Text("Play as guest") }
        vm.authError?.let { error ->
            Spacer(Modifier.height(8.dp))
            Text(
                text = error,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error
            )
        }
    }

    LaunchedEffect(vm.authToken) {
        if (vm.authToken != null) {
            onLoginSuccess()
        }
    }
}

@Composable
fun HomeScreen(
    vm: GameViewModel,
    onStartGame: () -> Unit,
    onGoToLogin: () -> Unit
) {
    var isCpu by remember { mutableStateOf(false) }
    var isOnline by remember { mutableStateOf(false) }
    var difficulty by remember { mutableStateOf(CpuDifficulty.MEDIUM) }
    var roomCode by remember { mutableStateOf("") }
    var timeControl by remember { mutableStateOf(OnlineTimeControl.default) }
    var authDisplayName by remember { mutableStateOf("") }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp)
    ) {
        Text(
            "PlainTextChess",
            style = MaterialTheme.typography.headlineLarge
        )
        if (vm.authToken != null) {
            Text("Signed in", style = MaterialTheme.typography.titleMedium)
            vm.userName?.let {
                Text("Display name: $it", style = MaterialTheme.typography.bodyMedium)
            }
            OutlinedTextField(
                value = authDisplayName,
                onValueChange = { authDisplayName = it },
                label = { Text("Display name") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth()
            )
            Button(
                onClick = { vm.doSetProfile(authDisplayName) },
                enabled = authDisplayName.isNotBlank()
            ) { Text("Update profile") }
            OutlinedButton(onClick = { vm.doLogout(); onGoToLogin() }) { Text("Logout") }
        } else {
            Text("Playing as guest", style = MaterialTheme.typography.titleMedium)
            OutlinedButton(onClick = { onGoToLogin() }) { Text("Login / Register") }
        }
        if (vm.authError != null) {
            Text(
                text = vm.authError ?: "",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error
            )
        }
        Text("Mode selection", style = MaterialTheme.typography.titleMedium)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(
                selected = !isCpu && !isOnline,
                onClick = { isCpu = false; isOnline = false },
                label = { Text("Two players") }
            )
            FilterChip(
                selected = isCpu,
                onClick = { isCpu = true; isOnline = false },
                label = { Text("CPU") }
            )
            FilterChip(
                selected = isOnline,
                onClick = { isOnline = true; isCpu = false },
                label = { Text("Online") }
            )
        }
        if (isCpu) {
            Text("Difficulty", style = MaterialTheme.typography.labelMedium)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                CpuDifficulty.values().forEach { level ->
                    FilterChip(
                        selected = difficulty == level,
                        onClick = { difficulty = level },
                        label = { Text(level.displayName) }
                    )
                }
            }
        }
        if (isOnline) {
            Text("Time control", style = MaterialTheme.typography.labelMedium)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OnlineTimeControl.presets.forEach { preset ->
                    FilterChip(
                        selected = timeControl == preset,
                        onClick = { timeControl = preset },
                        label = { Text(preset.label) }
                    )
                }
            }
            OutlinedTextField(
                value = roomCode,
                onValueChange = { roomCode = it.uppercase() },
                label = { Text("Room code to join (optional)") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth()
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = {
                    vm.startOnlineGame(true, null, timeControl.label)
                    onStartGame()
                }) { Text("Create room") }
                Button(
                    onClick = {
                        if (roomCode.isNotEmpty()) {
                            vm.startOnlineGame(false, roomCode, null)
                            onStartGame()
                        }
                    },
                    enabled = roomCode.isNotEmpty()
                ) { Text("Join room") }
            }
            vm.onlineJoinError?.let { error ->
                Text(error, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
            }
        }
        if (!isOnline) {
            Button(
                onClick = {
                    if (isCpu) vm.startGame(GameMode.Cpu(difficulty))
                    else vm.startGame(GameMode.TwoPlayers)
                    onStartGame()
                },
                modifier = Modifier.fillMaxWidth()
            ) {
                Text("Start game")
            }
        }
    }
}

@Composable
fun GameDestination(vm: GameViewModel) {
    GameScreen(vm = vm)
}
