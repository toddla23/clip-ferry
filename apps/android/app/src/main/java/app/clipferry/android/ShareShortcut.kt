package app.clipferry.android

import android.content.Context
import android.content.Intent
import android.util.Log
import androidx.core.content.pm.ShortcutInfoCompat
import androidx.core.content.pm.ShortcutManagerCompat
import androidx.core.graphics.drawable.IconCompat

/**
 * 공유 시트 맨 윗줄(직접 공유)에 "다른 기기로" 바로가기를 띄운다 (F19).
 *
 * `res/xml/shortcuts.xml`의 `share-target`이 [CATEGORY]와 [ShareActivity]를 잇는다. 사용자가 이 바로가기를
 * 고르면 [ShareActivity]가 평소와 같은 `ACTION_SEND`(+ `EXTRA_SHORTCUT_ID`)를 받는다.
 */
object ShareShortcut {
    private const val ID = "send_to_group"
    private const val CATEGORY = "app.clipferry.android.category.SEND_TO_GROUP"

    fun publish(context: Context) {
        val shortcut = ShortcutInfoCompat.Builder(context, ID)
            .setShortLabel("다른 기기로")
            .setLongLabel("clip-ferry로 다른 기기에 보내기")
            .setIcon(IconCompat.createWithResource(context, R.drawable.ic_launcher))
            // 바로가기 자체를 눌렀을 때(런처 길게 누르기 등)는 앱을 연다.
            .setIntent(Intent(Intent.ACTION_MAIN, null, context, MainActivity::class.java))
            .setCategories(setOf(CATEGORY))
            .setLongLived(true)
            .build()
        try {
            ShortcutManagerCompat.pushDynamicShortcut(context, shortcut)
        } catch (e: Exception) {
            // 바로가기 개수 제한·런처 문제 등. 공유 시트 일반 목록에는 그대로 남는다.
            Log.w("clipferry", "공유 바로가기 등록 실패: ${e.javaClass.simpleName}")
        }
    }
}
