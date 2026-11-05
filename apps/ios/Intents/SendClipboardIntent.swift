import AppIntents
import Foundation

/// "clip-ferry로 클립보드 보내기" (F23). 단축어·뒷면 탭·제어 센터 버튼이 부른다.
///
/// 앱을 열지 않고 클립보드를 읽어 보낸다. 다른 앱에서 복사한 것을 읽을 때 iOS가 붙여넣기 허용을 묻는다
/// (설정 → clip-ferry → 다른 앱에서 붙여넣기 → 허용).
struct SendClipboardIntent: AppIntent {
    static let title: LocalizedStringResource = "클립보드 보내기"
    static let description = IntentDescription("지금 클립보드의 텍스트나 이미지를 그룹의 다른 기기로 보냅니다.")
    static let openAppWhenRun = false

    func perform() async throws -> some IntentResult & ProvidesDialog {
        let line: String
        switch await MainActor.run(body: { Sender.fromPasteboard() }) {
        case let .failure(.message(m)):
            line = m
        case let .success(item):
            line = await Task.detached { Sender.send([item]) }.value
        }
        return .result(dialog: IntentDialog(stringLiteral: line))
    }
}
