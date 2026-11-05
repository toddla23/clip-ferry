import AppIntents

/// 단축어 앱에 바로 보이는 동작 (F23). 설치하면 자동으로 등록된다.
struct ClipFerryShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(
            intent: SendClipboardIntent(),
            phrases: [
                "\(.applicationName)로 클립보드 보내기",
                "\(.applicationName) 보내기",
            ],
            shortTitle: "클립보드 보내기",
            systemImageName: "paperplane"
        )
    }
}
