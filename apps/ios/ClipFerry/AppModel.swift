import Foundation
import SwiftUI
import UIKit
import UserNotifications

/// 코어(Rust)와 화면 사이의 얇은 층. 암호·프로토콜 로직은 없다 (Android `CoreController`와 같은 역할).
@MainActor
final class AppModel: ObservableObject {
    static let shared = AppModel()

    enum Phase: Equatable {
        case loading
        case needsName
        case noGroup
        case ready
        case failed(String)
    }

    enum Pairing: Equatable {
        case idle
        /// (초대) QR·코드를 보여 주는 중
        case inviting(code: String, qr: String)
        /// (초대) 참여 요청이 왔다. 숫자를 비교하고 승인·거절
        case inviteRequest(peer: String, sas: String)
        /// (참여) 릴레이에 붙어 초대한 기기를 기다린다
        case joining
        /// (참여) 숫자를 비교하고 상대가 승인하길 기다린다
        case joinWaiting(peer: String, sas: String)
        /// 끝났다(성공·실패 문장)
        case finished(String)
    }

    @Published private(set) var phase: Phase = .loading
    @Published private(set) var status: StatusInfo?
    @Published private(set) var devices: [DeviceEntry] = []
    @Published private(set) var link: LinkStatus = .disconnected
    @Published private(set) var onlineCount: UInt32 = 0
    @Published private(set) var pairing: Pairing = .idle
    /// 화면 아래에 잠깐 보여 줄 한 줄 (받음·보냄·오류)
    @Published private(set) var notice: String?
    @Published private(set) var lastReceived: String?
    @Published var relayUrl: String = Settings.relayUrl
    @Published var notificationPreview: Bool = Settings.notificationPreview {
        didSet { Settings.notificationPreview = notificationPreview }
    }

    private var client: ClipFerryClient?
    private var askedNotifications = false
    /// 재전송을 요청하고 기다리는 중 (요청 번호). 받거나 시간이 지나면 지운다.
    private var awaitingResend: Int?
    private var resendCounter = 0

    private init() {
        load()
    }

    // MARK: - 열기 · 첫 실행

    func load() {
        do {
            let c = try CoreHub.shared()
            client = c
            refresh()
        } catch {
            phase = .failed(describe(error))
        }
    }

    private func refresh() {
        guard let client else { return }
        if !client.isInitialized() {
            phase = .needsName
            return
        }
        status = try? client.status()
        devices = client.devices()
        if status?.health == .removed || !client.hasGroup() {
            phase = .noGroup
        } else {
            phase = .ready
        }
    }

    func setName(_ name: String) {
        guard let client else { return }
        do {
            try client.initDevice(deviceName: name)
            refresh()
        } catch {
            note(describe(error))
        }
    }

    func createGroup() {
        guard let client else { return }
        do {
            try client.createGroup()
            refresh()
            startSession()
        } catch {
            note(describe(error))
        }
    }

    func leaveGroup() {
        guard let client else { return }
        do {
            try client.leaveGroup()
            link = .disconnected
            onlineCount = 0
            refresh()
        } catch {
            note(describe(error))
        }
    }

    func saveRelay(_ url: String) {
        let trimmed = url.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix("ws://") || trimmed.hasPrefix("wss://") else {
            note("릴레이 주소는 ws:// 또는 wss://로 시작해야 합니다")
            return
        }
        Settings.relayUrl = trimmed
        relayUrl = trimmed
        if phase == .ready {
            client?.stop()
            startSession()
        }
        note("릴레이 주소를 저장했습니다")
    }

    // MARK: - 연결

    func becameActive() {
        refresh()
        guard phase == .ready else { return }
        startSession()
        askNotificationsOnce()
    }

    func wentBackground() {
        // 화면을 벗어나면 iOS가 곧 앱을 멈춘다. 연결을 닫고 그동안은 푸시로 받는다.
        client?.stop()
        link = .disconnected
        onlineCount = 0
    }

    private func startSession() {
        guard let client, client.hasGroup(), !client.isRunning() else { return }
        do {
            try client.start(relayUrl: Settings.relayUrl, events: EventsBridge())
        } catch {
            note(describe(error))
        }
    }

    // MARK: - 이벤트 (코어 스레드 → 메인)

    fileprivate func onStatus(_ s: LinkStatus) {
        link = s
        if s == .connected {
            syncPushToken()
        } else {
            onlineCount = 0
        }
    }

    fileprivate func onOnline(_ n: UInt32) {
        onlineCount = n
        devices = client?.devices() ?? []
    }

    fileprivate func onChanged() {
        refresh()
    }

    fileprivate func onText(from: String, text: String) {
        // 앱이 화면에 있으므로 클립보드에 쓸 수 있다.
        Received.put(text: text)
        receivedSomething("\(from)에서 텍스트를 받았습니다 — 붙여넣기 할 수 있습니다")
    }

    fileprivate func onImage(from: String, mime: String, bytes: Data) {
        Received.put(image: bytes, mime: mime)
        receivedSomething("\(from)에서 이미지를 받았습니다 — 붙여넣기 할 수 있습니다")
    }

    private func receivedSomething(_ line: String) {
        awaitingResend = nil
        lastReceived = line
        note(line)
    }

    // MARK: - 푸시 (F22)

    private func askNotificationsOnce() {
        guard !askedNotifications else { return }
        askedNotifications = true
        Task {
            let granted = (try? await UNUserNotificationCenter.current()
                .requestAuthorization(options: [.alert, .sound])) ?? false
            if granted {
                UIApplication.shared.registerForRemoteNotifications()
            }
        }
    }

    func pushTokenArrived(_ hex: String) {
        Settings.apnsToken = hex
        syncPushToken()
    }

    /// 토큰을 로스터에 적는다. 같으면 코어가 아무것도 보내지 않는다.
    private func syncPushToken() {
        guard let client, link == .connected, let token = Settings.apnsToken else { return }
        Task.detached {
            _ = try? client.setPushToken(platform: .apns, token: token)
        }
    }

    // MARK: - 알림 탭 (F22·F25)

    func handleNotificationTap(_ info: [AnyHashable: Any]) {
        let kind = info[PushKeys.kind] as? String
        switch kind {
        case PushKeys.kindText:
            if let text = info[PushKeys.text] as? String {
                Received.put(text: text)
                let from = info[PushKeys.fromName] as? String ?? "다른 기기"
                receivedSomething("\(from)에서 받은 텍스트를 클립보드에 넣었습니다 — 붙여넣기 할 수 있습니다")
            }
        case PushKeys.kindImage, PushKeys.kindNeedOpen:
            guard let from = info[PushKeys.fromId] as? String else {
                note("보낸 기기를 알 수 없습니다")
                return
            }
            let msg = (info[PushKeys.msgId] as? String).flatMap { Data(base64Encoded: $0) } ?? Data()
            let name = info[PushKeys.fromName] as? String ?? "보낸 기기"
            requestResend(fromId: from, fromName: name, msgId: msg)
        default:
            break
        }
    }

    /// 보낸 기기에 다시 보내 달라고 한다. 받은 것은 평소처럼 `onText`·`onImage`로 온다.
    private func requestResend(fromId: String, fromName: String, msgId: Data) {
        refresh()
        guard phase == .ready, let client else { return }
        startSession()
        resendCounter += 1
        let ticket = resendCounter
        awaitingResend = ticket
        note("\(fromName)에 다시 보내 달라고 요청하는 중…")
        Task.detached {
            let result: String?
            if !client.waitConnected(timeoutMs: 8_000) {
                result = "릴레이에 연결되지 않았습니다"
            } else {
                do {
                    let delivered = try client.requestResend(fromId: fromId, msgId: msgId)
                    result = delivered == 0 ? "보낸 기기(\(fromName))가 오프라인입니다" : nil
                } catch {
                    result = describe(error)
                }
            }
            await MainActor.run {
                guard AppModel.shared.awaitingResend == ticket else { return }
                if let result {
                    AppModel.shared.awaitingResend = nil
                    AppModel.shared.note(result)
                }
            }
            // 보낸 기기는 10분 동안만 들고 있다. 그 사이에 다른 것을 복사했으면 오지 않는다.
            try? await Task.sleep(for: .seconds(20))
            await MainActor.run {
                guard AppModel.shared.awaitingResend == ticket else { return }
                AppModel.shared.awaitingResend = nil
                AppModel.shared.note("보낸 기기에 더 이상 남아 있지 않습니다 (10분이 지났거나 그 뒤에 다른 것을 복사함)")
            }
        }
    }

    // MARK: - 보내기 (앱 화면의 버튼)

    func sendClipboardNow() {
        switch Sender.fromPasteboard() {
        case let .failure(.message(m)):
            note(m)
        case let .success(item):
            Task.detached {
                let line = Sender.send([item])
                await MainActor.run { AppModel.shared.note(line) }
            }
        }
    }

    // MARK: - 페어링 (F21)

    func startInvite() {
        guard let client else { return }
        do {
            let info = try client.startInvite(relayUrl: Settings.relayUrl, listener: PairBridge())
            pairing = .inviting(code: info.textCode, qr: info.qrString)
        } catch {
            pairing = .finished(describe(error))
        }
    }

    func decideInvite(_ approve: Bool) {
        client?.decideInvite(approve: approve)
    }

    func startJoin(code: String) {
        guard let client else { return }
        let trimmed = code.trimmingCharacters(in: .whitespacesAndNewlines)
        do {
            let relay = try client.startJoin(code: trimmed, defaultRelay: Settings.relayUrl, listener: PairBridge())
            // QR에 든 릴레이를 이 기기의 릴레이로 쓴다.
            Settings.relayUrl = relay
            relayUrl = relay
            pairing = .joining
        } catch {
            pairing = .finished(describe(error))
        }
    }

    func cancelPairing() {
        client?.cancelPairing()
        pairing = .idle
    }

    fileprivate func onPair(_ e: PairEvent) {
        switch e {
        case .connected:
            pairing = .joining
        case let .request(peer, sas):
            pairing = .inviteRequest(peer: peer, sas: sas)
        case let .waitingApproval(peer, sas):
            pairing = .joinWaiting(peer: peer, sas: sas)
        case let .approved(peer, delivered):
            pairing = .finished(delivered ? "\(peer)을(를) 그룹에 넣었습니다" : "\(peer)을(를) 넣었지만 상대가 응답을 못 받았을 수 있습니다")
            refresh()
        case .joined:
            pairing = .finished("그룹에 참여했습니다")
            refresh()
            startSession()
            askNotificationsOnce()
        case .rejected:
            pairing = .finished("거절했습니다")
        case let .ended(message):
            pairing = .finished(message)
        }
    }

    // MARK: - 알림 줄

    func note(_ line: String) {
        notice = line
        let shown = line
        Task {
            try? await Task.sleep(for: .seconds(4))
            if notice == shown { notice = nil }
        }
    }
}

/// 코어 콜백을 메인 스레드의 `AppModel`로 옮긴다.
final class EventsBridge: CoreEvents, @unchecked Sendable {
    func onStatus(status: LinkStatus) {
        Task { @MainActor in AppModel.shared.onStatus(status) }
    }

    func onOnlineCount(count: UInt32) {
        Task { @MainActor in AppModel.shared.onOnline(count) }
    }

    func onChanged() {
        Task { @MainActor in AppModel.shared.onChanged() }
    }

    func onText(fromName: String, text: String) {
        Task { @MainActor in AppModel.shared.onText(from: fromName, text: text) }
    }

    func onImage(fromName: String, mime: String, bytes: Data) {
        Task { @MainActor in AppModel.shared.onImage(from: fromName, mime: mime, bytes: bytes) }
    }
}

final class PairBridge: PairListener, @unchecked Sendable {
    func onPairEvent(event: PairEvent) {
        Task { @MainActor in AppModel.shared.onPair(event) }
    }
}
