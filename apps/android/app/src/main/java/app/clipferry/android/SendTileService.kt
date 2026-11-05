package app.clipferry.android

import android.annotation.SuppressLint
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService

/**
 * 빠른 설정 타일 "클립보드 보내기" (F18). 누르면 패널을 닫고 [SendActivity]를 연다.
 * 타일은 그룹이 있으면 켜진 모양, 없으면 꺼진 모양이다(눌러도 안내만 나온다).
 */
class SendTileService : TileService() {
    override fun onStartListening() {
        super.onStartListening()
        val tile = qsTile ?: return
        val state = CoreController.state.value
        tile.state = if (state.hasGroup && !state.removed) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
        if (Build.VERSION.SDK_INT >= 29) {
            tile.subtitle = when {
                !state.hasGroup -> "그룹 없음"
                state.removed -> "다시 페어링 필요"
                state.link == Link.CONNECTED -> "연결됨"
                else -> "연결 안 됨"
            }
        }
        tile.updateTile()
    }

    override fun onClick() {
        super.onClick()
        if (isLocked) unlockAndRun { open() } else open()
    }

    @SuppressLint("StartActivityAndCollapseDeprecated")
    private fun open() {
        if (Build.VERSION.SDK_INT >= 34) {
            startActivityAndCollapse(sendPendingIntent(this))
        } else {
            @Suppress("DEPRECATION")
            startActivityAndCollapse(sendIntent(this))
        }
    }

    companion object {
        fun sendIntent(context: Context): Intent =
            Intent(context, SendActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_NO_ANIMATION)

        fun sendPendingIntent(context: Context): PendingIntent =
            PendingIntent.getActivity(
                context,
                10,
                sendIntent(context),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
    }
}
