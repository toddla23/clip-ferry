import SwiftUI
import VisionKit

/// 참여: QR 스캔(카메라) 또는 코드 입력 → 숫자 비교 → 상대 승인.
struct JoinView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var code = ""
    @State private var scanning = false

    var body: some View {
        Form {
            switch model.pairing {
            case .joining:
                Section { ProgressView("초대한 기기의 응답을 기다리는 중…") }
                cancelSection
            case let .joinWaiting(peer, sas):
                Section("숫자 비교") {
                    Text(sas).font(.system(size: 40, weight: .bold, design: .monospaced))
                        .frame(maxWidth: .infinity)
                    Text("\(peer) 화면에도 같은 숫자가 떠 있는지 확인하세요. 같으면 그쪽에서 승인합니다.")
                        .font(.footnote).foregroundStyle(.secondary)
                }
                cancelSection
            case let .finished(message):
                Section {
                    Text(message)
                    Button("닫기") {
                        model.cancelPairing()
                        dismiss()
                    }
                }
            default:
                if DataScannerViewController.isSupported {
                    Section("QR 스캔") {
                        Button(scanning ? "스캔 닫기" : "카메라로 QR 스캔") { scanning.toggle() }
                        if scanning {
                            QRScanner { found in
                                scanning = false
                                model.startJoin(code: found)
                            }
                            .frame(height: 300)
                            .clipShape(RoundedRectangle(cornerRadius: 12))
                        }
                    }
                }
                Section("코드 입력") {
                    TextField("PC 화면의 코드", text: $code, axis: .vertical)
                        .textInputAutocapitalization(.characters)
                        .autocorrectionDisabled()
                    Button("참여") { model.startJoin(code: code) }
                        .disabled(code.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
        }
        .navigationTitle("그룹에 참여")
        .onDisappear {
            if case .finished = model.pairing { model.cancelPairing() }
        }
    }

    private var cancelSection: some View {
        Section { Button("취소", role: .cancel) { model.cancelPairing() } }
    }
}

/// 초대: QR·코드를 보여 주고, 요청이 오면 숫자를 비교해 승인한다.
struct InviteView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        Form {
            switch model.pairing {
            case let .inviting(code, qr):
                Section("다른 기기에서 이 QR을 스캔") {
                    if let image = QRImage.make(qr) {
                        Image(uiImage: image)
                            .interpolation(.none)
                            .resizable()
                            .scaledToFit()
                            .frame(maxWidth: 260)
                            .frame(maxWidth: .infinity)
                    }
                }
                Section("또는 코드 입력") {
                    Text(code).font(.title3.monospaced()).textSelection(.enabled)
                }
                Section { Button("취소", role: .cancel) { model.cancelPairing() } }
            case let .inviteRequest(peer, sas):
                Section("\(peer)이(가) 참여하려고 합니다") {
                    Text(sas).font(.system(size: 40, weight: .bold, design: .monospaced))
                        .frame(maxWidth: .infinity)
                    Text("상대 화면의 숫자와 같을 때만 승인하세요.")
                        .font(.footnote).foregroundStyle(.secondary)
                    Button("승인") { model.decideInvite(true) }
                    Button("거절", role: .destructive) { model.decideInvite(false) }
                }
            case let .finished(message):
                Section {
                    Text(message)
                    Button("닫기") {
                        model.cancelPairing()
                        dismiss()
                    }
                }
            default:
                Section { ProgressView() }
            }
        }
        .navigationTitle("기기 추가")
        .onAppear { model.startInvite() }
        .onDisappear { model.cancelPairing() }
    }
}

/// VisionKit 실시간 QR 인식 (기기 안에서만, 사진은 저장하지 않음). 시뮬레이터에서는 지원되지 않아 코드 입력을 쓴다.
struct QRScanner: UIViewControllerRepresentable {
    let onFound: (String) -> Void

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let vc = DataScannerViewController(
            recognizedDataTypes: [.barcode(symbologies: [.qr])],
            qualityLevel: .balanced,
            isHighlightingEnabled: true
        )
        vc.delegate = context.coordinator
        try? vc.startScanning()
        return vc
    }

    func updateUIViewController(_ vc: DataScannerViewController, context: Context) {}

    static func dismantleUIViewController(_ vc: DataScannerViewController, coordinator: Coordinator) {
        vc.stopScanning()
    }

    func makeCoordinator() -> Coordinator { Coordinator(onFound: onFound) }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        let onFound: (String) -> Void
        private var done = false

        init(onFound: @escaping (String) -> Void) { self.onFound = onFound }

        func dataScanner(_ scanner: DataScannerViewController, didAdd items: [RecognizedItem], allItems: [RecognizedItem]) {
            guard !done else { return }
            for item in items {
                if case let .barcode(code) = item, let value = code.payloadStringValue {
                    done = true
                    scanner.stopScanning()
                    onFound(value)
                    return
                }
            }
        }
    }
}

/// 코어가 만든 QR 칸을 그림으로 그린다 (데스크톱·Android와 같은 `qrcode` 크레이트).
enum QRImage {
    static func make(_ data: String, scale: Int = 8) -> UIImage? {
        guard let m = qrMatrix(data: data) else { return nil }
        let width = Int(m.width)
        let quiet = 4
        let size = (width + quiet * 2) * scale
        let renderer = UIGraphicsImageRenderer(size: CGSize(width: size, height: size))
        return renderer.image { ctx in
            UIColor.white.setFill()
            ctx.fill(CGRect(x: 0, y: 0, width: size, height: size))
            UIColor.black.setFill()
            for y in 0..<width {
                for x in 0..<width where m.dark[y * width + x] {
                    ctx.fill(CGRect(x: (x + quiet) * scale, y: (y + quiet) * scale, width: scale, height: scale))
                }
            }
        }
    }
}
