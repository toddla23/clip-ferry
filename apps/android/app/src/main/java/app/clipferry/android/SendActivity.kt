package app.clipferry.android

import android.app.Activity
import android.os.Build
import android.os.Bundle

/**
 * 보이지 않는 액티비티. 빠른 설정 타일·알림 버튼이 연다 (F18).
 *
 * 안드로이드 10+는 초점이 있는 앱만 클립보드를 읽게 한다. 이 액티비티는 투명하게 떠서
 * 초점을 받는 순간 클립보드를 읽고 보내기를 맡긴 뒤 바로 닫힌다. 보내기와 결과 알림(토스트)은
 * [SendClipboard]가 작업 스레드에서 한다. 화면 전환 효과를 끄므로 원래 앱 위에서 깜빡이지 않는다.
 */
class SendActivity : Activity() {
    private var handled = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        noTransition()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (!hasFocus || handled) return
        handled = true
        val content = SendClipboard.read(this, CoreController.syncSensitive)
        // 연결이 꺼져 있으면 지금(앱이 화면에 있을 때) 서비스를 켜 둔다. 보내기는 연결을 잠깐 기다린다.
        if (content !is SendClipboard.Content.Skip) CoreController.ensureRunning()
        SendClipboard.send(this, content)
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
}
