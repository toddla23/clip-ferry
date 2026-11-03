package app.clipferry.android

import android.content.Context

/** 화면에서 바꾸는 설정. 비밀이 아니라 일반 SharedPreferences에 둔다. */
class Settings(context: Context) {
    private val prefs = context.getSharedPreferences("settings", Context.MODE_PRIVATE)

    /** 개발용 기본값: 에뮬레이터에서 본 PC의 로컬 릴레이. 실제 릴레이 주소는 화면에서 바꾼다(정식 주소는 O-03 이후). */
    var relayUrl: String
        get() = prefs.getString(KEY_RELAY, DEFAULT_RELAY) ?: DEFAULT_RELAY
        set(value) = prefs.edit().putString(KEY_RELAY, value).apply()

    var serviceEnabled: Boolean
        get() = prefs.getBoolean(KEY_SERVICE, true)
        set(value) = prefs.edit().putBoolean(KEY_SERVICE, value).apply()

    private companion object {
        const val KEY_RELAY = "relay_url"
        const val KEY_SERVICE = "service_enabled"
        const val DEFAULT_RELAY = "ws://10.0.2.2:8787"
    }
}
