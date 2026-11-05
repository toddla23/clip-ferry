import Foundation

/// 이 프로세스(앱 또는 확장)에서 하나만 여는 코어 핸들.
///
/// 앱과 확장은 각자 프로세스라 각자 연다. 같은 `state.bin`과 키체인을 보므로 같은 기기다.
enum CoreHub {
    private static let lock = NSLock()
    nonisolated(unsafe) private static var client: ClipFerryClient?

    /// 코어를 연다(이미 열었으면 그것을 준다).
    static func shared() throws -> ClipFerryClient {
        lock.lock()
        defer { lock.unlock() }
        if let client {
            return client
        }
        let opened = try ClipFerryClient.open(
            dataDir: AppGroup.coreDirectory.path,
            vault: KeychainVault()
        )
        client = opened
        return opened
    }
}

/// 사용자에게 보여 줄 오류 문장.
func describe(_ error: Error) -> String {
    if let e = error as? FfiError {
        switch e {
        case .Removed:
            return "이 기기는 그룹에서 제거되었습니다. 다시 페어링하세요"
        case let .Failed(reason):
            return reason
        }
    }
    return error.localizedDescription
}
