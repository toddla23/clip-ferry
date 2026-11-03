package app.clipferry.android

import android.content.Context
import android.os.Build
import java.io.File
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import uniffi.clip_ferry_ffi.ClipFerryClient
import uniffi.clip_ferry_ffi.CoreEvents
import uniffi.clip_ferry_ffi.FfiException
import uniffi.clip_ferry_ffi.Health
import uniffi.clip_ferry_ffi.LinkStatus
import uniffi.clip_ferry_ffi.StatusInfo

enum class Link { STOPPED, RECONNECTING, CONNECTED }

/** 화면과 알림이 읽는 상태. 비밀값은 없다. */
data class UiState(
    val link: Link = Link.STOPPED,
    /** 그룹 채널에 붙어 있는 기기 수 (이 기기 포함). */
    val online: Int = 0,
    val status: StatusInfo? = null,
    val error: String? = null,
    val relayUrl: String = "",
    /** 백그라운드 연결 유지(포그라운드 서비스)를 켜 두었는가. */
    val serviceEnabled: Boolean = true,
)

/**
 * 코어(Rust)와 화면 사이의 얇은 층. 암호·프로토콜 로직은 없다.
 * 코어를 열고, 릴레이 세션을 시작·중지하고, 코어가 알려 주는 상태를 [UiState]로 모은다.
 */
object CoreController : CoreEvents {
    private lateinit var appContext: Context
    private lateinit var settings: Settings
    private var client: ClipFerryClient? = null

    private val mutableState = MutableStateFlow(UiState())
    val state: StateFlow<UiState> = mutableState

    fun init(context: Context) {
        appContext = context.applicationContext
        settings = Settings(appContext)
        mutableState.value = UiState(
            relayUrl = settings.relayUrl,
            serviceEnabled = settings.serviceEnabled,
        )
    }

    /**
     * 코어를 열고, 처음이면 기기와 그룹을 만든다.
     * (임시: 페어링 화면은 F16에서 생긴다. 그때까지는 데스크톱 F09처럼 혼자인 그룹으로 시작한다.)
     */
    @Synchronized
    private fun ensureOpen(): ClipFerryClient {
        client?.let { return it }
        val dir = File(appContext.filesDir, "clip-ferry").absolutePath
        val opened = ClipFerryClient.open(dir, KeystoreVault(appContext))
        if (!opened.isInitialized()) {
            opened.initDevice(Build.MODEL.trim().ifEmpty { "Android" }.take(64))
        }
        if (opened.status().epoch == null) {
            opened.createGroup()
        }
        client = opened
        return opened
    }

    /** 릴레이에 (다시) 연결한다. 이미 연결 중이면 닫고 새로 연다. */
    @Synchronized
    fun start() {
        try {
            val c = ensureOpen()
            c.start(settings.relayUrl, this)
            mutableState.update {
                it.copy(
                    link = Link.RECONNECTING,
                    error = null,
                    status = c.status(),
                    relayUrl = settings.relayUrl,
                )
            }
        } catch (e: FfiException.Removed) {
            // 제거된 기기는 화면의 안내 문구로 충분하다.
            mutableState.update { it.copy(link = Link.STOPPED, error = null, online = 0) }
            refreshStatus()
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
    }

    /** 네트워크가 바뀌었을 때: 끊긴 연결이 시간 초과를 기다리지 않고 바로 다시 붙게 한다. */
    @Synchronized
    fun restart() {
        if (client?.isRunning() == true) start()
    }

    /** 기기에 네트워크가 없을 때: 코어가 알아채기 전에 화면 표시만 먼저 바꾼다. */
    @Synchronized
    fun markOffline() {
        mutableState.update {
            if (it.link == Link.CONNECTED) it.copy(link = Link.RECONNECTING, online = 0) else it
        }
    }

    /** 릴레이 주소를 저장한다. 잘못되었으면 이유를, 괜찮으면 `null`을 돌려준다. */
    fun setRelayUrl(url: String): String? {
        val trimmed = url.trim()
        if (!(trimmed.startsWith("ws://") || trimmed.startsWith("wss://"))) {
            return "릴레이 주소는 ws:// 또는 wss://로 시작해야 합니다"
        }
        settings.relayUrl = trimmed
        mutableState.update { it.copy(relayUrl = trimmed) }
        return null
    }

    fun setServiceEnabled(enabled: Boolean) {
        settings.serviceEnabled = enabled
        mutableState.update { it.copy(serviceEnabled = enabled) }
    }

    @Synchronized
    private fun refreshStatus() {
        val c = client ?: return
        try {
            mutableState.update { it.copy(status = c.status()) }
        } catch (_: FfiException) {
            // 상태를 못 읽어도 연결 표시는 그대로 둔다.
        }
    }

    private fun fail(message: String) {
        mutableState.update { it.copy(link = Link.STOPPED, online = 0, error = message) }
    }

    private fun FfiException.userMessage(): String =
        (this as? FfiException.Failed)?.reason ?: "알 수 없는 오류"

    // ------------------------------------------------------- 코어 이벤트 (코어 스레드에서 온다)

    override fun onStatus(status: LinkStatus) {
        mutableState.update {
            when (status) {
                LinkStatus.CONNECTED -> it.copy(link = Link.CONNECTED, error = null)
                LinkStatus.DISCONNECTED, LinkStatus.RECONNECTING ->
                    it.copy(link = Link.RECONNECTING)
            }
        }
    }

    override fun onOnlineCount(count: UInt) {
        mutableState.update { it.copy(online = count.toInt()) }
    }

    override fun onChanged() {
        refreshStatus()
    }
}

val StatusInfo.isRemoved: Boolean get() = health == Health.REMOVED
