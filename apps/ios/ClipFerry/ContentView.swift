import SwiftUI

struct ContentView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        NavigationStack {
            Group {
                switch model.phase {
                case .loading:
                    ProgressView()
                case .needsName:
                    NameView()
                case .noGroup:
                    NoGroupView()
                case .ready:
                    HomeView()
                case let .failed(message):
                    ContentUnavailableView("코어를 열 수 없습니다", systemImage: "exclamationmark.triangle", description: Text(message))
                }
            }
            .navigationTitle("clip-ferry")
        }
        .overlay(alignment: .bottom) {
            if let notice = model.notice {
                Text(notice)
                    .font(.callout)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 10)
                    .background(.thinMaterial, in: Capsule())
                    .padding(.bottom, 24)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            }
        }
        .animation(.default, value: model.notice)
    }
}

/// 첫 실행: 이 기기 이름.
private struct NameView: View {
    @EnvironmentObject private var model: AppModel
    @State private var name = UIDevice.current.name

    var body: some View {
        Form {
            Section("이 기기 이름") {
                TextField("예: 내 아이폰", text: $name)
                Button("다음") { model.setName(name) }
                    .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty)
            }
            Section {
                Text("다른 기기 목록에 보이는 이름입니다. 나중에 바꿀 수 있습니다.")
                    .font(.footnote).foregroundStyle(.secondary)
            }
        }
    }
}

/// 그룹이 없음: 다른 기기의 그룹에 참여하거나 새로 만든다.
private struct NoGroupView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        Form {
            if model.status?.health == .removed {
                Section {
                    Label("이 기기는 그룹에서 제거되었습니다. 다시 페어링하세요.", systemImage: "person.crop.circle.badge.xmark")
                    Button("그룹에서 나가기", role: .destructive) { model.leaveGroup() }
                }
            }
            Section("다른 기기의 그룹에 참여") {
                NavigationLink("QR 스캔 또는 코드 입력") { JoinView() }
                Text("PC 앱에서 '기기 추가'를 누르면 QR과 코드가 나옵니다.")
                    .font(.footnote).foregroundStyle(.secondary)
            }
            Section("처음 쓰는 경우") {
                Button("새 그룹 만들기") { model.createGroup() }
            }
            RelaySection()
        }
    }
}

/// 그룹이 있음: 연결 상태, 보내기, 기기, 설정.
private struct HomeView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        Form {
            Section {
                HStack {
                    Circle().fill(color).frame(width: 10, height: 10)
                    Text(linkText)
                    Spacer()
                    if let s = model.status {
                        Text(s.deviceName).foregroundStyle(.secondary)
                    }
                }
                if let last = model.lastReceived {
                    Text(last).font(.footnote).foregroundStyle(.secondary)
                }
            }
            Section("보내기") {
                Button("지금 클립보드 보내기") { model.sendClipboardNow() }
                Text("단축어 앱의 'clip-ferry로 클립보드 보내기'를 뒷면 탭(설정 → 손쉬운 사용 → 터치 → 뒷면 탭)이나 제어 센터에 연결하면 1번에 보냅니다. 공유 시트의 clip-ferry로도 보낼 수 있습니다.")
                    .font(.footnote).foregroundStyle(.secondary)
                Text("다른 앱에서 복사한 것을 읽을 때 '붙여넣기 허용'을 묻습니다. 설정 → clip-ferry → 다른 앱에서 붙여넣기 → 허용 으로 묻지 않게 할 수 있습니다.")
                    .font(.footnote).foregroundStyle(.secondary)
            }
            Section("기기 (\(model.devices.count))") {
                ForEach(model.devices, id: \.idHex) { d in
                    HStack {
                        Circle().fill(d.online ? .green : .gray.opacity(0.4)).frame(width: 8, height: 8)
                        Text(d.name)
                        if d.isSelf { Text("이 기기").font(.caption).foregroundStyle(.secondary) }
                        Spacer()
                        Text(d.idShort).font(.caption.monospaced()).foregroundStyle(.secondary)
                    }
                }
                NavigationLink("기기 추가 (초대)") { InviteView() }
            }
            Section("알림") {
                Toggle("알림에 받은 텍스트 미리보기", isOn: $model.notificationPreview)
                Text("앱이 꺼져 있을 때 PC에서 복사하면 알림이 옵니다. 알림을 누르면 클립보드에 들어갑니다. 이미지는 알림을 누르면 보낸 기기에서 다시 받아 옵니다(보낸 기기가 켜져 있어야 함).")
                    .font(.footnote).foregroundStyle(.secondary)
            }
            RelaySection()
            Section {
                Button("그룹에서 나가기", role: .destructive) { model.leaveGroup() }
            }
        }
    }

    private var color: Color {
        switch model.link {
        case .connected: .green
        case .reconnecting: .orange
        case .disconnected: .gray
        }
    }

    private var linkText: String {
        switch model.link {
        case .connected: "연결됨 · 온라인 \(model.onlineCount)대"
        case .reconnecting: "다시 연결하는 중…"
        case .disconnected: "연결 안 됨"
        }
    }
}

private struct RelaySection: View {
    @EnvironmentObject private var model: AppModel
    @State private var draft = ""

    var body: some View {
        Section("릴레이 주소") {
            TextField("wss://…", text: $draft)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .keyboardType(.URL)
            Button("저장") { model.saveRelay(draft) }
                .disabled(draft == model.relayUrl)
        }
        .onAppear { draft = model.relayUrl }
        .onChange(of: model.relayUrl) { _, new in draft = new }
    }
}
