package app.clipferry.android

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log

/**
 * 폰을 켰을 때(또는 앱이 업데이트됐을 때) 앱을 열지 않아도 백그라운드 연결을 다시 켠다.
 * `백그라운드 연결 유지`가 켜져 있고(기본값) 로그인·그룹이 있을 때만.
 * `specialUse` 포그라운드 서비스는 부팅 브로드캐스트에서 시작할 수 있다(Android 15 제한 대상 아님).
 */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            Intent.ACTION_BOOT_COMPLETED, Intent.ACTION_MY_PACKAGE_REPLACED -> Unit
            else -> return
        }
        // ClipFerryApp.onCreate에서 CoreController.init이 이미 돌았다.
        val s = CoreController.state.value
        if (!(s.serviceEnabled && s.signedIn && s.hasGroup)) return
        try {
            ClipFerryService.start(context)
        } catch (e: Exception) {
            // 시스템이 막으면 앱을 열 때 켜진다.
            Log.w("clipferry", "부팅 후 서비스 시작 실패: ${e.javaClass.simpleName}")
        }
    }
}
