import Foundation
import Security

/// 코어의 비밀 저장소(`SecretVault`)를 iOS 키체인으로 구현한다.
///
/// - 앱과 확장이 같이 쓰도록 키체인 접근 그룹(`<팀 ID>.app.clipferry.shared`)에 둔다. 그룹은
///   Info.plist의 `ClipFerryKeychainGroup`에서 읽는다(빌드할 때 팀 ID가 채워진다).
/// - 화면이 잠겨 있어도 알림 확장이 열 수 있도록 "처음 잠금 해제 뒤" 접근으로 둔다. 이 기기에서만 쓰고 백업하지 않는다.
/// - 서명 없이 만든 시뮬레이터 빌드처럼 접근 그룹을 쓸 수 없으면 그룹 없이 저장한다(앱 안에서만 보인다).
final class KeychainVault: SecretVault, @unchecked Sendable {
    private let service = "app.clipferry.core"
    private let group: String?

    init() {
        let value = Bundle.main.object(forInfoDictionaryKey: "ClipFerryKeychainGroup") as? String
        // 팀 ID 없이 빌드하면 접두어가 비어 `app.clipferry.shared`만 남는다. 그래도 그대로 시도한다.
        group = (value?.isEmpty == false && value?.contains("$(") == false) ? value : nil
    }

    private func query(_ key: String, withGroup: Bool) -> [String: Any] {
        var q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
        ]
        if withGroup, let group {
            q[kSecAttrAccessGroup as String] = group
        }
        return q
    }

    func get(key: String) -> Data? {
        for withGroup in [true, false] {
            var q = query(key, withGroup: withGroup)
            q[kSecReturnData as String] = true
            q[kSecMatchLimit as String] = kSecMatchLimitOne
            var out: CFTypeRef?
            if SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess, let data = out as? Data {
                return data
            }
        }
        return nil
    }

    func set(key: String, value: Data) throws {
        var status = write(key, value, withGroup: true)
        if status == errSecMissingEntitlement {
            status = write(key, value, withGroup: false)
        }
        guard status == errSecSuccess else {
            throw FfiError.Failed(reason: "키체인에 저장하지 못했습니다 (\(status))")
        }
    }

    private func write(_ key: String, _ value: Data, withGroup: Bool) -> OSStatus {
        let q = query(key, withGroup: withGroup)
        let attrs: [String: Any] = [
            kSecValueData as String: value,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]
        let status = SecItemUpdate(q as CFDictionary, attrs as CFDictionary)
        if status != errSecItemNotFound {
            return status
        }
        var add = q
        add.merge(attrs) { _, new in new }
        return SecItemAdd(add as CFDictionary, nil)
    }

    func delete(key: String) throws {
        for withGroup in [true, false] {
            let status = SecItemDelete(query(key, withGroup: withGroup) as CFDictionary)
            guard status == errSecSuccess || status == errSecItemNotFound || status == errSecMissingEntitlement else {
                throw FfiError.Failed(reason: "키체인에서 지우지 못했습니다 (\(status))")
            }
        }
    }
}
