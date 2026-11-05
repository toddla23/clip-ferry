import UIKit
import UniformTypeIdentifiers

/// 공유 시트의 clip-ferry (F24). 텍스트·URL·이미지 하나를 받아 그룹의 다른 기기로 보내고 닫힌다.
///
/// 확장은 메모리가 적으므로(약 120 MB) 이미지를 줄이지 않고 20 MiB 이하만 보낸다.
/// 사용자가 직접 고른 내용이라 민감 표시는 따지지 않는다(Android F19와 같다).
final class ShareViewController: UIViewController {
    private let label = UILabel()
    private let spinner = UIActivityIndicatorView(style: .large)
    private var started = false

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = UIColor.systemBackground.withAlphaComponent(0.95)
        label.text = "보내는 중…"
        label.numberOfLines = 0
        label.textAlignment = .center
        label.font = .preferredFont(forTextStyle: .body)
        let stack = UIStackView(arrangedSubviews: [spinner, label])
        stack.axis = .vertical
        stack.spacing = 16
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            stack.centerYAnchor.constraint(equalTo: view.centerYAnchor),
            stack.leadingAnchor.constraint(greaterThanOrEqualTo: view.leadingAnchor, constant: 24),
        ])
        spinner.startAnimating()
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        guard !started else { return }
        started = true
        Task {
            let line: String
            switch await loadItem() {
            case let .success(item):
                line = await Task.detached { Sender.send([item]) }.value
            case let .failure(.message(m)):
                line = m
            }
            spinner.stopAnimating()
            label.text = line
            try? await Task.sleep(for: .seconds(1.2))
            extensionContext?.completeRequest(returningItems: nil)
        }
    }

    /// 첫 첨부에서 이미지 → URL → 텍스트 순으로 꺼낸다.
    private func loadItem() async -> Result<Outgoing, SendFailure> {
        let providers = (extensionContext?.inputItems as? [NSExtensionItem] ?? [])
            .flatMap { $0.attachments ?? [] }
        for p in providers where p.hasItemConformingToTypeIdentifier(UTType.image.identifier) {
            if let data = await loadImageData(p) {
                guard let ready = Sender.prepareImage(data) else {
                    return .failure(.message("이미지가 20 MiB를 넘거나 읽을 수 없습니다"))
                }
                return .success(.image(ready))
            }
        }
        for p in providers where p.hasItemConformingToTypeIdentifier(UTType.url.identifier) {
            if let url = try? await p.loadItem(forTypeIdentifier: UTType.url.identifier) as? URL {
                return .success(.text(url.absoluteString))
            }
        }
        for p in providers where p.hasItemConformingToTypeIdentifier(UTType.plainText.identifier) {
            if let text = try? await p.loadItem(forTypeIdentifier: UTType.plainText.identifier) as? String,
               !text.isEmpty {
                return .success(.text(text))
            }
        }
        return .failure(.message("보낼 수 없는 형식입니다 (텍스트·URL·이미지만)"))
    }

    private func loadImageData(_ p: NSItemProvider) async -> Data? {
        // 파일 그대로 받는 것이 가장 가볍다. 안 되면 데이터, 마지막으로 UIImage.
        if let data = try? await p.cfLoadData(UTType.image.identifier) {
            return data
        }
        let item = try? await p.loadItem(forTypeIdentifier: UTType.image.identifier)
        if let url = item as? URL {
            return try? Data(contentsOf: url)
        }
        if let data = item as? Data {
            return data
        }
        if let image = item as? UIImage {
            return image.pngData()
        }
        return nil
    }
}

private extension NSItemProvider {
    func cfLoadData(_ typeIdentifier: String) async throws -> Data {
        try await withCheckedThrowingContinuation { cont in
            _ = loadDataRepresentation(forTypeIdentifier: typeIdentifier) { data, error in
                if let data {
                    cont.resume(returning: data)
                } else {
                    cont.resume(throwing: error ?? CocoaError(.fileReadUnknown))
                }
            }
        }
    }
}
