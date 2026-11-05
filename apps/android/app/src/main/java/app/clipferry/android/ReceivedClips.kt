package app.clipferry.android

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PersistableBundle
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.content.FileProvider
import java.io.File
import java.util.concurrent.Executors

/**
 * 다른 기기에서 받은 텍스트·이미지를 이 기기 클립보드에 쓴다 (F17).
 *
 * - 텍스트는 그대로 쓴다.
 * - 이미지는 앱 캐시(`cache/received/`)에 파일로 두고 [FileProvider] URI로 쓴다.
 *   붙여넣는 앱은 클립보드가 넘겨주는 임시 권한으로 이 파일을 읽는다.
 *   캐시에는 가장 최근 것 하나만 남긴다(새로 받으면 이전 파일은 지운다. 텍스트를 받아도 지운다).
 * - 내용은 로그·알림에 남기지 않는다. 알림에는 보낸 기기 이름과 종류만 적는다.
 *
 * 코어 스레드에서 불린다. 파일 쓰기는 작업 스레드 하나에서, 클립보드 쓰기는 메인 스레드에서 한다.
 * 둘 다 순서대로 처리하므로 받은 순서대로 클립보드에 들어간다.
 */
object ReceivedClips {
    private const val TAG = "clipferry"
    private const val CHANNEL_ID = "received"
    private const val NOTIFICATION_ID = 2
    /** 받아서 넣은 클립의 이름표. 보내기([SendClipboard])가 이것을 보고 되돌려 보내지 않는다. */
    const val LABEL = "clip-ferry"
    private const val NOTIFICATION_TIMEOUT_MS = 15_000L

    private lateinit var appContext: Context
    private lateinit var settings: Settings
    private val worker = Executors.newSingleThreadExecutor { r -> Thread(r, "clip-ferry-received") }
    private val main = Handler(Looper.getMainLooper())

    fun init(context: Context, settings: Settings) {
        appContext = context.applicationContext
        this.settings = settings
        createChannel()
    }

    /** 받은 텍스트. */
    fun onText(fromName: String, text: String) {
        worker.execute {
            main.post {
                val ok = setClip(ClipData.newPlainText(LABEL, text))
                // 클립보드가 바뀐 뒤에만 이전 이미지 파일을 지운다.
                if (ok) worker.execute { deleteOldFiles(keep = null) }
                report(fromName, Kind.TEXT, ok)
            }
        }
    }

    /** 받은 이미지. `mime`은 코어가 머리말로 확인한 형식이다. */
    fun onImage(fromName: String, mime: String, bytes: ByteArray) {
        worker.execute {
            val file = try {
                writeFile(mime, bytes)
            } catch (e: Exception) {
                Log.w(TAG, "받은 이미지를 저장하지 못함: ${e.javaClass.simpleName}")
                main.post { report(fromName, Kind.IMAGE, ok = false) }
                return@execute
            }
            val uri = FileProvider.getUriForFile(appContext, authority(), file)
            main.post {
                val clip = ClipData.newUri(appContext.contentResolver, LABEL, uri)
                val ok = setClip(clip)
                worker.execute { deleteOldFiles(keep = if (ok) file else null) }
                report(fromName, Kind.IMAGE, ok)
            }
        }
    }

    private enum class Kind { TEXT, IMAGE }

    /** 메인 스레드에서 부른다. 실패(예: 너무 큰 텍스트로 시스템 전달 한도 초과)하면 `false`. */
    private fun setClip(clip: ClipData): Boolean {
        if (Build.VERSION.SDK_INT >= 33) {
            // 다른 기기에서 온 내용임을 알린다. 시스템이 복사 확인 표시를 그에 맞게 보여 준다.
            clip.description.extras = PersistableBundle().apply {
                putBoolean(ClipDescription.EXTRA_IS_REMOTE_DEVICE, true)
            }
        }
        return try {
            appContext.getSystemService(ClipboardManager::class.java).setPrimaryClip(clip)
            true
        } catch (e: RuntimeException) {
            Log.w(TAG, "클립보드에 쓰지 못함: ${e.javaClass.simpleName}")
            false
        }
    }

    private fun dir(): File = File(appContext.cacheDir, "received").apply { mkdirs() }

    private fun authority(): String = "${appContext.packageName}.files"

    private fun writeFile(mime: String, bytes: ByteArray): File {
        val ext = when (mime) {
            "image/png" -> "png"
            "image/jpeg" -> "jpg"
            "image/gif" -> "gif"
            "image/webp" -> "webp"
            else -> throw IllegalArgumentException("알 수 없는 형식")
        }
        // 이름을 매번 바꾼다. 붙여넣는 앱이 같은 URI를 예전 내용으로 기억하지 않게 한다.
        val file = File(dir(), "clip-${System.currentTimeMillis()}.$ext")
        file.writeBytes(bytes)
        return file
    }

    /** `keep`을 뺀 받은 파일을 모두 지운다. */
    private fun deleteOldFiles(keep: File?) {
        dir().listFiles()?.forEach { f ->
            if (f != keep) f.delete()
        }
    }

    private fun report(fromName: String, kind: Kind, ok: Boolean) {
        CoreController.noteReceived(fromName, isImage = kind == Kind.IMAGE, ok = ok)
        if (!settings.notifyOnReceive && ok) return
        val what = if (kind == Kind.IMAGE) "이미지" else "텍스트"
        val text = if (ok) {
            "'$fromName'에서 받은 ${what}를 클립보드에 넣었습니다"
        } else if (kind == Kind.TEXT) {
            "'$fromName'에서 받은 텍스트가 너무 커서 클립보드에 넣지 못했습니다"
        } else {
            "'$fromName'에서 받은 이미지를 클립보드에 넣지 못했습니다"
        }
        val open = PendingIntent.getActivity(
            appContext,
            0,
            Intent(appContext, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val notification = NotificationCompat.Builder(appContext, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_ferry)
            .setContentTitle(appContext.getString(R.string.notif_title))
            .setContentText(text)
            .setContentIntent(open)
            .setAutoCancel(true)
            .setTimeoutAfter(NOTIFICATION_TIMEOUT_MS)
            .build()
        // 알림 권한이 없으면 시스템이 조용히 무시한다.
        appContext.getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, notification)
    }

    private fun createChannel() {
        val channel = NotificationChannel(
            CHANNEL_ID,
            appContext.getString(R.string.received_channel_name),
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = appContext.getString(R.string.received_channel_description)
            setShowBadge(false)
        }
        appContext.getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
    }
}
