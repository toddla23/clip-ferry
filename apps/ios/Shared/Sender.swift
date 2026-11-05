import Foundation
import UIKit
import UniformTypeIdentifiers

/// 보낼 것.
enum Outgoing {
    case text(String)
    case image(Data)
}

/// 한 번 보내고 끝내는 전송 (App Intents·공유 확장·앱 버튼이 쓴다).
///
/// 이미 연결된 세션이 있으면 그것으로 보내고, 없으면 잠깐 연결해서 보낸 뒤 닫는다.
/// 막는 호출이므로 메인 스레드가 아닌 곳에서 부른다. 결과는 사용자에게 보여 줄 한 줄이다.
enum Sender {
    static let maxImageBytes = 20 * 1024 * 1024
    private static let connectTimeoutMs: UInt64 = 8_000

    static func send(_ items: [Outgoing]) -> String {
        guard !items.isEmpty else { return "보낼 것이 없습니다" }
        let client: ClipFerryClient
        do {
            client = try CoreHub.shared()
        } catch {
            return describe(error)
        }
        guard client.hasGroup() else { return "먼저 앱에서 그룹을 만들거나 참여하세요" }

        let startedHere = !client.isRunning()
        if startedHere {
            do {
                try client.start(relayUrl: Settings.relayUrl, events: SilentEvents())
            } catch {
                return describe(error)
            }
        }
        defer {
            if startedHere { client.stop() }
        }
        guard client.waitConnected(timeoutMs: connectTimeoutMs) else {
            return "릴레이에 연결되지 않아 보내지 못했습니다"
        }

        var sent = 0
        var delivered: UInt32 = 0
        var lastError: String?
        for item in items {
            do {
                switch item {
                case let .text(t):
                    delivered = try client.sendText(text: t)
                case let .image(d):
                    delivered = try client.sendImage(bytes: d)
                }
                sent += 1
            } catch {
                lastError = describe(error)
            }
        }
        let isText = items.allSatisfy { if case .text = $0 { true } else { false } }
        let what = isText ? "텍스트를" : (items.count == 1 ? "이미지를" : "이미지 \(sent)장을")
        if sent == 0 { return lastError ?? "보내지 못했습니다" }
        if sent < items.count { return "\(items.count)개 중 \(sent)개만 보냈습니다 (\(lastError ?? ""))" }
        if delivered == 0 { return "보냈지만 지금 연결된 다른 기기가 없습니다" }
        return "\(what) 다른 기기 \(delivered)대로 보냈습니다"
    }

    /// 이미지 데이터를 보낼 수 있는 형태로 맞춘다. PNG·JPEG·GIF·WebP는 그대로, 그 밖(HEIC 등)은 JPEG로.
    /// 20 MiB를 넘으면 `nil`. 크기는 줄이지 않는다.
    static func prepareImage(_ data: Data) -> Data? {
        let bytes = [UInt8](data.prefix(12))
        func at(_ i: Int) -> UInt8? { i < bytes.count ? bytes[i] : nil }
        let png = at(0) == 0x89 && at(1) == 0x50 && at(2) == 0x4E && at(3) == 0x47
        let jpeg = at(0) == 0xFF && at(1) == 0xD8
        let gif = at(0) == 0x47 && at(1) == 0x49 && at(2) == 0x46
        let webp = at(0) == 0x52 && at(1) == 0x49 && at(8) == 0x57 && at(9) == 0x45
        let out: Data?
        if png || jpeg || gif || webp {
            out = data
        } else {
            out = UIImage(data: data)?.jpegData(compressionQuality: 0.92)
        }
        guard let out, out.count <= maxImageBytes else { return nil }
        return out
    }

    /// 지금 클립보드에서 보낼 것을 꺼낸다. 다른 앱에서 복사한 것을 읽으면 iOS가 붙여넣기 허용을 묻는다
    /// (설정 → clip-ferry → 다른 앱에서 붙여넣기 → 허용 으로 묻지 않게 할 수 있다).
    static func fromPasteboard() -> Result<Outgoing, SendFailure> {
        let pb = UIPasteboard.general
        if Received.isOurs(pb) {
            return .failure(.message("다른 기기에서 받은 내용이라 다시 보내지 않았습니다"))
        }
        if pb.hasImages {
            for type in [UTType.png, .jpeg, .gif, .webP, .heic, .image] {
                if let data = pb.data(forPasteboardType: type.identifier) {
                    guard let ready = prepareImage(data) else {
                        return .failure(.message("이미지가 20 MiB를 넘거나 읽을 수 없습니다"))
                    }
                    return .success(.image(ready))
                }
            }
            if let image = pb.image, let data = image.pngData(), let ready = prepareImage(data) {
                return .success(.image(ready))
            }
        }
        if let text = pb.string, !text.isEmpty {
            return .success(.text(text))
        }
        if let url = pb.url {
            return .success(.text(url.absoluteString))
        }
        return .failure(.message("클립보드가 비어 있습니다"))
    }
}

enum SendFailure: Error {
    case message(String)
}

/// 다른 기기에서 받아 클립보드에 넣을 때 쓰는 표시. 되돌려 보내지 않으려고 쓴다.
enum Received {
    static let markerType = "app.clipferry.received"

    /// 받은 텍스트를 클립보드에 넣는다.
    static func put(text: String) {
        UIPasteboard.general.setItems([[
            UTType.utf8PlainText.identifier: text,
            markerType: Data([1]),
        ]])
    }

    /// 받은 이미지를 클립보드에 넣는다. `mime`은 코어가 머리말로 다시 확인한 형식이다.
    static func put(image: Data, mime: String) {
        let type = UTType(mimeType: mime) ?? .png
        UIPasteboard.general.setItems([[
            type.identifier: image,
            markerType: Data([1]),
        ]])
    }

    static func isOurs(_ pb: UIPasteboard) -> Bool {
        pb.contains(pasteboardTypes: [markerType])
    }
}

/// 한 번 보내기용 세션의 이벤트는 쓰지 않는다.
final class SilentEvents: CoreEvents, @unchecked Sendable {
    func onStatus(status: LinkStatus) {}
    func onOnlineCount(count: UInt32) {}
    func onChanged() {}
    func onText(fromName: String, text: String) {}
    func onImage(fromName: String, mime: String, bytes: Data) {}
}
