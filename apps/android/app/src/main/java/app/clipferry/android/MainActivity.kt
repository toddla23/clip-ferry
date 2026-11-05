package app.clipferry.android

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.os.PowerManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import uniffi.clip_ferry_ffi.Health

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    MainScreen()
                }
            }
        }
    }
}

@Composable
private fun MainScreen() {
    val context = LocalContext.current
    val state by CoreController.state.collectAsStateWithLifecycle()

    // 처음 열 때 알림 권한(안드로이드 13+)을 한 번 묻고, 그룹이 있고 서비스를 켜 두었으면 시작한다.
    val notificationPermission = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { /* 거부해도 서비스는 돈다. 알림만 안 보인다. */
        val s = CoreController.state.value
        if (s.serviceEnabled && s.hasGroup) ClipFerryService.start(context)
    }
    LaunchedEffect(Unit) {
        val needsAsk = Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        if (needsAsk) {
            notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        } else if (state.serviceEnabled && state.hasGroup) {
            ClipFerryService.start(context)
        }
    }

    var batteryExempt by remember { mutableStateOf(isBatteryExempt(context)) }
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) { batteryExempt = isBatteryExempt(context) }

    // 첫 실행 화면의 이름과, 참여 화면을 열었는지(어떤 방식으로).
    var name by rememberSaveable { mutableStateOf(CoreController.defaultDeviceName) }
    var joinMode by rememberSaveable { mutableStateOf<JoinMode?>(null) }
    // 그룹에 들어가면 참여 화면은 닫는다.
    LaunchedEffect(state.hasGroup) { if (state.hasGroup) joinMode = null }

    Column(
        modifier = Modifier
            .safeDrawingPadding()
            .verticalScroll(rememberScrollState())
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Text("clip-ferry", style = MaterialTheme.typography.headlineMedium)

        val pairing = state.pairing
        val mode = joinMode
        when {
            pairing != null -> PairingScreen(pairing)
            !state.hasGroup && mode != null ->
                JoinScreen(name, state.relayUrl, mode, onBack = { joinMode = null })
            !state.hasGroup -> {
                SetupScreen(
                    state,
                    name,
                    onNameChange = { name = it },
                    onScan = { joinMode = JoinMode.SCAN },
                    onEnterCode = { joinMode = JoinMode.CODE },
                )
                state.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                RelayCard(
                    state.relayUrl,
                    buttonLabel = "저장",
                    note = "코드로 참여하거나 새 그룹을 만들 때 쓰는 주소입니다. " +
                        "QR로 참여하면 QR에 들어 있는 주소로 바뀝니다.",
                ) {}
            }
            else -> ReadyScreen(state, batteryExempt)
        }
    }
}

@Composable
private fun ReadyScreen(state: UiState, batteryExempt: Boolean) {
    val context = LocalContext.current
    var inviteError by remember { mutableStateOf<String?>(null) }

    StatusCard(state)

    ReceiveCard(state)

    SendCard(state)

    DevicesCard(
        devices = state.devices,
        canInvite = !state.removed,
        onInvite = { inviteError = CoreController.startInvite() },
    )
    inviteError?.let { Text(it, color = MaterialTheme.colorScheme.error) }

    GroupCard(state)

    Card(modifier = Modifier.fillMaxWidth()) {
        Row(
            modifier = Modifier.padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Column(modifier = Modifier.weight(1f)) {
                Text("백그라운드 연결 유지", style = MaterialTheme.typography.titleMedium)
                Text(
                    "켜 두면 앱을 닫아도 알림이 남고 연결이 유지됩니다",
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            Switch(
                checked = state.serviceEnabled,
                onCheckedChange = { on ->
                    CoreController.setServiceEnabled(on)
                    if (on) ClipFerryService.start(context) else ClipFerryService.stop(context)
                },
            )
        }
    }

    RelayCard(
        state.relayUrl,
        buttonLabel = "저장하고 다시 연결",
        note = "그룹의 모든 기기가 같은 릴레이를 써야 합니다.",
    ) {
        // 주소가 같아도 다시 연결한다. 중지된 상태였으면 켠다.
        CoreController.setServiceEnabled(true)
        ClipFerryService.start(context)
    }

    BatteryCard(batteryExempt) {
        context.startActivity(
            Intent(android.provider.Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
        )
    }
}

/** 보내기: 타일·알림 버튼 안내, 타일 추가, 지금 보내기, 민감한 내용 설정. */
@Composable
private fun SendCard(state: UiState) {
    val context = LocalContext.current
    var tileMessage by remember { mutableStateOf<String?>(null) }
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("보내기", style = MaterialTheme.typography.titleMedium)
            Text(
                "다른 앱에서 복사한 뒤 빠른 설정의 '클립보드 보내기' 타일이나 " +
                    "상시 알림의 '클립보드 보내기'를 누르면 다른 기기로 갑니다. 텍스트·이미지 모두 됩니다. " +
                    "공유 메뉴의 '다른 기기로 보내기'(이미지 여러 장 가능)나 " +
                    "텍스트를 길게 눌러 나오는 'PC로 보내기'로도 보낼 수 있습니다.",
                style = MaterialTheme.typography.bodySmall,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (Build.VERSION.SDK_INT >= 33) {
                    OutlinedButton(onClick = { requestAddTile(context) { tileMessage = it } }) {
                        Text("타일 추가")
                    }
                }
                OutlinedButton(onClick = {
                    // 앱 화면에 초점이 있으니 바로 읽을 수 있다.
                    val content = SendClipboard.read(context, state.syncSensitive)
                    SendClipboard.send(context, content)
                }) { Text("지금 클립보드 보내기") }
            }
            if (Build.VERSION.SDK_INT < 33) {
                Text(
                    "타일은 빠른 설정 패널의 편집(연필)에서 'clip-ferry 클립보드 보내기'를 끌어다 놓아 추가합니다.",
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            tileMessage?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Column(modifier = Modifier.weight(1f)) {
                    Text("민감한 내용도 보내기")
                    Text(
                        "비밀번호 관리자 등이 민감하다고 표시한 복사. 끄면 보내지 않습니다",
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                Switch(
                    checked = state.syncSensitive,
                    onCheckedChange = { CoreController.setSyncSensitive(it) },
                )
            }
        }
    }
}

/** 빠른 설정 패널에 타일을 넣어 달라고 시스템에 요청한다 (안드로이드 13+). */
private fun requestAddTile(context: android.content.Context, onResult: (String) -> Unit) {
    if (Build.VERSION.SDK_INT < 33) return
    val sbm = context.getSystemService(android.app.StatusBarManager::class.java)
    sbm.requestAddTileService(
        android.content.ComponentName(context, SendTileService::class.java),
        context.getString(R.string.tile_label),
        android.graphics.drawable.Icon.createWithResource(context, R.drawable.ic_stat_ferry),
        ContextCompat.getMainExecutor(context),
    ) { result ->
        onResult(
            when (result) {
                android.app.StatusBarManager.TILE_ADD_REQUEST_RESULT_TILE_ADDED -> "타일을 추가했습니다"
                android.app.StatusBarManager.TILE_ADD_REQUEST_RESULT_TILE_ALREADY_ADDED -> "이미 추가되어 있습니다"
                android.app.StatusBarManager.TILE_ADD_REQUEST_RESULT_TILE_NOT_ADDED -> "추가하지 않았습니다"
                else -> "추가하지 못했습니다. 빠른 설정 편집에서 직접 추가하세요"
            },
        )
    }
}

/** 받은 클립보드: 마지막으로 받은 것과 알림 설정. 내용은 보여 주지 않는다. */
@Composable
private fun ReceiveCard(state: UiState) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("받은 클립보드", style = MaterialTheme.typography.titleMedium)
            Text(
                "다른 기기에서 복사한 텍스트·이미지가 이 기기 클립보드에 바로 들어갑니다. " +
                    "붙여넣기로 쓰세요.",
                style = MaterialTheme.typography.bodySmall,
            )
            val last = state.lastReceived
            if (last == null) {
                Text("아직 받은 것이 없습니다")
            } else {
                val time = java.text.DateFormat.getTimeInstance(java.text.DateFormat.SHORT)
                    .format(java.util.Date(last.atMs))
                val what = if (last.isImage) "이미지" else "텍스트"
                Text(
                    if (last.ok) "$time · '${last.fromName}'에서 받은 $what"
                    else "$time · '${last.fromName}'에서 받은 $what (클립보드에 넣지 못함)",
                    color = if (last.ok) MaterialTheme.colorScheme.onSurface
                    else MaterialTheme.colorScheme.error,
                )
            }
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Text("받으면 알림 띄우기", modifier = Modifier.weight(1f))
                Switch(
                    checked = state.notifyOnReceive,
                    onCheckedChange = { CoreController.setNotifyOnReceive(it) },
                )
            }
        }
    }
}

@Composable
private fun StatusCard(state: UiState) {
    val removed = state.removed
    val stale = state.status?.health == Health.STALE
    val headline = when {
        removed -> "그룹에서 제거됨"
        state.link == Link.CONNECTED -> "연결됨"
        state.link == Link.RECONNECTING -> "재연결 중"
        else -> "연결 안 됨"
    }
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(headline, style = MaterialTheme.typography.headlineSmall)
            state.status?.let { s ->
                Text("이 기기: ${s.deviceName} (${s.deviceIdShort})")
                s.epoch?.let { Text("에포크 $it · 구성원 ${s.memberCount}대") }
            }
            if (state.link == Link.CONNECTED) {
                Text("지금 연결된 기기 ${state.online}대 (이 기기 포함)")
            }
            if (removed) Text("이 기기는 그룹에서 제거되었습니다. 다시 페어링하세요.")
            if (stale) Text("너무 오래 연결되지 않았습니다. 다시 페어링해야 할 수 있습니다.")
            state.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        }
    }
}

/**
 * 그룹에서 나가는 버튼. 제거되었거나 오래 연결되지 않았을 때(다시 페어링),
 * 또는 혼자인 그룹일 때(다른 그룹에 참여)만 보인다.
 */
@Composable
private fun GroupCard(state: UiState) {
    val status = state.status ?: return
    val stale = status.health == Health.STALE
    val alone = status.memberCount <= 1u
    if (!state.removed && !stale && !alone) return
    var confirm by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val (title, body, label) = if (state.removed || stale) {
        Triple(
            "다시 페어링",
            "이 그룹에서 나간 뒤 처음 화면에서 다시 참여합니다.",
            "그룹 나가고 다시 페어링",
        )
    } else {
        Triple(
            "다른 그룹에 참여",
            "지금은 이 기기 혼자인 그룹입니다. 데스크톱에 이미 그룹이 있으면 " +
                "이 그룹에서 나가 그쪽 그룹에 참여하세요.",
            "그룹 나가고 참여하기",
        )
    }
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(title, style = MaterialTheme.typography.titleMedium)
            Text(body)
            OutlinedButton(onClick = { confirm = true }) { Text(label) }
            error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        }
    }
    if (confirm) {
        AlertDialog(
            onDismissRequest = { confirm = false },
            title = { Text("그룹에서 나갈까요?") },
            text = { Text("이 기기의 그룹 키를 지웁니다. 다시 쓰려면 페어링해야 합니다.") },
            confirmButton = {
                TextButton(onClick = {
                    confirm = false
                    error = CoreController.leaveGroup()
                }) { Text("나가기") }
            },
            dismissButton = { TextButton(onClick = { confirm = false }) { Text("취소") } },
        )
    }
}

@Composable
private fun RelayCard(saved: String, buttonLabel: String, note: String, onSaved: () -> Unit) {
    var text by remember(saved) { mutableStateOf(saved) }
    var message by remember { mutableStateOf<String?>(null) }
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("릴레이 주소", style = MaterialTheme.typography.titleMedium)
            OutlinedTextField(
                value = text,
                onValueChange = { text = it; message = null },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
                supportingText = { message?.let { Text(it) } },
                isError = message != null,
            )
            Button(
                onClick = {
                    message = CoreController.setRelayUrl(text)
                    if (message == null) onSaved()
                },
            ) { Text(buttonLabel) }
            Text(note, style = MaterialTheme.typography.bodySmall)
        }
    }
}

@Composable
private fun BatteryCard(exempt: Boolean, onOpenSettings: () -> Unit) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("배터리 최적화", style = MaterialTheme.typography.titleMedium)
            if (exempt) {
                Text("제외되어 있습니다. 백그라운드 연결이 끊기지 않습니다.")
            } else {
                Text(
                    "절전 기능이 백그라운드 연결을 끊을 수 있습니다. " +
                        "목록에서 clip-ferry를 찾아 '최적화 안 함'으로 바꿔 주세요.",
                )
            }
            Spacer(Modifier.height(4.dp))
            OutlinedButton(onClick = onOpenSettings) { Text("배터리 최적화 설정 열기") }
        }
    }
}

private fun isBatteryExempt(context: android.content.Context): Boolean =
    context.getSystemService(PowerManager::class.java)
        .isIgnoringBatteryOptimizations(context.packageName)
