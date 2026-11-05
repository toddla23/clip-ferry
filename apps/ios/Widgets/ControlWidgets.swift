import AppIntents
import SwiftUI
import WidgetKit

/// 제어 센터 버튼 "클립보드 보내기" (F23, iOS 18+). 누르면 `SendClipboardIntent`를 실행한다.
@main
struct ClipFerryWidgets: WidgetBundle {
    var body: some Widget {
        SendClipboardControl()
    }
}

struct SendClipboardControl: ControlWidget {
    var body: some ControlWidgetConfiguration {
        StaticControlConfiguration(kind: "app.clipferry.ios.send") {
            ControlWidgetButton(action: SendClipboardIntent()) {
                Label("클립보드 보내기", systemImage: "paperplane")
            }
        }
        .displayName("클립보드 보내기")
        .description("지금 클립보드를 다른 기기로 보냅니다.")
    }
}
