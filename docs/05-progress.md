# 05. 진행 상황

상태 값: `대기` → `진행중` → `확인대기` → `완료` (막히면 `보류`, 사유를 메모에)

- Claude Code는 기능을 시작할 때 `진행중`, 확인 요청할 때 `확인대기`로 바꾼다.
- `완료`는 **사용자가 승인한 뒤에만** 바꾼다. 메모에 한 줄 요약.

| ID | 기능 | 선행 | 상태 | 메모 |
|---|---|---|---|---|
| **M1** | **코어 · 릴레이 · CLI** | | | |
| F01 | 레포 스캐폴딩과 CI | - | 완료 | 로컬 검증 통과(Windows), 세 OS CI 통과, main 머지 |
| F02 | 코어 암호 기본 | F01 | 완료 | 로컬 검증 통과(Windows), 세 OS CI 통과, main 머지 |
| F03 | 메시지 포맷과 봉투 | F02 | 완료 | 로컬 검증 통과(Windows), 세 OS CI 통과, main 머지 |
| F04 | 릴레이 최소판 + 전송 계층 | F03 | 완료 | 로컬·터미널 3개 시나리오 확인, 세 OS CI 통과, main 머지 |
| F05 | 그룹 생성과 로컬 상태 | F04 | 완료 | 로컬·터미널 시나리오 확인, 세 OS CI 통과, main 머지 |
| F06 | 페어링 (CLI) | F05 | 완료 | 로컬·터미널 시나리오 확인, 세 OS CI 통과, main 머지 |
| F07 | 텍스트 송수신 (CLI) | F06 | 완료 | 로컬·터미널 시나리오 확인, 세 OS CI 통과, main 머지 |
| F08 | 기기 관리 · 키 회전 · 복구 | F07 | 완료 | 로컬·실제 프로세스 시나리오 확인, 세 OS CI 통과, main 머지 |
| **M2** | **데스크톱** | | | |
| F09 | 데스크톱 셸 기본 | F08 | 완료 | Windows 직접 확인, 세 OS CI 통과, main 머지. macOS 실행은 미확인(Mac 생기면 확인) |
| F10 | 데스크톱 페어링 UI | F09 | 완료 | Windows 직접 확인, 세 OS CI 통과, main 머지(PR #11). macOS 실행은 미확인 |
| F11 | 텍스트 자동 동기화 | F10 | 확인대기 | 로컬·실제 앱+CLI 자동 검증 통과(Windows). 직접 확인, macOS 실행, 세 OS CI는 확인 필요 |
| F12 | 이미지 동기화 | F11 | 대기 | |
| F13 | 기기 관리 UI와 설정 | F12 | 대기 | |
| F14 | 릴레이 배포 | F13 | 대기 | |
| **M3** | **Android** | | | |
| F15 | Android 셸 + 코어 바인딩 | F14 | 대기 | |
| F16 | Android 페어링 | F15 | 대기 | |
| F17 | Android 수신 → 클립보드 | F16 | 대기 | |
| F18 | Android 타일 · 알림 버튼 | F17 | 대기 | |
| F19 | Android 공유 · 텍스트 선택 메뉴 | F18 | 대기 | |
| F20 | 푸시 게이트웨이(FCM) | F19 | 대기 | |
| **M4** | **iOS** | | | |
| F21 | iOS 셸 + 페어링 + 포그라운드 수신 | F20 | 대기 | |
| F22 | APNs + NSE | F21 | 대기 | |
| F23 | App Intents 보내기 | F22 | 대기 | |
| F24 | Share Extension | F23 | 대기 | |
| F25 | iOS 이미지 수신 | F24, O-01 | 대기 | |
| **M5** | **확장** | | | |
| F26 | 다중 릴레이 (NATS) | F14 | 대기 | |
| F27 | 관측 · 남용 방지 | F26 | 대기 | |
| **M6** | **선택** | | | |
| F28 | 같은 네트워크 직접 연결 | - | 대기 | 착수 전 재합의 |
| F29 | 로컬 클립보드 내역 | - | 대기 | 착수 전 재합의 |
| F30 | 받을 기기 지정 전송 | - | 대기 | 착수 전 재합의 |
| F31 | 데스크톱 배포 패키지 | - | 대기 | 착수 전 재합의 |

## 작업 로그

새 항목을 위에 추가한다. 형식: `YYYY-MM-DD F00 — 한 줄`

- 2026-10-31 F11 — 데스크톱 클립보드 자동 동기화: `clipsync`(변경 감시 250ms·디바운스 300ms·에코 방지 해시·시작 전 내용/빈 텍스트/1MiB 초과 제외), `sysclip`(arboard + Windows 시퀀스 번호·민감 포맷 4종 / macOS changeCount·nspasteboard 숨김 타입), 셸은 받은 텍스트 쓰기와 `send_text`만, 단위 테스트 11개 + 실제 클립보드 테스트, 실제 앱+CLI 자동 검증 10항목, 확인대기
- 2026-10-31 F10 — core/pair_flow(릴레이 페어링 진행을 코어로: start_invite/start_join), 데스크톱 마법사·초대(QR·코드·남은 시간)·SAS 승인/거절·참여 화면·기기 목록, 개발용 프로필(CF_DESKTOP_PROFILE), 페어링 통합 테스트 5개, 실제 앱+CLI 양방향 페어링 자동 검증, 확인대기
- 2026-10-31 F09 — 사용자 확인·세 OS CI 통과·main 머지, 완료 (macOS 실행 미확인)
- 2026-10-31 F09 — apps/desktop(Tauri 2 + React): 트레이(상태 색)·창 닫아도 상주·Credential Manager `SecretStore`·릴레이 주소 설정·코어 이벤트→UI, 처음 실행 시 기기·그룹 자동 생성(임시, 마법사는 F10), CI에 Node·WebKitGTK 단계 추가, 테스트 12개 + 키 저장소 1개, 확인대기
- 2026-10-31 F08 — 사용자 승인·세 OS CI 통과·main 머지, 완료
- 2026-10-29 F02 — core/src/crypto 구현(기기 키·그룹 키·HKDF 파생·seal/open·wrap/unwrap), 테스트 20개 통과, 03-protocol 1절 확정, 확인대기
- 2026-10-29 F08 — 기기 이름 변경·제거, 키 회전(Rekey), 이전 에포크 30일 유예, EpochQuery·먼저 밀어 주기, 제거/stale 상태, CLI `devices`·`rename`·`remove`, 테스트 추가(core 118개, 릴레이 통합 4개), 확인대기
- 2026-10-29 F07 — 사용자 확인·세 OS CI 통과·main 머지, 완료
- 2026-10-29 F07 — core/session(`EventSink`, `send_text`), api `seal_text`/`handle_deliver`/`RosterUpdate` 채택, 텍스트 1 MiB 제한, 릴레이 프레임 2 MiB(D-012), CLI `send`·`listen`, 통합 테스트 5개 추가, 확인대기
- 2026-10-29 F06 — 사용자 확인·세 OS CI 통과·main 머지, 완료
- 2026-10-29 F06 — core/pairing(초대 코드·QR, SAS, 환영 메시지, 초대자/참여자 상태 기계), api create_invite/approve_join/join/accept_join, CLI `invite`·`join`, 테스트 추가(core 88개), 확인대기
- 2026-10-29 F05 — 사용자 확인·세 OS CI 통과·main 머지, 완료
- 2026-10-29 F05 — core/store·group·api, CLI `--profile`·`init`·`group create`·`status`, 테스트 추가(core 54개), 확인대기
- 2026-10-29 F04 — 사용자 확인·세 OS CI 통과·main 머지, 완료
- 2026-10-29 F04 — relay(axum WS, 제한값, 집계 로그), core/transport(자동 재연결·재구독·핑), CLI `debug sub/pub`, 통합 테스트 14개 통과, 확인대기
- 2026-10-29 F03 — 세 OS CI 통과, main 머지, 완료
- 2026-10-29 F03 — proto 전체 정의, core/src/envelope(서명·봉인·검증 파이프라인·중복·시간 창), 테스트 42개 통과, 확인대기
- 2026-10-29 F02 — 세 OS CI 통과, main 머지, 완료
- 2026-10-29 F01 — 사용자 로컬 확인 완료, 완료 처리 (세 OS CI 통과, main 머지)
- 2026-10-28 F01 — 워크스페이스·CI 골격 작성, 로컬 fmt/clippy/test/--version 통과, 확인대기
- 2026-10-28 — 문서 초안 작성
