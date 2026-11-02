package app.clipferry.android

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.Network
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch

/**
 * 릴레이 연결을 유지하는 포그라운드 서비스. 상시 알림에 연결 상태를 보여 준다.
 * 앱 화면(최근 앱)을 지워도 서비스는 남는다.
 */
class ClipFerryService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private var restartJob: Job? = null
    private var stateJob: Job? = null
    private var firstNetworkEvent = true
    private var networkCallback: ConnectivityManager.NetworkCallback? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        createChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            CoreController.setServiceEnabled(false)
            stopSelf()
            return START_NOT_STICKY
        }
        // startForegroundService 후 5초 안에 불러야 한다. 연결보다 먼저 한다.
        ServiceCompat.startForeground(
            this,
            NOTIFICATION_ID,
            buildNotification(CoreController.state.value),
            ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE,
        )
        CoreController.start()
        watchState()
        watchNetwork()
        return START_STICKY
    }

    override fun onDestroy() {
        networkCallback?.let {
            getSystemService(ConnectivityManager::class.java).unregisterNetworkCallback(it)
        }
        networkCallback = null
        scope.cancel()
        CoreController.stop()
        super.onDestroy()
    }

    /** 상태가 바뀔 때마다 알림을 고친다. 같은 문구면 다시 그리지 않는다. */
    private fun watchState() {
        stateJob?.cancel()
        stateJob = scope.launch {
            CoreController.state
                .map { buildNotification(it) to notificationText(it) }
                .distinctUntilChanged { a, b -> a.second == b.second }
                .collect { (notification, _) ->
                    getSystemService(NotificationManager::class.java)
                        .notify(NOTIFICATION_ID, notification)
                }
        }
    }

    /**
     * 기본 네트워크가 바뀌면(Wi-Fi ↔ LTE 등) 연결을 새로 연다.
     * 안 하면 끊어진 연결이 핑 시간 초과(최대 60초)를 기다리다 뒤늦게 복구된다.
     */
    private fun watchNetwork() {
        if (networkCallback != null) return
        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                // 등록 직후 현재 네트워크로 오는 첫 알림은 바뀐 것이 아니다.
                if (firstNetworkEvent) {
                    firstNetworkEvent = false
                    return
                }
                scheduleRestart()
            }
        }
        networkCallback = callback
        getSystemService(ConnectivityManager::class.java).registerDefaultNetworkCallback(callback)
    }

    /** 짧은 시간에 여러 번 바뀌어도 한 번만 다시 연결한다. */
    private fun scheduleRestart() {
        restartJob?.cancel()
        restartJob = scope.launch {
            delay(NETWORK_SETTLE_MS)
            CoreController.restart()
        }
    }

    private fun createChannel() {
        val channel = NotificationChannel(
            CHANNEL_ID,
            getString(R.string.channel_name),
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = getString(R.string.channel_description)
            setShowBadge(false)
        }
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
    }

    private fun notificationText(state: UiState): String = when {
        state.status?.isRemoved == true -> getString(R.string.notif_removed)
        state.link == Link.CONNECTED && state.online > 1 ->
            getString(R.string.notif_connected_online, state.online)
        state.link == Link.CONNECTED -> getString(R.string.notif_connected)
        state.link == Link.RECONNECTING -> getString(R.string.notif_reconnecting)
        else -> getString(R.string.notif_stopped)
    }

    private fun buildNotification(state: UiState): Notification {
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val stop = PendingIntent.getService(
            this,
            1,
            Intent(this, ClipFerryService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_ferry)
            .setContentTitle(getString(R.string.notif_title))
            .setContentText(notificationText(state))
            .setContentIntent(open)
            .addAction(0, getString(R.string.notif_action_stop), stop)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .build()
    }

    companion object {
        private const val CHANNEL_ID = "connection"
        private const val NOTIFICATION_ID = 1
        private const val ACTION_STOP = "app.clipferry.android.STOP"
        private const val NETWORK_SETTLE_MS = 1_000L

        fun start(context: Context) {
            context.startForegroundService(Intent(context, ClipFerryService::class.java))
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, ClipFerryService::class.java))
        }
    }
}
