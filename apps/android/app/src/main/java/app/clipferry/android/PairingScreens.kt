package app.clipferry.android

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import kotlinx.coroutines.delay
import uniffi.clip_ferry_ffi.DeviceEntry
import uniffi.clip_ferry_ffi.QrMatrix

// ------------------------------------------------------------ 첫 실행

/** 그룹이 아직 없을 때: 이름을 정하고 참여(QR·코드)하거나 새 그룹을 만든다. */
@Composable
fun SetupScreen(
    state: UiState,
    name: String,
    onNameChange: (String) -> Unit,
    onScan: () -> Unit,
    onEnterCode: () -> Unit,
) {
    var error by remember { mutableStateOf<String?>(null) }
    val initialized = state.status != null
    Text("처음 설정", style = MaterialTheme.typography.headlineSmall)
    Text(
        "데스크톱(또는 다른 폰)의 clip-ferry에서 '기기 추가'를 누른 뒤, " +
            "QR을 스캔하거나 표시된 코드를 입력하세요.",
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
    Button(onClick = onScan, modifier = Modifier.fillMaxWidth()) { Text("QR로 참여") }
    OutlinedButton(onClick = onEnterCode, modifier = Modifier.fillMaxWidth()) {
        Text("코드 입력으로 참여")
    }
    TextButton(
        onClick = { error = CoreController.createGroup(name) },
        modifier = Modifier.fillMaxWidth(),
    ) { Text("새 그룹 만들기 (이 기기가 처음일 때)") }
    error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
}

// ------------------------------------------------------------ 참여 (QR · 코드)

enum class JoinMode { SCAN, CODE }

/**
 * 참여 화면. [JoinMode.SCAN]이면 카메라 권한을 받아 QR을 읽고, 권한이 없으면 코드 입력으로 바꾼다.
 */
@Composable
fun JoinScreen(
    name: String,
    relayUrl: String,
    initialMode: JoinMode,
    onBack: () -> Unit,
) {
    val context = LocalContext.current
    BackHandler(onBack = onBack)
    var mode by rememberSaveable { mutableStateOf(initialMode) }
    var notice by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var granted by remember {
        mutableStateOf(
            ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) ==
                PackageManager.PERMISSION_GRANTED
        )
    }
    val cameraPermission = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { ok ->
        granted = ok
        if (!ok) {
            mode = JoinMode.CODE
            notice = "카메라 권한이 없어 QR을 스캔할 수 없습니다. " +
                "초대한 기기에 표시된 코드를 입력하세요. (설정 → 앱 → clip-ferry → 권한에서 카메라를 켤 수 있습니다)"
        }
    }
    LaunchedEffect(mode) {
        if (mode == JoinMode.SCAN && !granted) cameraPermission.launch(Manifest.permission.CAMERA)
    }

    val join = { code: String -> error = CoreController.startJoin(name, code) }

    Text(
        if (mode == JoinMode.SCAN) "QR로 참여" else "코드 입력으로 참여",
        style = MaterialTheme.typography.headlineSmall,
    )
    notice?.let {
        Card(modifier = Modifier.fillMaxWidth()) {
            Text(it, modifier = Modifier.padding(16.dp))
        }
    }

    if (mode == JoinMode.SCAN && granted) {
        Text("초대한 기기 화면의 QR을 네모 안에 비추세요.")
        var hint by remember { mutableStateOf<String?>(null) }
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .aspectRatio(1f)
                .background(Color.Black),
        ) {
            QrScanner(
                modifier = Modifier.fillMaxWidth().aspectRatio(1f),
                onInvite = { join(it) },
                onOtherQr = { hint = "clip-ferry 초대 QR이 아닙니다" },
                onCameraError = {
                    mode = JoinMode.CODE
                    notice = it
                },
            )
        }
        hint?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
        OutlinedButton(onClick = { mode = JoinMode.CODE }, modifier = Modifier.fillMaxWidth()) {
            Text("대신 코드 입력하기")
        }
    } else if (mode == JoinMode.CODE) {
        var code by rememberSaveable { mutableStateOf("") }
        Text("초대한 기기에 표시된 코드(4자씩 묶인 53자)를 입력하세요. 하이픈과 대소문자는 무시합니다.")
        OutlinedTextField(
            value = code,
            onValueChange = { code = it; error = null },
            modifier = Modifier.fillMaxWidth(),
            textStyle = MaterialTheme.typography.bodyLarge.copy(fontFamily = FontFamily.Monospace),
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Characters),
            minLines = 2,
        )
        Button(
            onClick = { join(code) },
            enabled = code.isNotBlank(),
            modifier = Modifier.fillMaxWidth(),
        ) { Text("참여") }
        Text(
            "코드로 참여하면 이 릴레이를 씁니다: $relayUrl",
            style = MaterialTheme.typography.bodySmall,
        )
        OutlinedButton(
            onClick = {
                notice = null
                mode = JoinMode.SCAN
            },
            modifier = Modifier.fillMaxWidth(),
        ) { Text("QR 스캔으로 바꾸기") }
    }
    error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
    TextButton(onClick = onBack) { Text("뒤로") }
}

// ------------------------------------------------------------ 진행 중인 페어링

/** 초대(QR·코드 표시 → 요청 승인)와 참여(확인 숫자 → 승인 대기) 진행 화면. */
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
            Text("새 기기의 clip-ferry에서 'QR로 참여'로 스캔하거나 아래 코드를 입력하세요.")
            p.qr?.let { qr ->
                Box(modifier = Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                    QrImage(qr, modifier = Modifier.fillMaxWidth().widthIn(max = 360.dp))
                }
            }
            p.textCode?.let { code ->
                Card(modifier = Modifier.fillMaxWidth()) {
                    Text(
                        code,
                        modifier = Modifier.padding(16.dp),
                        fontFamily = FontFamily.Monospace,
                        style = MaterialTheme.typography.bodyLarge,
                    )
                }
            }
            p.expiresAtMs?.let { Countdown(it) }
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
        PairPhase.FAILED -> Failed(p.message, onRetry = {
            CoreController.closePairing()
            CoreController.startInvite()
        })
    }
}

@Composable
private fun JoinContent(p: PairUi) {
    Text("그룹에 참여", style = MaterialTheme.typography.headlineSmall)
    when (p.phase) {
        PairPhase.CONNECTING -> {
            Text(p.message ?: "릴레이에 연결하는 중입니다")
            Text(
                "초대한 기기의 '기기 추가' 화면이 열려 있어야 합니다.",
                style = MaterialTheme.typography.bodySmall,
            )
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
        PairPhase.FAILED -> Failed(p.message, onRetry = null)
    }
}

@Composable
private fun Failed(message: String?, onRetry: (() -> Unit)?) {
    Text("페어링하지 못했습니다", style = MaterialTheme.typography.titleMedium)
    message?.let { Text(it, color = MaterialTheme.colorScheme.error) }
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
        OutlinedButton(onClick = CoreController::closePairing, modifier = Modifier.weight(1f)) {
            Text("닫기")
        }
        if (onRetry != null) {
            Button(onClick = onRetry, modifier = Modifier.weight(1f)) { Text("새 초대") }
        }
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

@Composable
private fun Countdown(expiresAtMs: Long) {
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(expiresAtMs) {
        while (now < expiresAtMs) {
            delay(1_000)
            now = System.currentTimeMillis()
        }
    }
    val left = ((expiresAtMs - now) / 1000).coerceAtLeast(0)
    Text(
        "남은 시간 ${left / 60}:${"%02d".format(left % 60)}",
        style = MaterialTheme.typography.bodySmall,
    )
}

/** QR 칸을 흰 바탕(여백 4칸)에 그린다. 어두운 테마에서도 스캔되게 바탕은 항상 흰색이다. */
@Composable
fun QrImage(qr: QrMatrix, modifier: Modifier = Modifier) {
    val width = qr.width.toInt()
    val quiet = 4
    Canvas(modifier = modifier.aspectRatio(1f).background(Color.White)) {
        val cell = size.width / (width + quiet * 2)
        for (i in 0 until width * width) {
            if (qr.dark[i]) {
                drawRect(
                    color = Color.Black,
                    topLeft = Offset((i % width + quiet) * cell, (i / width + quiet) * cell),
                    // 칸 사이 틈이 생기지 않게 조금 겹친다.
                    size = Size(cell + 0.5f, cell + 0.5f),
                )
            }
        }
    }
}

// ------------------------------------------------------------ 기기 목록 (읽기 전용)

/** 그룹 기기 목록. 제거·이름 변경은 데스크톱에서 한다. */
@Composable
fun DevicesCard(devices: List<DeviceEntry>, canInvite: Boolean, onInvite: () -> Unit) {
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
                "기기 제거와 이름 변경은 데스크톱 앱에서 합니다.",
                style = MaterialTheme.typography.bodySmall,
            )
            Button(onClick = onInvite, enabled = canInvite) { Text("기기 추가") }
        }
    }
}
