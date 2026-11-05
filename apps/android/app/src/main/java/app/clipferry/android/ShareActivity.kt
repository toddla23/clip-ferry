package app.clipferry.android

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.widget.Toast
import java.io.File
import java.util.UUID
import java.util.concurrent.Executors

/**
 * 공유 시트와 텍스트 선택 메뉴에서 받은 내용을 다른 기기로 보낸다 (F19). 보이지 않는다.
 *
 * - `ACTION_SEND`: 텍스트(`EXTRA_TEXT`) 또는 이미지 한 장(`EXTRA_STREAM`)
 * - `ACTION_SEND_MULTIPLE`: 이미지 여러 장. 순서대로 한 장씩 보낸다(받는 쪽 클립보드에는 마지막 장이 남는다)
 * - `ACTION_PROCESS_TEXT`: 선택한 텍스트. 원래 텍스트는 바꾸지 않는다
 *
 * 공유로 받은 이미지 URI의 읽기 권한은 이 액티비티가 살아 있는 동안만 믿을 수 있다. 그래서 이미지는
 * 액티비티가 떠 있는 동안 `cache/outgoing/`에 복사해 두고 닫힌 뒤 [SendClipboard.sendBatch]가 보낸다.
 * 사용자가 직접 고른 내용이므로 민감 표시 설정은 따지지 않는다(공유 인텐트에는 그런 표시가 없다).
 */
class ShareActivity : Activity() {
    private var started = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        noTransition()
        // PROCESS_TEXT는 결과를 기다린다. 텍스트를 바꾸지 않으므로 취소로 돌려준다.
        setResult(RESULT_CANCELED)
    }

    override fun onResume() {
        super.onResume()
        if (started) return
        started = true
        val shared = parse(intent)
        if (shared == null) {
            toast("보낼 수 없는 형식입니다 (텍스트·이미지만)")
            close()
            return
        }
        // 화면에 있는 지금 서비스를 켜 둔다(백그라운드에서는 포그라운드 서비스를 시작할 수 없다).
        CoreController.ensureRunning()
        when (shared) {
            is Shared.Text -> {
                SendClipboard.sendBatch(this, listOf(SendClipboard.Outgoing.Text(shared.text)))
                close()
            }
            is Shared.Images -> copier.execute {
                val outgoing = copyImages(shared.uris)
                main.post {
                    SendClipboard.sendBatch(this, outgoing)
                    close()
                }
            }
        }
    }

    private sealed class Shared {
        data class Text(val text: String) : Shared()
        data class Images(val uris: List<Uri>) : Shared()
    }

    private fun parse(intent: Intent?): Shared? {
        intent ?: return null
        return when (intent.action) {
            Intent.ACTION_PROCESS_TEXT ->
                intent.getCharSequenceExtra(Intent.EXTRA_PROCESS_TEXT)?.toString()
                    ?.takeIf { it.isNotEmpty() }?.let { Shared.Text(it) }
            Intent.ACTION_SEND -> {
                val uri = streamExtra(intent)
                val text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()
                when {
                    uri != null && isImage(intent.type, uri) -> Shared.Images(listOf(uri))
                    !text.isNullOrEmpty() -> Shared.Text(text)
                    else -> null
                }
            }
            Intent.ACTION_SEND_MULTIPLE -> {
                val uris = streamListExtra(intent).filter { isImage(intent.type, it) }
                if (uris.isEmpty()) null else Shared.Images(uris)
            }
            else -> null
        }
    }

    /** 인텐트 형식이 `image/*`면 믿고, `*/*` 같은 섞인 형식이면 하나씩 형식을 묻는다. */
    private fun isImage(type: String?, uri: Uri): Boolean {
        if (type != null && type.startsWith("image/")) return true
        return try {
            contentResolver.getType(uri)?.startsWith("image/") == true
        } catch (e: Exception) {
            false
        }
    }

    /** 작업 스레드에서. 한 장씩 읽어(형식 변환·크기 확인 포함) 파일로 둔다. 실패한 장은 이유를 남긴다. */
    private fun copyImages(uris: List<Uri>): List<SendClipboard.Outgoing> {
        val dir = File(cacheDir, "$OUTGOING/${UUID.randomUUID()}").apply { mkdirs() }
        return uris.mapIndexed { i, uri ->
            try {
                val bytes = SendClipboard.loadImage(this, uri)
                val file = File(dir, "$i")
                file.writeBytes(bytes)
                SendClipboard.Outgoing.ImageFile(file)
            } catch (e: SecurityException) {
                Log.w(TAG, "공유받은 이미지를 읽을 권한 없음")
                SendClipboard.Outgoing.Failed("이미지를 읽을 수 없습니다")
            } catch (e: SendClipboard.TooLarge) {
                SendClipboard.Outgoing.Failed("이미지가 20 MiB를 넘습니다")
            } catch (e: Exception) {
                Log.w(TAG, "공유받은 이미지 읽기 실패: ${e.javaClass.simpleName}")
                SendClipboard.Outgoing.Failed(
                    (e as? uniffi.clip_ferry_ffi.FfiException.Failed)?.reason ?: "이미지를 읽지 못했습니다",
                )
            }
        }
    }

    @Suppress("DEPRECATION")
    private fun streamExtra(intent: Intent): Uri? =
        if (Build.VERSION.SDK_INT >= 33) intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
        else intent.getParcelableExtra(Intent.EXTRA_STREAM)

    @Suppress("DEPRECATION")
    private fun streamListExtra(intent: Intent): List<Uri> =
        (
            if (Build.VERSION.SDK_INT >= 33) {
                intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM)
            }
            )?.filterNotNull() ?: emptyList()

    private fun toast(message: String) {
        Toast.makeText(applicationContext, message, Toast.LENGTH_SHORT).show()
    }

    private fun close() {
        finish()
        noTransition()
    }

    @Suppress("DEPRECATION")
    private fun noTransition() {
        if (Build.VERSION.SDK_INT >= 34) {
            overrideActivityTransition(OVERRIDE_TRANSITION_OPEN, 0, 0)
            overrideActivityTransition(OVERRIDE_TRANSITION_CLOSE, 0, 0)
        } else {
            overridePendingTransition(0, 0)
        }
    }

    companion object {
        private const val TAG = "clipferry"

        /** 공유받은 이미지를 보내기 전에 잠깐 두는 곳(`cacheDir` 아래). 보내고 나면 지운다. */
        const val OUTGOING = "outgoing"

        private val copier = Executors.newSingleThreadExecutor { r -> Thread(r, "clip-ferry-share") }
        private val main = Handler(Looper.getMainLooper())

        /** 앱이 켜질 때 지난번에 못 보내고 남은 파일을 지운다. */
        fun clearStale(app: android.content.Context) {
            File(app.cacheDir, OUTGOING).deleteRecursively()
        }
    }
}
