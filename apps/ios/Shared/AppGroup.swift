import Foundation

/// 앱과 확장(알림·공유·위젯)이 같이 쓰는 저장소 위치와 설정.
///
/// - `state.bin`(암호화된 로스터·설정)은 App Group 컨테이너의 `core/`에 둔다.
/// - 비밀(기기 키·그룹 키)은 키체인 그룹에 둔다 (`KeychainVault`).
/// - 설정은 App Group의 `UserDefaults`에 둔다. 내용(클립)은 여기에 저장하지 않는다.
enum AppGroup {
    static let identifier = "group.app.clipferry"

    /// 코어 데이터 폴더. App Group을 쓸 수 없으면(서명 없이 빌드한 시뮬레이터 등) 앱 전용 폴더로 물러난다.
    static var coreDirectory: URL {
        let base = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: identifier)
            ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        let dir = base.appendingPathComponent("core", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    static var defaults: UserDefaults {
        UserDefaults(suiteName: identifier) ?? .standard
    }
}

/// 사용자 설정. 앱과 확장이 같이 읽는다.
enum Settings {
    private static let relayKey = "relayUrl"
    private static let previewKey = "notificationPreview"
    private static let pushTokenKey = "apnsToken"

    /// 개발 기본값. 실제 릴레이 주소는 앱 설정에서 넣는다 (`docs/07-release.md`).
    static let defaultRelay = "ws://127.0.0.1:8787"

    static var relayUrl: String {
        get { AppGroup.defaults.string(forKey: relayKey) ?? defaultRelay }
        set { AppGroup.defaults.set(newValue, forKey: relayKey) }
    }

    /// 알림에 받은 텍스트 미리보기를 보여 줄지 (기본: 보여 줌). 끄면 "텍스트가 도착했습니다"만 뜬다.
    static var notificationPreview: Bool {
        get { AppGroup.defaults.object(forKey: previewKey) as? Bool ?? true }
        set { AppGroup.defaults.set(newValue, forKey: previewKey) }
    }

    /// 마지막으로 받은 APNs 토큰(16진수). 연결될 때마다 로스터와 맞춘다.
    static var apnsToken: String? {
        get { AppGroup.defaults.string(forKey: pushTokenKey) }
        set { AppGroup.defaults.set(newValue, forKey: pushTokenKey) }
    }
}
