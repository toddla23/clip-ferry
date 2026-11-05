import Foundation

/// 푸시 알림의 키.
///
/// - 릴레이가 보내는 것(03-protocol 5절): `p` 봉투(base64), `f` 보낸 기기 태그(base64), `need_open`
/// - 알림 확장이 연 뒤 앱에 넘기는 것: `cf_*`
enum PushKeys {
    static let payload = "p"
    static let fromTag = "f"
    static let needOpen = "need_open"

    static let kind = "cf_kind"
    static let text = "cf_text"
    static let fromName = "cf_from_name"
    static let fromId = "cf_from"
    static let msgId = "cf_msg"

    static let kindText = "text"
    static let kindImage = "image"
    static let kindNeedOpen = "need_open"
    static let kindUnreadable = "unreadable"
}
