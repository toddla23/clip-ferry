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

    /** 받은 내용을 클립보드에 넣을 때 알림을 띄운다. 넣지 못했을 때는 이 설정과 상관없이 알린다. */
    var notifyOnReceive: Boolean
        get() = prefs.getBoolean(KEY_NOTIFY_RECEIVE, true)
        set(value) = prefs.edit().putBoolean(KEY_NOTIFY_RECEIVE, value).apply()

    /**
     * 민감한 내용으로 표시된 복사(비밀번호 관리자 등)도 보낸다. 데스크톱 `sync_sensitive`와 같은 뜻이며
     * 기본은 보낸다(종단간 암호화, 2026-10-31 사용자 결정). 끄면 그런 복사는 보내지 않는다.
     */
    var syncSensitive: Boolean
        get() = prefs.getBoolean(KEY_SYNC_SENSITIVE, true)
        set(value) = prefs.edit().putBoolean(KEY_SYNC_SENSITIVE, value).apply()

    private companion object {
        const val KEY_SYNC_SENSITIVE = "sync_sensitive"
        const val KEY_RELAY = "relay_url"
        const val KEY_SERVICE = "service_enabled"
        const val KEY_NOTIFY_RECEIVE = "notify_on_receive"
        const val DEFAULT_RELAY = "ws://10.0.2.2:8787"
    }
}
