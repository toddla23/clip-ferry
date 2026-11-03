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
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
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

    // 처음 열 때 알림 권한(안드로이드 13+)을 한 번 묻고, 서비스를 켜 두었으면 시작한다.
    val notificationPermission = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { /* 거부해도 서비스는 돈다. 알림만 안 보인다. */
        if (CoreController.state.value.serviceEnabled) ClipFerryService.start(context)
    }
    LaunchedEffect(Unit) {
        val needsAsk = Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        if (needsAsk) {
            notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        } else if (state.serviceEnabled) {
            ClipFerryService.start(context)
        }
    }

    var batteryExempt by remember { mutableStateOf(isBatteryExempt(context)) }
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) { batteryExempt = isBatteryExempt(context) }

    Column(
        modifier = Modifier
            .safeDrawingPadding()
            .verticalScroll(rememberScrollState())
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Text("clip-ferry", style = MaterialTheme.typography.headlineMedium)

        StatusCard(state)

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

        RelayCard(state.relayUrl)

        BatteryCard(batteryExempt) {
            context.startActivity(
                Intent(android.provider.Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
            )
        }
    }
}

@Composable
private fun StatusCard(state: UiState) {
    val removed = state.status?.health == Health.REMOVED
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

@Composable
private fun RelayCard(saved: String) {
    var text by remember(saved) { mutableStateOf(saved) }
    var message by remember { mutableStateOf<String?>(null) }
    val context = LocalContext.current
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
                    if (message == null) {
                        // 주소가 같아도 다시 연결한다. 중지된 상태였으면 켠다.
                        CoreController.setServiceEnabled(true)
                        ClipFerryService.start(context)
                    }
                },
            ) { Text("저장하고 다시 연결") }
            Text(
                "에뮬레이터에서 PC의 릴레이는 ws://10.0.2.2:8787 입니다.",
                style = MaterialTheme.typography.bodySmall,
            )
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
