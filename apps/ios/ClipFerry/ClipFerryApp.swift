import SwiftUI
import UIKit
import UserNotifications

@main
struct ClipFerryApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @Environment(\.scenePhase) private var phase
    @StateObject private var model = AppModel.shared

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(model)
        }
        .onChange(of: phase) { _, now in
            // iOS는 화면을 벗어난 앱을 곧 멈춘다. 화면에 있을 때만 붙어 있고, 꺼져 있을 땐 푸시로 받는다(F22).
            switch now {
            case .active: model.becameActive()
            case .background: model.wentBackground()
            default: break
            }
        }
    }
}

/// 푸시 등록과 알림 탭을 받는다.
final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        UNUserNotificationCenter.current().delegate = self
        return true
    }

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        let hex = deviceToken.map { String(format: "%02x", $0) }.joined()
        Task { @MainActor in AppModel.shared.pushTokenArrived(hex) }
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        Task { @MainActor in AppModel.shared.note("푸시 등록 실패: \(error.localizedDescription)") }
    }

    /// 앱이 화면에 있을 때 온 알림도 배너로 보여 준다.
    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        [.banner, .sound]
    }

    /// 알림을 눌렀다: 텍스트는 바로 클립보드에, 이미지·긴 텍스트는 보낸 기기에 재전송을 요청한다(F25, D-016).
    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse
    ) async {
        let info = response.notification.request.content.userInfo
        await MainActor.run { AppModel.shared.handleNotificationTap(info) }
    }
}
