package app.clipferry.android

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.runtime.rememberCoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.clip_ferry_ffi.DeviceEntry

// ------------------------------------------------------------ 첫 실행

/**
 * 그룹이 아직 없을 때: 이름을 정하고, 같은 계정의 다른 기기로 연결(F37)한다. 이 계정에 아직 그룹이
 * 없으면 새 그룹을 만든다(계정 하나당 그룹 하나, F44). 승인해 줄 기기가 없으면 처음부터 다시 시작한다.
 */
@Composable
fun SetupScreen(
    state: UiState,
    name: String,
    onNameChange: (String) -> Unit,
) {
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    // 처음부터 다시 시작: 0 = 닫힘, 1 = 설명, 2 = 마지막 확인
    var restart by rememberSaveable { mutableStateOf(0) }
    val scope = rememberCoroutineScope()
    val initialized = state.status != null
    val has = state.accountHasGroup

    // 계정 서버에 아직 못 물었으면 화면이 보일 때 다시 묻는다.
    LaunchedEffect(has) { if (has == null) CoreController.refreshAccountGroup() }

    fun work(block: () -> String?) {
        busy = true
        error = null
        scope.launch {
            error = withContext(Dispatchers.IO) { block() }
            busy = false
        }
    }

    Text("처음 설정", style = MaterialTheme.typography.headlineSmall)
    state.setupNotice?.let {
        Card(modifier = Modifier.fillMaxWidth()) {
            Text(it, modifier = Modifier.padding(16.dp), color = MaterialTheme.colorScheme.error)
        }
    }
    Text(
        if (has == false) "이 계정의 첫 기기입니다. 이름을 정하고 새 그룹을 만드세요."
        else "같은 계정으로 쓰는 기기(PC 등)로 연결하세요. 그 기기에서 clip-ferry가 켜져 있어야 합니다.",
    )
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("이 기기 이름", style = MaterialTheme.typography.titleMedium)
            if (initialized) {
                Text(state.status.deviceName)
            } else {
                OutlinedTextField(
                    value = name,
                    onValueChange = { onNameChange(it.take(64)) },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                    supportingText = { Text("다른 기기의 목록에 이 이름으로 보입니다") },
                )
            }
        }
    }
    when {
        has == null -> Text("계정 정보를 확인하는 중입니다…")
        has == false -> Button(
            onClick = { work { CoreController.createGroup(name) } },
            enabled = !busy,
            modifier = Modifier.fillMaxWidth(),
        ) { Text("새 그룹 만들기") }
        restart == 0 -> {
            Button(
                onClick = { work { CoreController.startLinkJoin(name) } },
                enabled = !busy,
                modifier = Modifier.fillMaxWidth(),
            ) { Text("이 계정의 다른 기기로 연결") }
            TextButton(onClick = { restart = 1 }, modifier = Modifier.fillMaxWidth()) {
                Text("다른 기기를 쓸 수 없나요? (처음부터 다시 시작)")
            }
        }
        restart == 1 -> Card(modifier = Modifier.fillMaxWidth()) {
            Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("처음부터 다시 시작", style = MaterialTheme.typography.titleMedium)
                Text(
                    "이 계정의 다른 기기를 모두 잃었거나 쓸 수 없을 때만 하세요. 이 기기에서 새 그룹을 " +
                        "만들고, 이 계정의 다른 기기들은 다음에 켜질 때 예전 그룹에서 나옵니다. " +
                        "그 기기들은 다시 연결해야 합니다.",
                )
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
                    OutlinedButton(onClick = { restart = 0 }, modifier = Modifier.weight(1f)) { Text("취소") }
                    Button(onClick = { restart = 2 }, modifier = Modifier.weight(1f)) { Text("계속") }
                }
            }
        }
        else -> Card(modifier = Modifier.fillMaxWidth()) {
            Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("정말 처음부터 다시 시작할까요?", style = MaterialTheme.typography.titleMedium)
                Text(
                    "되돌릴 수 없습니다. 다른 기기들은 예전 그룹에서 나오게 됩니다.",
                    color = MaterialTheme.colorScheme.error,
                )
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
                    OutlinedButton(
                        onClick = { restart = 0 },
                        enabled = !busy,
                        modifier = Modifier.weight(1f),
                    ) { Text("취소") }
                    Button(
                        onClick = { work { CoreController.restartGroup(name) } },
                        enabled = !busy,
                        colors = ButtonDefaults.buttonColors(
                            containerColor = MaterialTheme.colorScheme.error,
                        ),
                        modifier = Modifier.weight(1f),
                    ) { Text("다시 시작") }
                }
            }
        }
    }
    error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
}

/** 저장된 정보가 일부만 남아 열 수 없을 때 (F44): 이 기기의 그룹 정보를 지우고 처음부터 시작한다. */
@Composable
fun BrokenStateScreen(reason: String) {
    var error by remember { mutableStateOf<String?>(null) }
    var confirm by remember { mutableStateOf(false) }
    Text("저장된 정보를 열 수 없습니다", style = MaterialTheme.typography.headlineSmall)
    Text(
        "이 기기에 저장한 정보의 일부가 없거나 손상되었습니다. 이 기기의 그룹 정보를 지우고 처음부터 " +
            "시작하면 다시 쓸 수 있습니다. 시작 화면에서 '이 계정의 다른 기기로 연결'을 누르면 다시 들어옵니다.",
    )
    Text(reason, style = MaterialTheme.typography.bodySmall)
    if (!confirm) {
        Button(onClick = { confirm = true }, modifier = Modifier.fillMaxWidth()) {
            Text("처음부터 시작")
        }
    } else {
        Button(
            onClick = { error = CoreController.resetBrokenState() },
            colors = ButtonDefaults.buttonColors(containerColor = MaterialTheme.colorScheme.error),
            modifier = Modifier.fillMaxWidth(),
        ) { Text("이 기기의 그룹 정보 지우기") }
        OutlinedButton(onClick = { confirm = false }, modifier = Modifier.fillMaxWidth()) {
            Text("취소")
        }
    }
    error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
}

// ------------------------------------------------------------ 진행 중인 페어링

/** 계정 연결 진행 화면: 받는 쪽(요청 승인)과 들어가는 쪽(확인 숫자 → 승인 대기). */
@Composable
fun PairingScreen(pairing: PairUi) {
    BackHandler { CoreController.closePairing() }
    when (pairing.kind) {
        PairKind.INVITE -> InviteContent(pairing)
        PairKind.JOIN -> JoinContent(pairing)
    }
}

@Composable
private fun InviteContent(p: PairUi) {
    Text("기기 추가", style = MaterialTheme.typography.headlineSmall)
    when (p.phase) {
        PairPhase.WAITING, PairPhase.CONNECTING -> {
            Text("'${p.peerName}' 기기에 연결하는 중입니다.", style = MaterialTheme.typography.titleMedium)
            p.message?.let { Text(it) }
            OutlinedButton(onClick = CoreController::closePairing, modifier = Modifier.fillMaxWidth()) {
                Text("취소")
            }
        }
        PairPhase.REQUEST -> {
            Text("'${p.peerName}' 기기가 참여하려고 합니다.", style = MaterialTheme.typography.titleMedium)
            Sas(p.sas)
            Text("상대 기기 화면에도 같은 숫자가 보이면 승인하세요. 다르면 거절하세요.")
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
                OutlinedButton(
                    onClick = { CoreController.decideInvite(false) },
                    modifier = Modifier.weight(1f),
                ) { Text("거절") }
                Button(
                    onClick = { CoreController.decideInvite(true) },
                    modifier = Modifier.weight(1f),
                ) { Text("승인") }
            }
        }
        PairPhase.DONE -> {
            Text("'${p.peerName}' 기기를 추가했습니다.", style = MaterialTheme.typography.titleMedium)
            p.message?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            Button(onClick = CoreController::closePairing, modifier = Modifier.fillMaxWidth()) {
                Text("닫기")
            }
        }
        PairPhase.FAILED -> Failed(p.message)
    }
}

@Composable
private fun JoinContent(p: PairUi) {
    Text("그룹에 참여", style = MaterialTheme.typography.headlineSmall)
    when (p.phase) {
        PairPhase.CONNECTING -> {
            Text(p.message ?: "릴레이에 연결하는 중입니다")
            OutlinedButton(onClick = CoreController::closePairing, modifier = Modifier.fillMaxWidth()) {
                Text("취소")
            }
        }
        PairPhase.WAITING, PairPhase.REQUEST -> {
            Text("'${p.peerName}'의 그룹에 참여합니다.", style = MaterialTheme.typography.titleMedium)
            Sas(p.sas)
            Text("'${p.peerName}' 화면에 같은 숫자가 보이는지 확인하고, 그 기기에서 승인하세요.")
            Text("승인을 기다리는 중…", style = MaterialTheme.typography.bodySmall)
            OutlinedButton(onClick = CoreController::closePairing, modifier = Modifier.fillMaxWidth()) {
                Text("취소")
            }
        }
        // 참여가 끝나면 컨트롤러가 이 화면을 바로 닫는다.
        PairPhase.DONE -> Text("참여했습니다.")
        PairPhase.FAILED -> Failed(p.message)
    }
}

@Composable
private fun Failed(message: String?) {
    Text("연결하지 못했습니다", style = MaterialTheme.typography.titleMedium)
    message?.let { Text(it, color = MaterialTheme.colorScheme.error) }
    OutlinedButton(onClick = CoreController::closePairing, modifier = Modifier.fillMaxWidth()) {
        Text("닫기")
    }
}

/** 6자리 확인 숫자를 3자리씩 크게 보여 준다. */
@Composable
private fun Sas(sas: String?) {
    val text = sas?.let { if (it.length == 6) "${it.take(3)} ${it.drop(3)}" else it } ?: "…"
    Text(
        text,
        modifier = Modifier.fillMaxWidth(),
        textAlign = TextAlign.Center,
        fontFamily = FontFamily.Monospace,
        fontSize = 44.sp,
    )
}

// ------------------------------------------------------------ 기기 목록 (읽기 전용)

/** 그룹 기기 목록. 제거·이름 변경은 데스크톱에서 한다. */
@Composable
fun DevicesCard(devices: List<DeviceEntry>) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("기기 ${devices.size}대", style = MaterialTheme.typography.titleMedium)
            devices.sortedByDescending { it.isSelf }.forEach { d ->
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box(
                        modifier = Modifier
                            .size(10.dp)
                            .background(
                                if (d.online) Color(0xFF2E7D32) else Color(0xFF9E9E9E),
                                CircleShape,
                            ),
                    )
                    Column(modifier = Modifier.padding(start = 12.dp)) {
                        Text(d.name + if (d.isSelf) " (이 기기)" else "")
                        Text(
                            "${d.idShort} · ${if (d.online) "연결됨" else "연결 안 됨"}",
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
            }
            Text(
                "기기를 더하려면 새 기기에서 같은 계정으로 로그인하고 '이 계정의 다른 기기로 연결'을 " +
                    "누르세요. 기기 제거와 이름 변경은 데스크톱 앱에서 합니다.",
                style = MaterialTheme.typography.bodySmall,
            )
        }
    }
}
