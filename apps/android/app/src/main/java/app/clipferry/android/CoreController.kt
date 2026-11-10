package app.clipferry.android

import android.app.Activity
import android.content.Context
import android.os.Build
import android.os.Handler
import android.os.Looper
import java.io.File
import kotlin.concurrent.thread
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.withContext
import uniffi.clip_ferry_ffi.AccountInfo
import uniffi.clip_ferry_ffi.ClipFerryClient
import uniffi.clip_ferry_ffi.CoreEvents
import uniffi.clip_ferry_ffi.DeviceEntry
import uniffi.clip_ferry_ffi.FfiException
import uniffi.clip_ferry_ffi.Health
import uniffi.clip_ferry_ffi.LinkStatus
import uniffi.clip_ferry_ffi.PairEvent
import uniffi.clip_ferry_ffi.PairListener
import uniffi.clip_ferry_ffi.QrMatrix
import uniffi.clip_ferry_ffi.StatusInfo
import uniffi.clip_ferry_ffi.qrMatrix

enum class Link { STOPPED, RECONNECTING, CONNECTED }

enum class PairKind { INVITE, JOIN }

enum class PairPhase {
    /** 참여: 릴레이에 연결하는 중 */
    CONNECTING,
    /** 초대: 상대를 기다리는 중 / 참여: 초대한 기기의 승인을 기다리는 중 */
    WAITING,
    /** 초대: 참여 요청이 와서 확인 숫자를 보여 주고 승인을 받는 중 */
    REQUEST,
    DONE,
    FAILED,
}

/** 진행 중인 페어링 화면에 필요한 값 (데스크톱 `PairingView`와 같은 모양). */
data class PairUi(
    val kind: PairKind,
    val phase: PairPhase,
    val textCode: String? = null,
    val qr: QrMatrix? = null,
    val expiresAtMs: Long? = null,
    val peerName: String? = null,
    /** 6자리 확인 숫자 */
    val sas: String? = null,
    val message: String? = null,
)

/** 마지막으로 받은 것의 요약. 내용은 담지 않는다. */
data class Received(val fromName: String, val isImage: Boolean, val ok: Boolean, val atMs: Long)

/** 화면과 알림이 읽는 상태. 비밀값은 없다. */
data class UiState(
    val link: Link = Link.STOPPED,
    /** 그룹 채널에 붙어 있는 기기 수 (이 기기 포함). */
    val online: Int = 0,
    /** 기기가 아직 없으면 `null`. */
    val status: StatusInfo? = null,
    val devices: List<DeviceEntry> = emptyList(),
    val pairing: PairUi? = null,
    val error: String? = null,
    val relayUrl: String = "",
    /** 백그라운드 연결 유지(포그라운드 서비스)를 켜 두었는가. */
    val serviceEnabled: Boolean = true,
    /** 받은 내용을 클립보드에 넣을 때 알림을 띄우는가. */
    val notifyOnReceive: Boolean = true,
    /** 민감한 내용으로 표시된 복사도 보내는가. */
    val syncSensitive: Boolean = true,
    /** 마지막으로 받은 것 (이 앱이 켜진 뒤). 내용은 담지 않는다. */
    val lastReceived: Received? = null,
    /** 로그인한 계정 (F35). 없으면 로그인 화면만 보이고 연결하지 않는다. */
    val account: AccountInfo? = null,
    /** 로그인할 계정 서버 주소 (로그아웃 상태에서만 바꾼다). */
    val accountUrl: String = "",
    /** 구글 로그인 창이 떠 있거나 계정 서버를 기다리는 중. */
    val loginBusy: Boolean = false,
    /** 로그인 화면에 보여 줄 문장 (실패·만료). */
    val loginMessage: String? = null,
    /** 릴레이가 접속 토큰을 받지 않았다. 다시 연결되면 사라진다. */
    val authRejected: Boolean = false,
    /** 같은 계정의 새 기기가 보낸 참여 요청(기기 이름, F37). 연결·무시를 고르면 사라진다. */
    val linkRequest: String? = null,
    /** 서버 주소 칸을 보인다 (디버그 빌드이거나 제목을 7번 눌렀을 때, F43). */
    val showServerSettings: Boolean = false,
) {
    /** 그룹이 있다(제거된 그룹 포함). 없으면 첫 실행 화면을 보여 준다. */
    val hasGroup: Boolean get() = status?.epoch != null
    val removed: Boolean get() = status?.health == Health.REMOVED
    val signedIn: Boolean get() = account != null
}

/**
 * 코어(Rust)와 화면 사이의 얇은 층. 암호·프로토콜 로직은 없다.
 * 코어를 열고, 릴레이 세션·페어링을 시작·중지하고, 코어가 알려 주는 상태를 [UiState]로 모은다.
 *
 * 코어 콜백(코어 스레드)에서는 이 객체의 잠금을 잡지 않는다. 잠금을 쥔 채 코어를 부르는 쪽과
 * 서로 기다리지 않게 하려는 것이다. 상태는 [MutableStateFlow.update]로만 바꾼다.
 */
object CoreController : CoreEvents {
    private lateinit var appContext: Context
    private lateinit var settings: Settings

    @Volatile
    private var client: ClipFerryClient? = null

    /** 참여가 끝나면 이 주소를 릴레이 설정으로 저장한다(QR에 들어 있던 주소). */
    @Volatile
    private var joinRelay: String? = null

    /** 지금 참여가 계정 연결(F37)로 진행 중인가 (안내 문구가 다르다). */
    @Volatile
    private var linkJoin = false

    private val mutableState = MutableStateFlow(UiState())
    val state: StateFlow<UiState> = mutableState

    fun init(context: Context) {
        appContext = context.applicationContext
        settings = Settings(appContext)
        ReceivedClips.init(appContext, settings)
        mutableState.value = UiState(
            relayUrl = settings.relayUrl,
            serviceEnabled = settings.serviceEnabled,
            notifyOnReceive = settings.notifyOnReceive,
            syncSensitive = settings.syncSensitive,
            accountUrl = settings.accountUrl,
            showServerSettings = BuildConfig.DEBUG || settings.showServer,
        )
        try {
            val c = ensureOpen()
            mutableState.update { it.copy(account = c.account()) }
            refresh()
            refreshServerConfig()
        } catch (e: FfiException) {
            fail(e.userMessage())
        } catch (e: UnsatisfiedLinkError) {
            fail("코어 라이브러리를 불러오지 못했습니다")
        }
    }

    /** 코어를 연다. 기기·그룹은 만들지 않는다(첫 실행 화면에서 정한다). */
    @Synchronized
    private fun ensureOpen(): ClipFerryClient {
        client?.let { return it }
        val dir = File(appContext.filesDir, "clip-ferry").absolutePath
        val opened = ClipFerryClient.open(dir, KeystoreVault(appContext))
        client = opened
        return opened
    }

    /** 첫 실행 화면에 미리 채워 둘 기기 이름. */
    val defaultDeviceName: String
        get() = Build.MODEL.trim().ifEmpty { "Android" }.take(64)

    /** 기기가 아직 없으면 이 이름으로 만든다. 이미 있으면 이름은 그대로 둔다. */
    private fun ensureDevice(c: ClipFerryClient, name: String) {
        if (!c.isInitialized()) c.initDevice(name.trim().ifEmpty { defaultDeviceName })
    }

    /**
     * 릴레이에 (다시) 연결한다. 이미 연결 중이면 닫고 새로 연다.
     * 로그인하지 않았거나 그룹이 없으면 연결하지 않는다.
     */
    @Synchronized
    fun start() {
        try {
            val c = ensureOpen()
            if (!c.isSignedIn() || !c.hasGroup()) {
                // 로그인 전이거나, 첫 실행(그룹 없음)이거나 제거된 기기다. 화면의 안내로 충분하다.
                c.stop()
                mutableState.update { it.copy(link = Link.STOPPED, online = 0, error = null) }
                refresh()
                return
            }
            c.start(settings.relayUrl, this)
            mutableState.update {
                it.copy(link = Link.RECONNECTING, error = null, relayUrl = settings.relayUrl)
            }
            refresh()
        } catch (e: FfiException.Removed) {
            mutableState.update { it.copy(link = Link.STOPPED, error = null, online = 0) }
            refresh()
        } catch (e: FfiException) {
            fail(e.userMessage())
        } catch (e: UnsatisfiedLinkError) {
            fail("코어 라이브러리를 불러오지 못했습니다")
        }
    }

    @Synchronized
    fun stop() {
        client?.stop()
        mutableState.update { it.copy(link = Link.STOPPED, online = 0) }
        refresh()
    }

    /** 네트워크가 바뀌었을 때: 끊긴 연결이 시간 초과를 기다리지 않고 바로 다시 붙게 한다. */
    @Synchronized
    fun restart() {
        if (client?.isRunning() == true) start()
    }

    /** 기기에 네트워크가 없을 때: 코어가 알아채기 전에 화면 표시만 먼저 바꾼다. */
    fun markOffline() {
        mutableState.update {
            if (it.link == Link.CONNECTED) it.copy(link = Link.RECONNECTING, online = 0) else it
        }
    }

    /**
     * 릴레이 주소를 직접 정한다. 비우면 계정 서버가 알려 주는 주소로 돌아간다 (F43).
     * 잘못되었으면 이유를, 괜찮으면 `null`을 돌려준다.
     */
    fun setRelayUrl(url: String): String? {
        val trimmed = url.trim()
        if (trimmed.isEmpty()) {
            settings.relayCustom = false
            refreshServerConfig()
            return null
        }
        if (!(trimmed.startsWith("ws://") || trimmed.startsWith("wss://"))) {
            return "릴레이 주소는 ws:// 또는 wss://로 시작해야 합니다"
        }
        settings.relayUrl = trimmed
        settings.relayCustom = true
        mutableState.update { it.copy(relayUrl = trimmed) }
        return null
    }

    /**
     * 계정 서버가 알려 주는 릴레이 주소를 받아, 사용자가 직접 정하지 않았으면 그 주소로 바꾼다 (F43).
     * 못 받으면 저장해 둔 주소를 그대로 쓴다. 네트워크를 쓰므로 따로 스레드에서 한다.
     */
    fun refreshServerConfig() {
        if (settings.relayCustom) return
        val c = client ?: return
        val accountUrl = settings.accountUrl
        thread(name = "server-config", isDaemon = true) {
            val relay = try {
                c.serverRelayUrl(accountUrl)
            } catch (e: Exception) {
                null
            } ?: return@thread
            synchronized(this) {
                // 그사이 사용자가 직접 정했거나 다른 계정 서버로 바꿨으면 따르지 않는다.
                if (settings.relayCustom || settings.accountUrl != accountUrl || settings.relayUrl == relay) {
                    return@thread
                }
                settings.relayUrl = relay
                mutableState.update { it.copy(relayUrl = relay) }
                if (c.isRunning()) start()
            }
        }
    }

    /** 배포 빌드에서도 서버 주소 칸을 보이게 한다 (제목을 7번 누르면, F43). */
    fun showServerSettings() {
        settings.showServer = true
        mutableState.update { it.copy(showServerSettings = true) }
    }

    fun setServiceEnabled(enabled: Boolean) {
        settings.serviceEnabled = enabled
        mutableState.update { it.copy(serviceEnabled = enabled) }
    }

    fun setNotifyOnReceive(enabled: Boolean) {
        settings.notifyOnReceive = enabled
        mutableState.update { it.copy(notifyOnReceive = enabled) }
    }

    val syncSensitive: Boolean get() = settings.syncSensitive

    fun setSyncSensitive(enabled: Boolean) {
        settings.syncSensitive = enabled
        mutableState.update { it.copy(syncSensitive = enabled) }
    }

    // ------------------------------------------------------- 보내기 (F18)

    /**
     * 그룹이 있는데 연결이 꺼져 있으면 서비스를 켠다. 앱 화면(보내기 액티비티 포함)이
     * 떠 있을 때 불러야 한다(백그라운드에서는 포그라운드 서비스를 시작할 수 없다).
     */
    fun ensureRunning() {
        val s = mutableState.value
        if (!s.signedIn || !s.hasGroup || s.removed) return
        if (client?.isRunning() == true) return
        startService()
    }

    /**
     * 연결될 때까지 최대 [timeoutMs] 기다린다. 연결되면 `null`, 아니면 사용자에게 보여 줄 이유.
     * 작업 스레드에서 부른다.
     */
    fun awaitConnected(timeoutMs: Long): String? {
        val end = System.currentTimeMillis() + timeoutMs
        while (true) {
            val s = mutableState.value
            when {
                !s.signedIn -> return "먼저 clip-ferry 앱에서 로그인하세요"
                !s.hasGroup -> return "먼저 그룹을 만들거나 참여하세요"
                s.removed -> return "이 기기는 그룹에서 제거되었습니다. 다시 페어링하세요"
                s.link == Link.CONNECTED -> return null
            }
            if (System.currentTimeMillis() >= end) {
                return "릴레이에 연결되지 않아 보내지 못했습니다"
            }
            Thread.sleep(100)
        }
    }

    /** 작업 스레드에서 부른다. 릴레이가 전달한 기기 수. */
    fun sendText(text: String): Int = ensureOpen().sendText(text).toInt()

    /** 작업 스레드에서 부른다. 릴레이가 전달한 기기 수. */
    fun sendImage(bytes: ByteArray): Int = ensureOpen().sendImage(bytes).toInt()

    /** [ReceivedClips]가 클립보드에 쓴 뒤 부른다. 화면의 "마지막으로 받은 것"을 고친다. */
    fun noteReceived(fromName: String, isImage: Boolean, ok: Boolean) {
        mutableState.update {
            it.copy(lastReceived = Received(fromName, isImage, ok, System.currentTimeMillis()))
        }
    }

    // ------------------------------------------------------- 계정 (F35)

    /** 계정 서버 주소를 저장한다. 로그아웃 상태에서만. 잘못되었으면 이유를, 괜찮으면 `null`. */
    fun setAccountUrl(url: String): String? {
        if (mutableState.value.signedIn) return "로그아웃한 뒤에 바꿀 수 있습니다"
        val trimmed = url.trim().trimEnd('/')
        if (!(trimmed.startsWith("http://") || trimmed.startsWith("https://"))) {
            return "계정 서버 주소는 http:// 또는 https://로 시작해야 합니다"
        }
        settings.accountUrl = trimmed
        mutableState.update { it.copy(accountUrl = trimmed) }
        refreshServerConfig()
        return null
    }

    /**
     * 구글 로그인: 계정 서버에서 웹 클라이언트 ID를 받고 → 구글 계정 선택 창(Credential Manager) →
     * ID 토큰을 계정 서버에 낸다. 메인 스레드의 코루틴에서 부른다(창은 [activity] 위에 뜬다).
     */
    suspend fun loginGoogle(activity: Activity) = login {
        val url = settings.accountUrl
        val c = ensureOpen()
        val clientId = withContext(Dispatchers.IO) { c.googleClientId(url) }
        val idToken = GoogleLogin.idToken(activity, clientId)
        withContext(Dispatchers.IO) { c.loginGoogle(url, idToken) }
    }

    /** 개발용 로그인 (디버그 빌드, 계정 서버 `--dev-login`). */
    suspend fun loginDev(email: String) = login {
        val url = settings.accountUrl
        val c = ensureOpen()
        withContext(Dispatchers.IO) { c.loginDev(url, email) }
    }

    private suspend fun login(block: suspend () -> AccountInfo) {
        if (mutableState.value.loginBusy) return
        mutableState.update { it.copy(loginBusy = true, loginMessage = null) }
        val result = try {
            Result.success(block())
        } catch (e: LoginFailed) {
            Result.failure(e)
        } catch (e: FfiException) {
            Result.failure(LoginFailed(e.userMessage()))
        } catch (e: UnsatisfiedLinkError) {
            Result.failure(LoginFailed("코어 라이브러리를 불러오지 못했습니다"))
        } finally {
            // 화면 회전 등으로 코루틴이 취소되어도 버튼이 다시 보이게 한다.
            mutableState.update { it.copy(loginBusy = false) }
        }
        result.onSuccess { info ->
            mutableState.update {
                it.copy(account = info, loginBusy = false, loginMessage = null, authRejected = false)
            }
            refresh()
            refreshServerConfig()
            // 기존 사용자(그룹이 있음)는 바로 이어서 연결한다. 없으면 첫 실행 화면으로 간다.
            if (mutableState.value.hasGroup && mutableState.value.serviceEnabled) startService()
        }.onFailure { e ->
            mutableState.update { it.copy(loginBusy = false, loginMessage = e.message) }
        }
    }

    /** 로그아웃: 연결을 멈추고 이 기기의 로그인 정보를 지운다. 그룹·페어링은 그대로다. */
    suspend fun logout() {
        ClipFerryService.stop(appContext)
        val c = ensureOpen()
        withContext(Dispatchers.IO) { c.logout() }
        GoogleLogin.clear(appContext)
        LinkNotice.clear(appContext)
        mutableState.update {
            it.copy(
                account = null,
                link = Link.STOPPED,
                online = 0,
                pairing = null,
                loginMessage = null,
                authRejected = false,
                linkRequest = null,
            )
        }
        refresh()
    }

    // ------------------------------------------------------- 첫 실행 · 그룹

    /** 새 그룹을 만들고 연결을 켠다. 실패하면 이유를 돌려준다. */
    @Synchronized
    fun createGroup(name: String): String? = guard {
        val c = ensureOpen()
        ensureDevice(c, name)
        if (c.status().epoch == null) c.createGroup()
        refresh()
        startService()
    }

    /** 그룹에서 나가 첫 실행 화면으로 돌아간다 (제거됨·혼자인 그룹·다시 페어링). */
    @Synchronized
    fun leaveGroup(): String? = guard {
        ClipFerryService.stop(appContext)
        val c = ensureOpen()
        c.leaveGroup()
        mutableState.update { it.copy(link = Link.STOPPED, online = 0, pairing = null, error = null) }
        refresh()
    }

    // ------------------------------------------------------- 페어링

    /** 이 그룹에 다른 기기를 초대한다. 초대(QR)에는 지금 릴레이 주소가 들어간다. */
    @Synchronized
    fun startInvite(): String? = guard {
        val c = ensureOpen()
        if (!c.hasGroup()) throw FfiException.Failed("먼저 그룹이 있어야 합니다")
        val info = c.startInvite(settings.relayUrl, pairListener)
        mutableState.update {
            it.copy(
                pairing = PairUi(
                    kind = PairKind.INVITE,
                    phase = PairPhase.WAITING,
                    textCode = info.textCode,
                    qr = qrMatrix(info.qrString),
                    expiresAtMs = info.expiresAtMs.toLong(),
                ),
            )
        }
    }

    /** 초대 요청을 승인(`true`)하거나 거절(`false`)한다. */
    fun decideInvite(approve: Boolean) {
        client?.decideInvite(approve)
    }

    /**
     * 초대 코드(QR 문자열 또는 텍스트 코드)로 참여를 시작한다. 기기가 없으면 [name]으로 만든다.
     * 실패하면 이유(잘못된 코드 등)를 돌려준다.
     */
    @Synchronized
    fun startJoin(name: String, code: String): String? = guard {
        val c = ensureOpen()
        ensureDevice(c, name)
        mutableState.update { it.copy(pairing = PairUi(PairKind.JOIN, PairPhase.CONNECTING)) }
        try {
            joinRelay = c.startJoin(code, settings.relayUrl, pairListener)
        } catch (e: Exception) {
            mutableState.update { it.copy(pairing = null) }
            throw e
        }
        refresh()
    }

    // ------------------------------------------------------- 계정 연결 (F37)

    /**
     * 같은 계정으로 로그인한 다른 기기에 참여를 요청한다(QR·코드 없이). 기기가 없으면 [name]으로 만든다.
     * 계정 서버에서 참여 확인서를 받으므로 작업 스레드에서 부른다. 실패하면 이유를 돌려준다.
     */
    fun startLinkJoin(name: String): String? = guard {
        val c = ensureOpen()
        synchronized(this) { ensureDevice(c, name) }
        linkJoin = true
        mutableState.update {
            it.copy(
                pairing = PairUi(
                    PairKind.JOIN,
                    PairPhase.CONNECTING,
                    message = "계정 서버에서 확인서를 받는 중입니다",
                ),
            )
        }
        try {
            c.startLinkJoin(settings.relayUrl, pairListener)
            joinRelay = settings.relayUrl
        } catch (e: Exception) {
            linkJoin = false
            mutableState.update { it.copy(pairing = null) }
            throw e
        }
        refresh()
    }

    /** 새 기기의 참여 요청에 응한다: 확인 숫자를 맞춰 보는 화면으로 간다. */
    @Synchronized
    fun acceptLinkRequest(): String? = guard {
        val c = ensureOpen()
        val peer = mutableState.value.linkRequest
        LinkNotice.clear(appContext)
        mutableState.update { it.copy(linkRequest = null) }
        c.acceptLinkRequest(settings.relayUrl, pairListener)
        mutableState.update {
            it.copy(
                pairing = PairUi(
                    kind = PairKind.INVITE,
                    phase = PairPhase.WAITING,
                    peerName = peer,
                    message = "새 기기의 응답을 기다리는 중입니다",
                ),
            )
        }
    }

    fun dismissLinkRequest() {
        client?.dismissLinkRequest()
        LinkNotice.clear(appContext)
        mutableState.update { it.copy(linkRequest = null) }
    }

    /** 페어링 화면을 닫는다(진행 중이면 취소). */
    @Synchronized
    fun closePairing() {
        client?.cancelPairing()
        joinRelay = null
        linkJoin = false
        mutableState.update { it.copy(pairing = null) }
        refresh()
    }

    private val pairListener = object : PairListener {
        // 코어 스레드에서 온다. 잠금을 잡지 않는다.
        override fun onPairEvent(event: PairEvent) {
            when (event) {
                is PairEvent.Connected -> updatePairing {
                    it.copy(
                        message = if (linkJoin) {
                            "같은 계정의 다른 기기에서 '연결'을 눌러 주기를 기다리는 중입니다. " +
                                "그 기기에서 clip-ferry가 켜져 있어야 합니다"
                        } else {
                            "초대한 기기의 응답을 기다리는 중입니다"
                        },
                    )
                }
                is PairEvent.Request -> updatePairing {
                    it.copy(phase = PairPhase.REQUEST, peerName = event.peerName, sas = event.sas)
                }
                is PairEvent.WaitingApproval -> updatePairing {
                    it.copy(
                        phase = PairPhase.WAITING,
                        peerName = event.peerName,
                        sas = event.sas,
                        message = null,
                    )
                }
                is PairEvent.Approved -> {
                    updatePairing {
                        it.copy(
                            phase = PairPhase.DONE,
                            peerName = event.peerName,
                            message = if (event.delivered) null else
                                "승인했지만 상대 기기가 응답을 받지 못했을 수 있습니다. " +
                                    "상대 앱을 확인하고, 안 되면 데스크톱에서 기기를 제거한 뒤 다시 초대하세요",
                        )
                    }
                    refresh()
                }
                is PairEvent.Rejected -> updatePairing {
                    it.copy(phase = PairPhase.FAILED, message = "참여 요청을 거절했습니다")
                }
                is PairEvent.Joined -> {
                    // 그룹에 들어왔다. QR의 릴레이를 저장하고 진행 화면을 닫은 뒤 연결한다.
                    joinRelay?.let { relay ->
                        settings.relayUrl = relay
                        mutableState.update { it.copy(relayUrl = relay) }
                    }
                    joinRelay = null
                    linkJoin = false
                    mutableState.update { it.copy(pairing = null) }
                    refresh()
                    startService()
                }
                is PairEvent.Ended -> updatePairing {
                    it.copy(phase = PairPhase.FAILED, message = event.message)
                }
            }
        }
    }

    private fun updatePairing(f: (PairUi) -> PairUi) {
        mutableState.update { s -> s.pairing?.let { s.copy(pairing = f(it)) } ?: s }
    }

    // ------------------------------------------------------- 공통

    private fun startService() {
        setServiceEnabled(true)
        ClipFerryService.start(appContext)
    }

    /** 상태와 기기 목록을 다시 읽는다. 잠금 없이 불러도 된다(코어 호출은 스레드 안전). */
    private fun refresh() {
        val c = client ?: return
        try {
            val status = if (c.isInitialized()) c.status() else null
            val devices = c.devices()
            mutableState.update { it.copy(status = status, devices = devices) }
        } catch (_: FfiException) {
            // 상태를 못 읽어도 연결 표시는 그대로 둔다.
        }
    }

    /** 실패하면 사용자에게 보여 줄 문장을, 성공하면 `null`을 돌려준다. */
    private inline fun guard(block: () -> Unit): String? = try {
        block()
        null
    } catch (e: FfiException) {
        e.userMessage()
    } catch (e: UnsatisfiedLinkError) {
        "코어 라이브러리를 불러오지 못했습니다"
    }

    private fun fail(message: String) {
        mutableState.update { it.copy(link = Link.STOPPED, online = 0, error = message) }
    }

    private fun FfiException.userMessage(): String = when (this) {
        is FfiException.Failed -> reason
        is FfiException.Removed -> "이 기기는 그룹에서 제거되었습니다. 다시 페어링하세요"
    }

    // ------------------------------------------------------- 코어 이벤트 (코어 스레드에서 온다)

    override fun onStatus(status: LinkStatus) {
        mutableState.update {
            when (status) {
                LinkStatus.CONNECTED -> it.copy(link = Link.CONNECTED, error = null, authRejected = false)
                LinkStatus.DISCONNECTED, LinkStatus.RECONNECTING ->
                    it.copy(link = Link.RECONNECTING)
            }
        }
        refresh()
    }

    override fun onOnlineCount(count: UInt) {
        mutableState.update { it.copy(online = count.toInt()) }
        refresh()
    }

    override fun onChanged() {
        refresh()
    }

    override fun onText(fromName: String, text: String) {
        ReceivedClips.onText(fromName, text)
    }

    override fun onImage(fromName: String, mime: String, bytes: ByteArray) {
        ReceivedClips.onImage(fromName, mime, bytes)
    }

    /** 같은 계정의 새 기기가 연결을 요청했다(F37). 앱 화면의 카드와 알림으로 알린다. */
    override fun onLinkRequest(peerName: String) {
        // 다른 페어링 중에는 받지 않는다(새 기기가 다시 보내면 그때 보인다).
        if (mutableState.value.pairing != null) return
        mutableState.update { it.copy(linkRequest = peerName) }
        LinkNotice.show(appContext, peerName)
    }

    override fun onAuthRejected() {
        mutableState.update { it.copy(authRejected = true) }
    }

    /** 계정 서버가 로그인을 받지 않았다. 코어는 세션을 닫았다. 서비스도 멈추고 로그인 화면으로 간다. */
    override fun onSignedOut(message: String) {
        mutableState.update {
            it.copy(
                account = null,
                link = Link.STOPPED,
                online = 0,
                pairing = null,
                loginMessage = message,
                authRejected = false,
            )
        }
        Handler(Looper.getMainLooper()).post { ClipFerryService.stop(appContext) }
    }
}

val StatusInfo.isRemoved: Boolean get() = health == Health.REMOVED
