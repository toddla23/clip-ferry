package app.clipferry.android

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import androidx.core.app.NotificationCompat

/**
 * 같은 계정의 새 기기가 연결을 요청했을 때(F37) 띄우는 알림. 누르면 앱이 열리고 메인 화면의
 * 연결 요청 카드에서 연결·무시를 고른다. 요청은 10분 뒤 만료되므로 알림도 그때 사라진다.
 */
object LinkNotice {
    private const val CHANNEL_ID = "link_request"
    private const val NOTIFICATION_ID = 3
    private const val TIMEOUT_MS = 10 * 60 * 1000L

    fun show(context: Context, peerName: String) {
        val app = context.applicationContext
        val manager = app.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_ID,
                app.getString(R.string.link_channel_name),
                NotificationManager.IMPORTANCE_DEFAULT,
            ).apply { description = app.getString(R.string.link_channel_description) },
        )
        val open = PendingIntent.getActivity(
            app,
            0,
            Intent(app, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val notification = NotificationCompat.Builder(app, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_ferry)
            .setContentTitle(app.getString(R.string.notif_title))
            .setContentText(app.getString(R.string.link_notice_text, peerName))
            .setContentIntent(open)
            .setAutoCancel(true)
            .setTimeoutAfter(TIMEOUT_MS)
            .build()
        // 알림 권한이 없으면 시스템이 조용히 무시한다. 앱을 열면 카드로 보인다.
        manager.notify(NOTIFICATION_ID, notification)
    }

    fun clear(context: Context) {
        context.applicationContext.getSystemService(NotificationManager::class.java)
            .cancel(NOTIFICATION_ID)
    }
}
