package app.clipferry.android

import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.widget.Toast
import java.io.ByteArrayOutputStream
import java.io.File
import java.util.concurrent.Executors
import uniffi.clip_ferry_ffi.FfiException

/**
 * 이 기기 클립보드의 텍스트·이미지를 그룹의 다른 기기로 보낸다 (F18).
 *
 * 클립보드는 화면에 초점이 있는 앱만 읽을 수 있다(안드로이드 10+). 그래서 [SendActivity]가
 * 초점을 받은 순간 [ClipboardManager.getPrimaryClip]으로 꺼내 [send]에 넘기고 바로 닫힌다.
 * 내용을 읽고 보내는 일은 작업 스레드에서 하고, 결과는 토스트로 알린다.
 *
 * 내용은 로그·토스트에 남기지 않는다.
 */
object SendClipboard {
    private const val TAG = "clipferry"
    private const val MAX_IMAGE_BYTES = 20 * 1024 * 1024
    private const val CONNECT_WAIT_MS = 8_000L

    private val worker = Executors.newSingleThreadExecutor { r -> Thread(r, "clip-ferry-send") }
    private val main = Handler(Looper.getMainLooper())

    /** 무엇을 보낼지. 클립보드를 읽은 직후(초점이 있을 때) 만든다. */
    sealed class Content {
        data class Text(val text: String) : Content()
        data class Image(val uri: Uri) : Content()
        data class Skip(val reason: String) : Content()
    }

    /**
     * 클립보드에서 보낼 것을 꺼낸다. 화면에 초점이 있을 때(메인 스레드) 불러야 한다.
     * 이미지 URI의 읽기 권한은 이때 시스템이 이 앱에 준다.
     */
    fun read(context: Context, syncSensitive: Boolean): Content {
        val cm = context.getSystemService(ClipboardManager::class.java)
        val clip: ClipData = try {
            cm.primaryClip
        } catch (e: SecurityException) {
            null
        } ?: return Content.Skip("클립보드가 비어 있거나 읽을 수 없습니다")
        if (clip.itemCount == 0) return Content.Skip("클립보드가 비어 있습니다")
        val desc = clip.description
        if (desc.label?.toString() == ReceivedClips.LABEL) {
            // 다른 기기에서 받아 이 앱이 넣은 내용이다. 그룹은 이미 갖고 있다.
            return Content.Skip("다른 기기에서 받은 내용이라 다시 보내지 않았습니다")
        }
        if (!syncSensitive && isSensitive(desc)) {
            return Content.Skip("민감한 내용으로 표시된 복사라 보내지 않았습니다 (설정에서 바꿀 수 있음)")
        }
        val item = clip.getItemAt(0)
        val uri = item.uri
        if (uri != null && isImageUri(context, desc, uri)) return Content.Image(uri)
        val text = item.text?.toString()
            ?: item.htmlText
            ?: if (uri == null) item.coerceToText(context)?.toString() else null
        if (text.isNullOrEmpty()) {
            return Content.Skip(
                if (uri != null) "보낼 수 없는 형식입니다 (텍스트·이미지만)" else "클립보드가 비어 있습니다",
            )
        }
        return Content.Text(text)
    }

    /** 작업 스레드에서 보내고 결과를 토스트로 알린다. */
    fun send(context: Context, content: Content) {
        val app = context.applicationContext
        if (content is Content.Skip) {
            toast(app, content.reason)
            return
        }
        worker.execute {
            val message = try {
                val wait = CoreController.awaitConnected(CONNECT_WAIT_MS)
                if (wait != null) {
                    wait
                } else {
                    val delivered = when (content) {
                        is Content.Text -> CoreController.sendText(content.text)
                        is Content.Image -> CoreController.sendImage(loadImage(app, content.uri))
                        is Content.Skip -> 0
                    }
                    val what = if (content is Content.Image) "이미지" else "텍스트"
                    if (delivered > 0) "${what}를 다른 기기 ${delivered}대로 보냈습니다"
                    else "보냈지만 지금 연결된 다른 기기가 없습니다"
                }
            } catch (e: SecurityException) {
                Log.w(TAG, "클립보드 이미지를 읽을 권한 없음")
                "이미지를 읽을 수 없습니다. 복사한 앱이 다른 앱에 열어 주지 않았습니다"
            } catch (e: TooLarge) {
                "이미지가 20 MiB를 넘어 보내지 않았습니다"
            } catch (e: FfiException) {
                when (e) {
                    is FfiException.Failed -> e.reason
                    is FfiException.Removed -> "이 기기는 그룹에서 제거되었습니다. 다시 페어링하세요"
                }
            } catch (e: Exception) {
                Log.w(TAG, "보내기 실패: ${e.javaClass.simpleName}")
                "보내지 못했습니다"
            }
            toast(app, message)
        }
    }

    // ------------------------------------------------------- 공유 · 텍스트 선택 메뉴 (F19)

    /** [ShareActivity]가 넘기는 것. 이미지는 액티비티가 떠 있는 동안 캐시 파일로 복사해 둔다. */
    sealed class Outgoing {
        data class Text(val text: String) : Outgoing()
        data class ImageFile(val file: File) : Outgoing()
        data class Failed(val reason: String) : Outgoing()
    }

    /**
     * 순서대로 한 건씩 보내고 결과를 토스트 하나로 알린다. 보낸 이미지 파일은 지운다.
     * 한 건이 실패해도 나머지는 계속 보낸다.
     */
    fun sendBatch(context: Context, items: List<Outgoing>) {
        val app = context.applicationContext
        worker.execute {
            val message = try {
                CoreController.awaitConnected(CONNECT_WAIT_MS) ?: sendEach(items)
            } finally {
                items.forEach { if (it is Outgoing.ImageFile) it.file.delete() }
                items.firstNotNullOfOrNull { (it as? Outgoing.ImageFile)?.file?.parentFile }?.delete()
            }
            toast(app, message)
        }
    }

    /** 작업 스레드에서. 사용자에게 보여 줄 결과 한 줄을 돌려준다. */
    private fun sendEach(items: List<Outgoing>): String {
        var sent = 0
        var delivered = 0
        var lastError: String? = null
        for (item in items) {
            try {
                delivered = when (item) {
                    is Outgoing.Text -> CoreController.sendText(item.text)
                    is Outgoing.ImageFile -> CoreController.sendImage(item.file.readBytes())
                    is Outgoing.Failed -> {
                        lastError = item.reason
                        continue
                    }
                }
                sent++
            } catch (e: FfiException) {
                lastError = when (e) {
                    is FfiException.Failed -> e.reason
                    is FfiException.Removed -> return "이 기기는 그룹에서 제거되었습니다. 다시 페어링하세요"
                }
            } catch (e: Exception) {
                Log.w(TAG, "보내기 실패: ${e.javaClass.simpleName}")
                lastError = "보내지 못했습니다"
            }
        }
        val images = items.count { it !is Outgoing.Text }
        val what = when {
            images == 0 -> "텍스트를"
            items.size == 1 -> "이미지를"
            else -> "이미지 ${sent}장을"
        }
        return when {
            sent == 0 -> lastError ?: "보내지 못했습니다"
            sent < items.size -> "${items.size}장 중 ${sent}장만 보냈습니다 ($lastError)"
            delivered > 0 -> "$what 다른 기기 ${delivered}대로 보냈습니다"
            else -> "보냈지만 지금 연결된 다른 기기가 없습니다"
        }
    }

    class TooLarge : Exception()

    private fun isSensitive(desc: ClipDescription): Boolean {
        val extras = desc.extras ?: return false
        // 안드로이드 13+의 표시. 그전에도 일부 앱은 같은 키를 쓴다.
        return extras.getBoolean(EXTRA_IS_SENSITIVE, false)
    }

    private fun isImageUri(context: Context, desc: ClipDescription, uri: Uri): Boolean {
        if (desc.hasMimeType("image/*")) return true
        return try {
            context.contentResolver.getType(uri)?.startsWith("image/") == true
        } catch (e: Exception) {
            false
        }
    }

    /**
     * 이미지를 읽는다. PNG·JPEG·GIF·WebP는 그대로 보내고, 다른 형식(HEIC 등)은 JPEG로 바꾼다.
     * 작업 스레드에서 부른다. 권한이 없으면 [SecurityException], 20 MiB를 넘으면 [TooLarge].
     */
    fun loadImage(context: Context, uri: Uri): ByteArray {
        val bytes = context.contentResolver.openInputStream(uri)?.use { input ->
            val out = ByteArrayOutputStream()
            val buf = ByteArray(64 * 1024)
            while (true) {
                val n = input.read(buf)
                if (n < 0) break
                out.write(buf, 0, n)
                // 바꿔서 줄어들 수도 있으니 원본은 조금 더 크게 허용한다.
                if (out.size() > MAX_IMAGE_BYTES * 2) throw TooLarge()
            }
            out.toByteArray()
        } ?: throw SecurityException("열 수 없음")
        val raw = if (isSupported(bytes)) bytes else toJpeg(bytes)
        if (raw.size > MAX_IMAGE_BYTES) throw TooLarge()
        return raw
    }

    private fun isSupported(b: ByteArray): Boolean {
        fun at(i: Int) = if (i < b.size) b[i].toInt() and 0xFF else -1
        val png = at(0) == 0x89 && at(1) == 0x50 && at(2) == 0x4E && at(3) == 0x47
        val jpeg = at(0) == 0xFF && at(1) == 0xD8
        val gif = at(0) == 'G'.code && at(1) == 'I'.code && at(2) == 'F'.code
        val webp = at(0) == 'R'.code && at(1) == 'I'.code && at(8) == 'W'.code && at(9) == 'E'.code
        return png || jpeg || gif || webp
    }

    private fun toJpeg(bytes: ByteArray): ByteArray {
        val bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
            ?: throw FfiException.Failed("보낼 수 없는 이미지 형식입니다")
        return try {
            ByteArrayOutputStream().also { bitmap.compress(Bitmap.CompressFormat.JPEG, 92, it) }
                .toByteArray()
        } finally {
            bitmap.recycle()
        }
    }

    private fun toast(context: Context, message: String) {
        main.post { Toast.makeText(context, message, Toast.LENGTH_SHORT).show() }
    }

    /** `ClipDescription.EXTRA_IS_SENSITIVE` (안드로이드 13+ 상수). 낮은 버전에서도 같은 키를 읽는다. */
    private val EXTRA_IS_SENSITIVE =
        if (Build.VERSION.SDK_INT >= 33) ClipDescription.EXTRA_IS_SENSITIVE
        else "android.content.extra.IS_SENSITIVE"
}
