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
| `pair_flow` | `pairing` 상태 기계에 릴레이 연결을 붙여 초대·참여를 끝까지 진행(셸은 진행 알림만 받고 승인·거절을 알림) |
| `transport` | 릴레이 WebSocket 연결, 재연결(지수 백오프), 구독, 핑, 송수신 |
| `transfer` | 이미지 청크 분할/조립, 해시 검증, 타임아웃 |
| `store` | 로컬 상태 저장(암호화된 파일) + `SecretStore` 트레이트 |
| `api` | 셸이 쓰는 공개 API (모바일 셸에는 `ffi/` 크레이트가 UniFFI로 노출, D-015) |

코어 공개 API 스케치 (구체 시그니처는 구현 기능에서 확정):

```rust
pub struct ClipFerry { /* ... */ }

impl ClipFerry {
    pub fn open(config: Config, secrets: Box<dyn SecretStore>) -> Result<Self>;   // EventSink는 이벤트가 생기는 기능에서 추가
    pub fn init_device(&self, device_name: &str) -> Result<()>;   // 기기 키·로컬 저장 키 생성 (F05)
    pub fn create_group(&self) -> Result<()>;                    // 에포크 1, 구성원은 이 기기뿐 (F05)
    pub fn status(&self) -> Result<Status>;                      // 이름·device_id·에포크·구성원 수 (F05)
    pub fn connection(&self) -> Option<Connection>;              // 릴레이 접속용 채널 ID·태그 (F05)
    pub fn create_invite(&self) -> Result<Invite>;            // QR 문자열 + 텍스트 코드
    pub fn join(&self, code: String, device_name: String) -> Result<()>;
    pub fn approve_pairing(&self, request_id: String, approve: bool) -> Result<()>;
    pub fn send_text(&self, text: String) -> Result<()>;
    pub fn send_image(&self, mime: String, bytes: Vec<u8>) -> Result<()>;
    pub fn devices(&self) -> Vec<DeviceInfo>;                    // F05
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
- 접속 토큰 확인(F33, `relay/src/auth.rs`): 계정 서버 주소가 있을 때만 켜진다. 계정 서버 공개키로 서명만 확인하고, 계정별 연결 수·분당 바이트를 세며, 계정별 게시 수·바이트를 1분마다 계정 서버에 보낸다(03-protocol 5절).

### 계정 서버 (`account/`, 크레이트 `clip-ferry-account`, M7 · D-019)

- axum + MariaDB(`sqlx`). 구글·Apple ID 토큰을 검증해 계정을 만들고, 릴레이 접속 토큰(JWT, 1시간)과 갱신 토큰(해시로만 저장)을 발급한다.
- 저장: 계정 기본정보, 계정에 묶인 기기 목록, 일별 사용량(전송 수·바이트). 클립보드 내용·그룹 키는 저장하지도 받지도 않는다.
- 릴레이는 DB를 보지 않고 접속 토큰 서명만 확인하며, 계정별 사용량을 1분마다 계정 서버 내부 API로 보낸다(F33).
- 개발·시험은 Pi의 Docker(+ MariaDB 볼륨)에서, 공개 배포는 F40. 배포는 `deploy/compose.account.yml`, API 목록은 `account/src/api.rs` 머리말.
- 저장소는 `Store`(MariaDB / 메모리) 하나로 감싸 테스트는 메모리로 어디서나 돌고, CI의 `account (MariaDB)` 작업이 같은 테스트를 실제 MariaDB로 돌린다(`CF_TEST_MARIADB_URL`).

### FFI (`ffi/`, 크레이트 `clip-ferry-ffi`)

- 모바일 셸(Kotlin/Swift)용 UniFFI 바인딩. 코어를 감싸는 얇은 층이며 암호·프로토콜 로직은 없다. 코어 크레이트에는 UniFFI를 넣지 않는다(D-015).
- `ClipFerryClient` 객체가 tokio 런타임과 릴레이 세션을 쥐고, 셸에는 동기 호출(`open`, `init_device`, `create_group`, `status`, `start`, `stop`)과 콜백(`CoreEvents`: 연결 상태·온라인 기기 수·변경 알림)을 준다. 비밀 저장소는 셸이 구현하는 `SecretVault` 콜백이다.
- 오류는 `FfiError`(`Removed` / `Failed{reason}`)로 단순화한다.
- 바인딩 생성기는 `tools/uniffi-bindgen`. 라이브러리 모드로 빌드된 `.so`의 메타데이터에서 Kotlin을 만든다.
- F16에서 페어링(`start_invite`·`decide_invite`·`start_join`·`cancel_pairing`, 결과는 `PairListener` 콜백), 기기 목록(`devices`, Presence로 온라인 표시), `leave_group`, QR 칸(`qr_matrix`, 데스크톱과 같은 `qrcode` 크레이트)을 더했다. F17에서 받은 텍스트·이미지 콜백(`CoreEvents::on_text`·`on_image`, 이미지 형식은 머리말로 다시 확인)을 더했다. F18에서 보내기(`send_text`·`send_image`, 이미지 형식·크기는 머리말로 알아냄)를 더했다.

### CLI (`tools/cli/`, 크레이트 `clip-ferry-cli`)

- 개발·검증용 클라이언트. 코어를 그대로 사용. `--profile <이름>`으로 한 컴퓨터에서 여러 기기를 흉내 낸다.
- 비밀 저장은 프로필 폴더의 파일(`FileSecretStore`, **개발 전용**, 시작 시 경고 출력).

### 플랫폼 셸

| 셸 | 기술 | 하는 일 |
|---|---|---|
| 데스크톱 (`apps/desktop/`) | Tauri 2 + React + TypeScript (Vite) | 트레이 앱, 클립보드 감지/쓰기, 페어링 UI(QR 표시·코드 입력), 기기 관리, 설정. 코어를 Rust에서 직접 호출 |
| Android (`apps/android/`) | Kotlin + Jetpack Compose | 포그라운드 서비스, 빠른 설정 타일, 알림 버튼, 공유·텍스트 선택 메뉴, QR 스캔, FCM |
| iOS (`apps/ios/`) | Swift + SwiftUI | 포그라운드 수신, App Intents, Share Extension, Notification Service Extension, QR 스캔, APNs |

Android 빌드 메모: 필요한 것은 Android Studio(SDK 플랫폼·NDK·CMake), 환경 변수 `ANDROID_HOME`·`ANDROID_NDK_HOME`, `rustup target add aarch64-linux-android x86_64-linux-android`, `cargo install cargo-ndk --locked`. `scripts/build-android-core.ps1`(Windows) 또는 `.sh`(macOS·Linux)가 `.so`를 `apps/android/app/src/main/jniLibs/`에, Kotlin 바인딩을 `app/src/main/java/uniffi/`에 만든다(둘 다 git에 넣지 않는다). 그 뒤 Android Studio로 열거나 `apps/android`에서 `./gradlew installDebug`. 처음 받은 저장소는 코어 스크립트를 먼저 한 번 돌려야 Gradle 빌드가 된다. Android 셸의 파일은 `KeystoreVault`(Keystore 비밀 저장소), `CoreController`(코어 ↔ 화면 상태), `ClipFerryService`(포그라운드 서비스·네트워크 변경 감지), `MainActivity`(Compose 화면: 첫 실행·메인), `PairingScreens`(참여·초대·확인 숫자·기기 목록), `QrScanner`(CameraX + ML Kit 번들 모델, 기기 안에서만 인식), `ReceivedClips`(받은 텍스트·이미지를 클립보드에 쓰기. 이미지는 `cache/received/`에 최근 1개만 두고 `FileProvider` URI로 넘긴다), `SendActivity`·`SendTileService`·`SendClipboard`(타일·알림 버튼 → 보이지 않는 액티비티가 초점을 받아 클립보드를 읽고 보낸다), `ShareActivity`(공유 시트·텍스트 선택 메뉴 → 보이지 않는 액티비티. 공유받은 이미지는 `cache/outgoing/`에 복사한 뒤 `SendClipboard.sendBatch`로 한 장씩 보낸다), `ShareShortcut`(공유 시트 맨 윗줄 직접 공유 바로가기 `다른 기기로`).

데스크톱 빌드 메모: Tauri 매크로가 컴파일 때 `apps/desktop/dist`(UI 빌드 결과)를 읽는다. 새로 받은 저장소에서는 `cd apps/desktop && npm ci && npm run build`를 먼저 한 번 해야 `cargo build/test --workspace`가 된다(CI도 같은 순서). `src-tauri`는 `shell`(코어 연결), `tray`, `secrets`(keyring), `settings`, `view`(UI용 요약)로 나뉜다.

셸에 비즈니스 로직을 넣지 않는다. "무엇을 보낼지·받은 걸 어디에 쓸지"만 결정한다.

## 레포 구조

```
clip-ferry/
├─ CLAUDE.md
├─ README.md
├─ docs/                       # 이 문서들
├─ proto/clip_ferry.proto      # 모든 와이어 포맷의 원본
├─ Cargo.toml                  # workspace: core, ffi, relay, tools/cli, tools/uniffi-bindgen, apps/desktop/src-tauri
├─ core/
│  ├─ src/{crypto,envelope,group,pairing,transport,transfer,store,api}/
│  └─ tests/
├─ ffi/                        # 모바일용 UniFFI 바인딩 (D-015)
├─ relay/
├─ account/                    # 계정 서버 (M7, D-019)
├─ tools/cli/
├─ tools/uniffi-bindgen/
├─ apps/
│  ├─ desktop/                 # ui/ (React) + src-tauri/
│  ├─ android/                 # Gradle 프로젝트, core는 cargo-ndk로 빌드한 .so + UniFFI Kotlin
│  └─ ios/                     # Xcode 프로젝트, core는 xcframework + UniFFI Swift
├─ scripts/                    # build-android-core 등
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
| 계정 서버 DB (M7, D-019) | `sqlx`(MySQL 드라이버로 MariaDB, rustls) |
| 테스트 | `proptest`, `tokio` test util |

빌드 도구: `cargo-ndk`(Android), `cargo-swift` 또는 수동 `xcodebuild -create-xcframework`(iOS), `tauri-cli`, GitHub Actions.

## 로컬 저장

- **비밀**(기기 개인키, 현재·이전 에포크 그룹 키, 로컬 저장 키): 플랫폼 보안 저장소 (`SecretStore`)
  - Windows: Credential Manager (`keyring`) / macOS·iOS: Keychain / Android: Keystore의 AES-256-GCM 키로 봉인한 SharedPreferences(이름을 AAD로 묶는다. `EncryptedSharedPreferences`는 deprecated라 직접 구현)
- **상태**(로스터, 설정, 에포크 번호): 앱 데이터 폴더의 `state.bin`. 로컬 저장 키로 XChaCha20-Poly1305 암호화.
  - 형식: `"CFST"`(4) ‖ 형식 버전(1) ‖ nonce(24) ‖ 암호문. 머리말(`"CFST"`‖버전)이 aad. 평문은 `LocalState` protobuf.
  - 비밀 저장소 키 이름: `device_keys`(서명 시드‖DH 비밀, 64B), `state_key`(32B), `group_key/<에포크>`(32B).
  - 비밀 저장소와 `state.bin`이 한쪽만 남아 있으면 열기를 거부한다(조용히 새로 만들지 않는다).
- **클립보드 내용은 디스크에 저장하지 않는다** (로컬 내역 기능 F29 전까지).

## 민감 클립보드 제외 (F11)

**설정 `sync_sensitive`(기본 켜짐)**: 사용자가 모바일↔데스크톱으로 비밀번호를 옮겨 쓰는 일이 잦아 기본은 민감 표시가 있어도 동기화한다(전송은 종단간 암호화). 이 설정을 끄면 아래 표시가 붙은 복사는 읽지도 보내지도 않는다. 설정 화면은 F13, 그전에는 `settings.json`. 모바일 셸도 같은 설정을 따른다.

- Windows: `ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory = 0` 포맷이 있으면 보내지 않음
- macOS: `org.nspasteboard.ConcealedType`, `org.nspasteboard.TransientType` 타입이 있으면 보내지 않음
- Android: `ClipDescription.EXTRA_IS_SENSITIVE` 가 true면 보내지 않음 (설정 `민감한 내용도 보내기`를 껐을 때, F18)
- 받은 내용을 쓸 때 Android 13+는 `EXTRA_IS_SENSITIVE`를 붙이지 않는다(사용자가 볼 수 있어야 함). 원본이 민감했다면 애초에 오지 않는다.

## 제한값 (기본값, 설정 가능)

| 항목 | 값 |
|---|---|
| 텍스트 최대 | 1 MiB (UTF-8) |
| 이미지 최대 | 20 MiB (원본 바이트) |
| 이미지 청크 | 256 KiB |
| 릴레이 프레임 최대 | 2 MiB (D-012: 1 MiB 텍스트가 한 봉투에 들어가야 함) |
| 연결당 구독 채널 수 | 8 |
| 연결당 속도 | 초당 50 프레임, 분당 64 MiB |
| IP당 동시 연결 | 32 |
| 이미지 조립 타임아웃 | 60초 |
