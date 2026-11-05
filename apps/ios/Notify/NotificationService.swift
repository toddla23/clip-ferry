import Foundation
import UserNotifications

/// Notification Service Extension (F22). 릴레이가 보낸 푸시의 봉투를 이 기기에서 열어 알림 문구를 바꾼다.
///
/// - 텍스트: 보낸 기기 이름을 제목으로, 내용을 본문으로(설정에서 끄면 "텍스트가 도착했습니다"만).
///   누르면 앱이 클립보드에 넣는다(내용은 알림의 `cf_text`로 넘긴다).
/// - 이미지·긴 텍스트: "눌러서 받기". 누르면 앱이 보낸 기기에 재전송을 요청한다(F25).
/// - 열 수 없으면(다른 그룹·위조·오래됨) 일반 문구 그대로 둔다. 크래시하지 않는다.
///
/// 메모리 한도는 24 MB다. 코어는 봉투 하나만 열고, 런타임 스레드 두 개 외에는 쓰지 않는다.
final class NotificationService: UNNotificationServiceExtension {
    private var handler: ((UNNotificationContent) -> Void)?
    private var best: UNMutableNotificationContent?

    override func didReceive(
        _ request: UNNotificationRequest,
        withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void
    ) {
        handler = contentHandler
        guard let content = request.content.mutableCopy() as? UNMutableNotificationContent else {
            contentHandler(request.content)
            return
        }
        best = content
        content.title = "clip-ferry"
        content.body = "새 클립이 도착했습니다"
        content.userInfo[PushKeys.kind] = PushKeys.kindUnreadable

        let info = request.content.userInfo
        if let b64 = info[PushKeys.payload] as? String, let payload = Data(base64Encoded: b64) {
            fill(content, opening: payload)
        } else if info[PushKeys.needOpen] as? Bool == true {
            fillNeedOpen(content, tag: (info[PushKeys.fromTag] as? String).flatMap { Data(base64Encoded: $0) })
        }
        contentHandler(content)
        handler = nil
    }

    private func fill(_ content: UNMutableNotificationContent, opening payload: Data) {
        guard let client = try? CoreHub.shared() else { return }
        switch client.openPush(payload: payload) {
        case let .text(fromName, fromId, text, msgId):
            content.title = fromName.isEmpty ? "clip-ferry" : fromName
            content.body = Settings.notificationPreview
                ? String(text.prefix(300))
                : "텍스트가 도착했습니다 — 눌러서 붙여넣기"
            content.userInfo[PushKeys.kind] = PushKeys.kindText
            content.userInfo[PushKeys.text] = text
            content.userInfo[PushKeys.fromName] = fromName
            content.userInfo[PushKeys.fromId] = fromId
            content.userInfo[PushKeys.msgId] = msgId.base64EncodedString()
        case let .image(fromName, fromId, msgId):
            content.title = fromName.isEmpty ? "clip-ferry" : fromName
            content.body = "이미지가 도착했습니다 — 눌러서 받기"
            content.userInfo[PushKeys.kind] = PushKeys.kindImage
            content.userInfo[PushKeys.fromName] = fromName
            content.userInfo[PushKeys.fromId] = fromId
            content.userInfo[PushKeys.msgId] = msgId.base64EncodedString()
        case .unreadable:
            break
        }
        // 봉투는 열었으니 알림에 남기지 않는다.
        content.userInfo[PushKeys.payload] = nil
    }

    /// 봉투가 너무 커서(긴 텍스트) 도착 알림만 온 경우.
    private func fillNeedOpen(_ content: UNMutableNotificationContent, tag: Data?) {
        guard let tag, let client = try? CoreHub.shared(), let device = client.deviceForTag(tag: tag) else {
            content.body = "긴 내용이 도착했습니다 — 앱을 열어 주세요"
            return
        }
        content.title = device.name
        content.body = "긴 내용이 도착했습니다 — 눌러서 받기"
        content.userInfo[PushKeys.kind] = PushKeys.kindNeedOpen
        content.userInfo[PushKeys.fromName] = device.name
        content.userInfo[PushKeys.fromId] = device.idHex
    }

    /// 시간이 다 되면(약 30초) 지금까지 만든 것을 보낸다.
    override func serviceExtensionTimeWillExpire() {
        if let handler, let best {
            handler(best)
        }
        handler = nil
    }
}
