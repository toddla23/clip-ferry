package app.clipferry.android

import android.content.Context

/** 화면에서 바꾸는 설정. 비밀이 아니라 일반 SharedPreferences에 둔다. */
class Settings(context: Context) {
    private val prefs = context.getSharedPreferences("settings", Context.MODE_PRIVATE)

    /** 릴레이 주소. 사용자가 직접 넣지 않았으면([relayCustom]) 계정 서버가 알려 주는 주소로 바뀐다 (F43). */
    var relayUrl: String
        get() = prefs.getString(KEY_RELAY, DEFAULT_RELAY) ?: DEFAULT_RELAY
        set(value) = prefs.edit().putString(KEY_RELAY, value).apply()

    /** 계정 서버 주소 (F35). 로그아웃 상태에서만 바꾼다. 기본값은 빌드 종류마다 다르다 (F43). */
    var accountUrl: String
        get() = prefs.getString(KEY_ACCOUNT, DEFAULT_ACCOUNT) ?: DEFAULT_ACCOUNT
        set(value) = prefs.edit().putString(KEY_ACCOUNT, value).apply()

    /** 사용자가 릴레이 주소를 직접 넣었다 (F43). 아니면 계정 서버가 알려 주는 주소를 따른다. */
    var relayCustom: Boolean
        get() = prefs.getBoolean(KEY_RELAY_CUSTOM, false)
        set(value) = prefs.edit().putBoolean(KEY_RELAY_CUSTOM, value).apply()

    /** 배포 빌드에서도 서버 주소 칸을 보인다 (제목을 7번 눌러 켠다, F43). */
    var showServer: Boolean
        get() = prefs.getBoolean(KEY_SHOW_SERVER, false)
        set(value) = prefs.edit().putBoolean(KEY_SHOW_SERVER, value).apply()

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
        const val DEFAULT_RELAY = BuildConfig.DEFAULT_RELAY_URL
        const val KEY_ACCOUNT = "account_url"
        const val DEFAULT_ACCOUNT = BuildConfig.DEFAULT_ACCOUNT_URL
        const val KEY_RELAY_CUSTOM = "relay_custom"
        const val KEY_SHOW_SERVER = "show_server"
    }
}
