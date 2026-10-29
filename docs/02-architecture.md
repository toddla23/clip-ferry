# 02. 아키텍처

## 전체 구조

```
┌──────── 기기 A (예: PC) ────────┐        ┌──── 릴레이 (무저장) ────┐        ┌──────── 기기 B (예: 폰) ────────┐
│ 플랫폼 셸  클립보드·UI·OS 연동    │        │ 릴레이 노드 (WebSocket)  │        │ 플랫폼 셸                         │
│      │  "보내줘" / "받았어"        │        │   ├ 메시지 버스 (확장 시) │        │      ▲                            │
│      ▼                            │        │   └ 푸시 게이트웨이       │        │      │                            │
│ Rust 코어  암호화·프로토콜·연결   │──암호문─▶│  채널 ID로 라우팅만      │──암호문─▶│ Rust 코어                         │
└──────────────────────────────────┘        └─────────────────────────┘   (오프라인이면 APNs/FCM 푸시)
```

### 복사 한 번의 흐름

1. 기기 A에서 복사 → 셸이 감지해서 코어의 `send_clip(...)` 호출
2. 코어가 현재 에포크 키로 암호화·서명 → 릴레이에 `Publish`
3. 릴레이가 같은 채널을 구독 중인 다른 연결에 `Deliver` (받을 기기가 오프라인이고 푸시 토큰이 함께 왔으면 APNs/FCM으로 전송)
4. 기기 B 코어가 복호화·서명 검증·중복 확인 → 셸 콜백 `on_clip(...)`
5. 셸이 클립보드에 쓰기 (에코 방지 해시 기록)

## 구성 요소별 책임

### Rust 코어 (`core/`, 크레이트 `clip-ferry-core`)

| 모듈 | 책임 |
|---|---|
| `crypto` | 키 생성, AEAD 봉인/개봉, 기기 간 키 래핑, HKDF 파생, 비밀 타입(zeroize) |
| `envelope` | 페이로드 직렬화(protobuf), 서명, 봉투 생성/검증, 재전송 방지 |
| `group` | 그룹 생성, 로스터, 에포크, 키 회전, 밀린 기기 복구 |
| `pairing` | 초대 생성, 참여, SAS 계산, 승인 |
| `transport` | 릴레이 WebSocket 연결, 재연결(지수 백오프), 구독, 핑, 송수신 |
| `transfer` | 이미지 청크 분할/조립, 해시 검증, 타임아웃 |
| `store` | 로컬 상태 저장(암호화된 파일) + `SecretStore` 트레이트 |
| `api` | 셸이 쓰는 공개 API (UniFFI로 Kotlin/Swift 노출) |

코어 공개 API 스케치 (구체 시그니처는 구현 기능에서 확정):

```rust
pub struct ClipFerry { /* ... */ }

impl ClipFerry {
    pub fn open(config: Config, secrets: Box<dyn SecretStore>, events: Box<dyn EventSink>) -> Result<Self>;
    pub fn create_group(&self, device_name: String) -> Result<()>;
    pub fn create_invite(&self) -> Result<Invite>;            // QR 문자열 + 텍스트 코드
    pub fn join(&self, code: String, device_name: String) -> Result<()>;
    pub fn approve_pairing(&self, request_id: String, approve: bool) -> Result<()>;
    pub fn send_text(&self, text: String) -> Result<()>;
    pub fn send_image(&self, mime: String, bytes: Vec<u8>) -> Result<()>;
    pub fn devices(&self) -> Vec<DeviceInfo>;
    pub fn remove_device(&self, device_id: String) -> Result<()>;
    pub fn set_push_token(&self, platform: PushPlatform, token: String) -> Result<()>;
    pub fn handle_push_payload(&self, payload: Vec<u8>) -> Result<Option<Clip>>; // iOS NSE·Android FCM용
}

pub trait EventSink: Send + Sync {          // 셸이 구현
    fn on_clip(&self, clip: Clip);
    fn on_pairing_request(&self, req: PairingRequest);   // 이름 + SAS
    fn on_status(&self, status: ConnectionStatus);
    fn on_devices_changed(&self);
}

pub trait SecretStore: Send + Sync {         // 셸이 구현 (플랫폼 보안 저장소)
    fn get(&self, key: &str) -> Option<Vec<u8>>;
    fn set(&self, key: &str, value: &[u8]) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
}
```

### 릴레이 (`relay/`, 크레이트 `clip-ferry-relay`)

- `axum` WebSocket 서버. 프레임 파싱, 채널 구독 관리(메모리의 `채널 ID → 연결 목록` 맵만), 전달, 제한.
- 상태는 "지금 열린 연결"뿐. 연결이 끊기면 그 연결의 구독도 사라진다.
- 푸시 게이트웨이: `Publish`에 푸시 대상이 붙어 왔고 대상 태그가 온라인이 아니면 APNs/FCM 호출. 토큰을 보관하지 않는다.
- 확장 시(F26): 노드 간 전달을 NATS 주제(`cf.<채널 ID 해시>`)로. 노드는 자기 연결이 구독한 채널만 NATS 구독.
- 설정은 환경 변수/CLI 인자. 디스크 쓰기 없음(로그는 stdout).

### CLI (`tools/cli/`, 크레이트 `clip-ferry-cli`)

- 개발·검증용 클라이언트. 코어를 그대로 사용. `--profile <이름>`으로 한 컴퓨터에서 여러 기기를 흉내 낸다.
- 비밀 저장은 프로필 폴더의 파일(`FileSecretStore`, **개발 전용**, 시작 시 경고 출력).

### 플랫폼 셸

| 셸 | 기술 | 하는 일 |
|---|---|---|
| 데스크톱 (`apps/desktop/`) | Tauri 2 + React + TypeScript (Vite) | 트레이 앱, 클립보드 감지/쓰기, 페어링 UI(QR 표시·코드 입력), 기기 관리, 설정. 코어를 Rust에서 직접 호출 |
| Android (`apps/android/`) | Kotlin + Jetpack Compose | 포그라운드 서비스, 빠른 설정 타일, 알림 버튼, 공유·텍스트 선택 메뉴, QR 스캔, FCM |
| iOS (`apps/ios/`) | Swift + SwiftUI | 포그라운드 수신, App Intents, Share Extension, Notification Service Extension, QR 스캔, APNs |

셸에 비즈니스 로직을 넣지 않는다. "무엇을 보낼지·받은 걸 어디에 쓸지"만 결정한다.

## 레포 구조

```
clip-ferry/
├─ CLAUDE.md
├─ README.md
├─ docs/                       # 이 문서들
├─ proto/clip_ferry.proto      # 모든 와이어 포맷의 원본
├─ Cargo.toml                  # workspace: core, relay, tools/cli, apps/desktop/src-tauri
├─ core/
│  ├─ src/{crypto,envelope,group,pairing,transport,transfer,store,api}/
│  └─ tests/
├─ relay/
├─ tools/cli/
├─ apps/
│  ├─ desktop/                 # ui/ (React) + src-tauri/
│  ├─ android/                 # Gradle 프로젝트, core는 cargo-ndk로 빌드한 .so + UniFFI Kotlin
│  └─ ios/                     # Xcode 프로젝트, core는 xcframework + UniFFI Swift
├─ deploy/                     # Dockerfile, fly.toml 등 (F14)
└─ .github/workflows/
```

## 기술 스택과 허용 크레이트

아래 목록 외의 크레이트(특히 암호 관련)를 추가하려면 먼저 사용자에게 묻는다.

| 용도 | 크레이트 / 도구 |
|---|---|
| 비동기 | `tokio` |
| WebSocket (클라이언트) | `tokio-tungstenite` (+ `rustls`) |
| WebSocket (서버) | `axum` (ws 기능), `tower` |
| 직렬화 | `prost`, `prost-build` |
| 키 교환 | `x25519-dalek` |
| 서명 | `ed25519-dalek` |
| AEAD | `chacha20poly1305` (`XChaCha20Poly1305`) |
| 키 파생 / 해시 | `hkdf`, `sha2` |
| 난수 | `rand_core` (`OsRng`) / `getrandom` |
| 비밀 지우기 | `zeroize` |
| 에러 | `thiserror`(라이브러리), `anyhow`(바이너리) |
| 로그 | `tracing`, `tracing-subscriber` |
| 인코딩 | `base64`, `data-encoding`(base32), `hex` |
| QR 생성 | `qrcode` |
| CLI | `clap` |
| FFI | `uniffi` |
| 데스크톱 클립보드 | `arboard`, `windows`(Windows 리스너), `objc2`/`objc2-app-kit`(macOS) |
| 데스크톱 키 저장 | `keyring` |
| 이미지 변환 | `image` (PNG 인코딩만) |
| 푸시 (릴레이) | `reqwest`(rustls), `jsonwebtoken`(APNs/FCM 인증) |
| 확장 (F26) | `async-nats` |
| 메트릭 (F27) | `metrics`, `metrics-exporter-prometheus` |
| 테스트 | `proptest`, `tokio` test util |

빌드 도구: `cargo-ndk`(Android), `cargo-swift` 또는 수동 `xcodebuild -create-xcframework`(iOS), `tauri-cli`, GitHub Actions.

## 로컬 저장

- **비밀**(기기 개인키, 현재·이전 에포크 그룹 키, 로컬 저장 키): 플랫폼 보안 저장소 (`SecretStore`)
  - Windows: Credential Manager (`keyring`) / macOS·iOS: Keychain / Android: Keystore로 감싼 EncryptedSharedPreferences
- **상태**(로스터, 설정, 에포크 번호): 앱 데이터 폴더의 `state.bin`. 로컬 저장 키로 XChaCha20-Poly1305 암호화.
- **클립보드 내용은 디스크에 저장하지 않는다** (로컬 내역 기능 F29 전까지).

## 민감 클립보드 제외 (F11)

- Windows: `ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory = 0` 포맷이 있으면 보내지 않음
- macOS: `org.nspasteboard.ConcealedType`, `org.nspasteboard.TransientType` 타입이 있으면 보내지 않음
- Android: `ClipDescription.EXTRA_IS_SENSITIVE` 가 true면 보내지 않음
- 받은 내용을 쓸 때 Android 13+는 `EXTRA_IS_SENSITIVE`를 붙이지 않는다(사용자가 볼 수 있어야 함). 원본이 민감했다면 애초에 오지 않는다.

## 제한값 (기본값, 설정 가능)

| 항목 | 값 |
|---|---|
| 텍스트 최대 | 1 MiB (UTF-8) |
| 이미지 최대 | 20 MiB (원본 바이트) |
| 이미지 청크 | 256 KiB |
| 릴레이 프레임 최대 | 512 KiB |
| 연결당 구독 채널 수 | 8 |
| 연결당 속도 | 초당 50 프레임, 분당 64 MiB |
| IP당 동시 연결 | 32 |
| 이미지 조립 타임아웃 | 60초 |
