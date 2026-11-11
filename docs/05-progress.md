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
| F11 | 텍스트 자동 동기화 | F10 | 완료 | Windows 직접 확인, 세 OS CI 통과(PR #12). 민감 표시 복사는 기본 동기화(설정 `sync_sensitive`, 사용자 결정) 후속 PR. macOS 실행은 미확인 |
| F12 | 이미지 동기화 | F11 | 완료 | Windows 직접 확인, 세 OS CI 통과, main 머지(PR #15). macOS 실행은 미확인(Mac 생기면 확인) |
| F13 | 기기 관리 UI와 설정 | F12 | 완료 | Windows 직접 확인(온라인 표시·이름 변경·제거·일시정지·다시 페어링), 세 OS CI 통과, main 머지. 재부팅 후 자동 시작은 2026-11-02 Windows에서 확인. macOS 실행은 미확인. 플랫폼 표시는 프로토콜 변경이 필요해 제외 |
| F14 | 릴레이 배포 (Pi + Tailscale) | F13 | 완료 | 사용자 시험 통과(Pi 5 arm64 빌드·기동, PC에서 Tailscale 접속, 텍스트·이미지 동기화), main 머지(PR #18, F14b와 함께). macOS 미확인 |
| F14b | 릴레이 공개 배포 (`wss://`) | F14 | 완료 | `tailscale serve`로 wss(무료 인증서), 릴레이 `--trust-proxy`(X-Forwarded-For). 사용자 시험 통과(wss 동기화·평문 차단·로그). Pi 재부팅 후 자동 복구 확인, main 머지(PR #18). 앱 기본 릴레이 주소·클라우드 이전은 도메인 결정(O-03, O-04) 후로 미룸 |
| **M3** | **Android** | | | |
| F15 | Android 셸 + 코어 바인딩 | F14b | 완료 | 실기기에서 연결·백그라운드 유지·배터리 안내·네트워크 전환·재시작 확인 통과, PR #19 머지. 확인 문서: `todo/F15-확인.md` |
| F16 | Android 페어링 | F15 | 완료 | 실기기 확인 통과(QR·코드 참여, 카메라 거부 안내, 폰 초대·거절, 취소·만료·재시작), 세 OS CI 통과, PR #20 머지. 확인 문서: `todo/F16-확인.md` |
| F17 | Android 수신 → 클립보드 | F16 | 완료 | 실기기 확인 통과(텍스트·이미지 붙여넣기, 캐시 최근 1개, 알림 설정, 중지·재시작, 화면 꺼짐), 세 OS CI 통과, PR #21 머지. 확인 문서: `todo/F17-확인.md` |
| F18 | Android 타일 · 알림 버튼 | F17 | 완료 | 실기기 확인 통과(타일·알림 버튼으로 텍스트·이미지, 깜빡임 없음, 되돌려 보내기 방지, 민감 설정, 자동 연결, 잠금 상태), 세 OS CI 통과, PR #22 머지. 확인 문서: `todo/F18-확인.md` |
| F19 | Android 공유 · 텍스트 선택 메뉴 | F18 | 완료 | 실기기 확인 통과(갤러리 공유 한 장·여러 장·HEIC, 텍스트 선택 메뉴 `PC로 보내기`, 텍스트 공유, 직접 공유 바로가기 `다른 기기로`, 자동 연결, 화면 회전 시 한 번만), 세 OS CI 통과, PR #23 머지. 확인 문서: `todo/F19-확인.md` |
| F20 | 푸시 게이트웨이(FCM) | F19 | 보류 | 사용자 결정(2026-11-05): Android는 포그라운드 서비스로 충분. D-017 |
| **M4** | **iOS** | | | |
| F21 | iOS 셸 + 페어링 + 포그라운드 수신 | F19 | 대기 | |
| F22 | APNs + NSE | F21 | 대기 | |
| F23 | App Intents 보내기 | F22 | 대기 | |
| F24 | Share Extension | F23 | 대기 | |
| F25 | iOS 이미지 수신 | F24, O-01 | 대기 | |
| **M5** | **확장** | | | |
| F26 | 다중 릴레이 (NATS) | F14 | 대기 | |
| F27 | 관측 · 남용 방지 | F26 | 대기 | |
| **M7** | **계정 (D-019)** | | | |
| F32 | 계정 서버 + 구글 로그인 검증 | F14b | 완료 | PC(메모리)·Pi(MariaDB)에서 실제 구글 로그인 확인, 세 OS + `account (MariaDB)` CI 통과(PR #27). 확인 문서: `todo/F32-확인.md` |
| F33 | 릴레이 토큰 확인 · 계정별 제한 · 사용량 | F32 | 완료 | 구현·테스트 통과, 사용자 PC 확인 1~6 통과. Pi에서 켜는 것은 F34·F35 뒤. 확인 문서: `todo/F33-확인.md` |
| F34 | 데스크톱 로그인 | F33 | 완료 | 사용자 PC 확인 1~7 통과(7: Pi 계정 서버로 기존 앱 업데이트, 폰 동기화 유지), 세 OS + `account (MariaDB)` CI 통과(PR #29). 확인 문서: `todo/F34-확인.md` |
| F35 | Android 로그인 | F33 | 완료 | 실기기 확인 0~6 통과(Releases 배포 APK 위에 배포 키 release 빌드로 업데이트, 그룹 유지, 토큰 확인 릴레이 접속), 세 OS + `account (MariaDB)` CI 통과(PR #30). 확인 문서: `todo/F35-확인.md` |
| F36 | iOS 로그인 (구글 + Apple) | F33, F25 | 대기 | |
| F37 | 계정으로 기기 연결 | F34, F35 | 완료 | PR #31. SAS 확인 유지 + 키 봉인 후 공개(D-022) |
| F38 | 운영자 통계 | F33 | 완료 | PR #32. `/admin` + 구글 로그인 허용 목록(D-023) |
| F39 | 계정 삭제 · 개인정보처리방침 | F34, F35 | 완료 | PR #33. 서버 `DELETE /v1/me` 즉시 전부 삭제, 사용량 13개월, `docs/privacy.md` 초안. 앱 버튼 없음 — 삭제는 홈페이지(F40)에서(D-024). 확인 문서: `todo/F39-확인.md` |
| F40 | 공개 배포 (도메인 · 클라우드) | F32~F39, O-03, O-04 | 진행중 | **1단계 완료**(PR #34): Azure VM(Japan East) + Caddy로 이전(D-025). 공개 주소로 로그인·텍스트·이미지(LTE 포함), 운영자 페이지, 백업·Pi 가져가기·복원, 재부팅 복구 확인. Pi 서버는 내림(6-10). 확인 문서: `todo/F40-확인.md`. 2단계(도메인·홈페이지·프로덕션)는 그 뒤 |
| F41 | 데스크톱 · Android Apple 로그인 | F36, O-03 | 대기 | 웹 방식이라 도메인 필요 |
| **M6** | **선택** | | | |
| F28 | 같은 네트워크 직접 연결 | - | 대기 | 착수 전 재합의 |
| F29 | 로컬 클립보드 내역 | - | 대기 | 착수 전 재합의 |
| F30 | 받을 기기 지정 전송 | - | 대기 | 착수 전 재합의 |
| F31 | 데스크톱 배포 패키지 | - | 완료 | 1단계: GitHub Releases 배포 워크플로(데스크톱 3 OS + Android 서명 APK), `v0.1.0` 배포, 폰·PC에 설치해 동기화 확인, 문서 `docs/07-release.md`. 코드 서명·공증·자동 업데이트는 후속(남은 항목은 04-features M6). 확인 문서: `todo/F31-확인.md` |
| F43 | 앱 기본 서버 주소 · 주소 숨기기 | F40 1단계 | 완료 | 배포 빌드 기본 계정 서버 = Azure 이름, 릴레이는 계정 서버 `/v1/config`가 알려 줌, 주소 칸 숨김(제목 7번). D-026 |
| F44 | 계정 하나당 그룹 하나 | F37, F43 | 완료 | PR #36. 계정에 그룹이 있으면 `새 그룹 만들기` 숨김·계정 연결만(QR·코드 페어링 제거), 탈출구 `처음부터 다시 시작`(예전 기기 자동으로 나감), 앱 데이터만 지워지면 묻고 초기화, 백그라운드 실행 기본값(데스크톱 자동 시작·Android 부팅 후 연결). D-027. 확인 문서: `todo/F44-확인.md` |
| F42 | Android 떠 있는 보내기 버튼 | F18 | 대기 | 사용자 요청(2026-11-08): 카카오톡·디스코드는 텍스트 선택 메뉴에 `PC로 보내기`가 안 나옴. 착수 전 재합의 |

## 작업 로그

새 항목을 위에 추가한다. 형식: `YYYY-MM-DD F00 — 한 줄`

- 2026-11-11 F44 — 완료 (PR #36 머지, CI 통과, 사용자 승인). 머지 뒤 VM을 `CF_IMAGE_TAG=main`으로 돌려놓는다(todo/F44-확인.md 7절)
- 2026-11-11 F44 — 6-1 실기기 확인 통과(PC 첫 실행 자동 시작 켜짐·재부팅 후 트레이만·끄면 유지, 폰 업데이트·재부팅 뒤 앱 열지 않고 연결). 확인대기
- 2026-11-11 F44 — 추가 요청(사용자): 백그라운드 실행 기본값. 데스크톱 배포 빌드는 처음 실행 때 한 번 `로그인할 때 자동 시작`을 켜고(표시 파일 `autostart-default-applied`, 끄면 그대로), 자동 시작(`--autostart`)으로 켜지면 로그인·그룹이 있을 때 창 없이 트레이에만. Android `BootReceiver`(부팅·앱 업데이트 뒤 `백그라운드 연결 유지`가 켜져 있고 로그인·그룹이 있으면 서비스 시작, `RECEIVE_BOOT_COMPLETED`). 데스크톱 clippy·테스트, `assembleDebug` 통과. 진행중
- 2026-11-11 F44 — 실기기 확인 1~6절 통과(서버 DB 정리·`f44-one-group` 이미지, 배포 빌드 PC·폰: 첫 기기만 `새 그룹 만들기`, 두 번째 기기 계정 연결, 처음부터 다시 시작 → PC 자동으로 나감, 앱 데이터만 지운 PC 초기화). 시험 중: 계정 서버 컨테이너 `Removed`(compose), 초기화 경고 창이 검은 빈 창 뒤에 숨음 → 묻기 전 창 숨김·`MB_TOPMOST`(빌드 확인 전). 확인대기
- 2026-11-10 F44 — 시작 (브랜치 `f44-one-group`, F43 완료 커밋 위). 계정 서버 `devices.in_group`·`retired`(마이그레이션 0004), `PUT /v1/devices`의 `in_group`, `POST /v1/group/restart`, `409 device_retired`. 데스크톱·Android: 계정에 그룹이 있으면 `새 그룹 만들기` 숨김, `처음부터 다시 시작`, 예전 그룹 자동으로 나감, QR·코드 페어링 화면 제거(Android 카메라 권한·CameraX·ML Kit 제거), 시작 때 로컬 상태를 못 열면 묻고 초기화(코어 `reset_local`). FFI `account_has_group`·`restart_group`·`reset_local_data`·`is_unrecoverable_state`·`CoreEvents.on_group_retired`. 테스트: 계정 +1(MariaDB 포함), 코어 +1, 데스크톱 +1, FFI +2. `assembleDebug` 성공. D-027
- 2026-11-10 F43 — 완료 (PR #35). 머지 뒤 VM을 `CF_IMAGE_TAG=main`으로 돌려놓는다(todo/F43-확인.md 5절)
- 2026-11-10 F43 — 실기기 확인(서버 `f43-default-server` 이미지 `/v1/config`, Release 워크플로 배포 빌드: PC·폰 주소 없이 연결·동기화, 주소 칸 숨김·제목 7번). 시험 중 PC 앱 데이터가 지워져 시작 즉시 종료(키만 남음) → 남은 키 삭제 후 새로 로그인. 시작 오류를 창에 보여 주는 것은 F44에서. 확인대기
- 2026-11-10 F43 — 시작 (브랜치 `f43-default-server`, F40 완료 커밋 위). 계정 서버 `GET /v1/config`(`CF_ACCOUNT_RELAY_URL`, compose.cloud는 `wss://${CF_DOMAIN}`), 데스크톱·Android 배포 빌드 기본 주소, 계정 서버에서 릴레이 주소 받기(직접 넣지 않았을 때), 주소 칸·문구 숨김(제목 7번), 오류 문구에서 주소 뺌. 테스트: 계정 +1, 데스크톱 +1, FFI +1. Android `assembleDebug`·`assembleRelease` 성공(BuildConfig 기본 주소 확인). Images 워크플로가 `f[0-9]*` 브랜치도 올림. D-026
- 2026-11-10 F40 — 1단계 완료 (PR #34 머지, CI 통과, 사용자 승인). 2단계(도메인·앱 기본 주소·홈페이지·프로덕션)는 착수 때 합의
- 2026-11-10 F40 — 서버 배포 확인(사용자): Pi DB 이전(users 2), 밖에서 `wss://`·`https://` ok, `/internal` 404, DB·내부 포트 닫힘, Tailscale 끄고 PC·폰(LTE) 텍스트·이미지 동기화, 운영자 페이지 전송 수 증가, 백업 → Pi 가져가기 → 빈 DB 복원(users 2·devices 15), VM 재부팅 후 복구. 고친 것: VM 사용자 이름(`jonghyeok`) 반영, 80 포트 규칙 누락. 확인대기
- 2026-11-10 F40 — VM(Japan East, B2ts v2, 크레딧) 만들고 포트까지(사용자). 학생 구독은 허용 지역 5곳뿐이고 어디서도 무료 크기 불가. SSH는 Tailscale 없이 어디서든 키로만(사용자 결정). 배포 코드: `images.yml`(GHCR, amd64), `deploy/compose.cloud.yml`·`Caddyfile`(검증 통과)·`env.cloud.example`, 백업 `backup.sh`·`backup-send.sh`·`pull-backup.sh`. 확인 문서 6절(배포·Pi DB 이전·앱 전환·백업·복원·재부팅)
- 2026-11-10 F40 — 시작 (브랜치 `f40-azure-deploy`). O-04 1단계 결정 D-025(Azure for Students VM + Caddy, 사용자 크레딧 $100). 04-features에 1단계 범위 초안, 2단계(도메인·홈페이지·프로덕션)와 분리
- 2026-11-09 F31 — 1단계 완료 (사용자 확인: 배포 키 백업, Releases APK·exe 설치, 설치한 앱끼리 텍스트 동기화)
- 2026-11-09 F39 — 완료 (PR #33 머지, CI 세 OS·MariaDB 통과, 사용자 승인)
- 2026-11-09 F39 — Pi 확인(시험 계정 로그인·기기 등록 → `DELETE /v1/me` 204 두 번 → `/v1/me`·기기 등록·갱신 모두 401, 로그 `account deleted`, 방침 초안 검토), 개발용 로그인 끔. 시험 중 문제 없음(compose 네트워크 경고만). 확인대기
- 2026-11-09 F39 — 범위 변경(사용자 결정): 앱의 `계정 삭제` 버튼·경로를 빼고 삭제는 나중에 홈페이지(F40)에서. 데스크톱·FFI·Android 변경을 되돌림. 서버 `DELETE /v1/me`·13개월 정리·`docs/privacy.md`는 남김. D-024 고침. 진행중
- 2026-11-09 F39 — 계정 서버 `DELETE /v1/me`(계정·기기·갱신 토큰·사용량을 표마다 삭제), `prune`이 13개월 지난 사용량 삭제, `docs/privacy.md` 초안. D-024. 테스트: 계정 +3(모든 표에서 삭제·갱신 401·재가입은 새 계정, 다른 계정은 그대로, 보관 기간). 확인 문서 `todo/F39-확인.md`. 진행중
- 2026-11-09 F38 — 완료 (PR #32 머지, CI 세 OS·MariaDB 통과, 사용자 승인). CI에서 고친 것: MariaDB 통계 테스트의 동시 실행 섞임, 기기 등록·토큰 교체 트랜잭션의 교착 재시도
- 2026-11-09 F38 — 실기기 확인(Pi MariaDB에서 마이그레이션 0003·통계 질의 동작, 구글 로그인·표·검색·상세, 전송 뒤 숫자 증가, 허용 목록에서 빼면 즉시 403). 시험 중 문제 없음. 확인대기
- 2026-11-09 F38 — 계정 서버 `/admin`(구글 로그인 → 허용 목록 → 운영자 토큰, 대상 `clip-ferry-admin`), 통계 API(가입자·DAU·MAU·플랫폼별 기기·일별 활성·신규·전송·바이트, 사용자 목록·상세), 마이그레이션 0003(날짜 색인), `CF_ACCOUNT_ADMIN_EMAILS`. D-023. 테스트: 계정 +5(허용 목록 밖 403, 토큰 대상 분리, 통계 증가분, 검색 LIKE 이스케이프, 페이지 CSP). MariaDB 질의는 CI에서 처음 돈다. 확인 문서 `todo/F38-확인.md`. 진행중
- 2026-11-09 F37 — 완료 (PR #31 머지, CI 세 OS·MariaDB 통과, 사용자 승인)
- 2026-11-09 F37 — 실기기 확인(데스크톱↔데스크톱 연결·무시·거절, 폰이 기존 기기로 PC-C를 받음, 폰이 계정 연결로 다시 참여, 양쪽 전송). 고친 것: 데스크톱 시작 시 교착(로그인된 프로필에서 `응답 없음`), CLI `InviterEvent::Reply` 미처리(CI clippy). 확인대기
- 2026-11-08 F37 — 계정 서버 `POST /v1/link/ticket`(참여 확인서, 접속 토큰 키로 서명), 코어 `link`(계정 채널 `HKDF(user_id)`, 확인서 확인)·봉인 후 공개 페어링(`PairCommit`·`PairReveal`, SAS에 두 nonce)·`start_link_invite`/`start_link_join`·`Session::set_account_link`/`on_link_request`, 데스크톱(마법사 버튼·홈 화면 요청 카드·알림), FFI·Android(첫 화면 버튼·요청 카드·알림 채널). 03-protocol 4.4, D-022. 테스트: 코어 +10(중간자면 숫자 다름), 계정 +4, 릴레이 통합 3, 데스크톱 +1, FFI +3. `assembleDebug` 성공. 확인 문서 `todo/F37-확인.md`. 진행중
- 2026-11-08 F37 — 시작 (브랜치 `f37-account-link`). 설계 합의: 계정 채널 + 계정 서버 서명 참여 확인서 + 키 봉인 후 공개(D-022)
- 2026-11-08 F35 — PR #30 CI 통과·머지. 완료
- 2026-11-08 F35 — 실기기 확인(`todo/F35-확인.md` 0~6) 전부 통과. 폰 앱이 Releases 배포 APK(versionCode 4)라 `installDebug` 불가 → 배포 키 release 빌드(versionCode 5)로 업데이트, 배포 키 SHA-1용 Android 클라이언트 추가. 확인대기
- 2026-11-07 F35 — 계정 서버 `GET /v1/auth/google/android`(웹 클라이언트 ID, `CF_ACCOUNT_GOOGLE_WEB_CLIENT_ID` 없으면 목록 첫 번째), `ffi/src/account.rs`(ID 토큰·개발용 로그인, 갱신 한 번에 하나, 거절되면 지우고 세션 닫음 `on_signed_out`, 로그아웃, 기기 등록 `android`), 로그인 전 `start`·페어링 거부, 모든 릴레이 연결에 접속 토큰. Android: `GoogleLogin`(Credential Manager), 로그인 화면·서버 설정·개발용 로그인(디버그), 계정 칸·로그아웃, 서비스·타일·공유가 로그인 상태를 따름. D-021. 테스트: 계정 +1·FFI +5(토큰 확인 릴레이로 페어링·전송, 갱신 거절 → 로그아웃, 계정 서버 꺼짐). `assembleDebug` 성공. 진행중
- 2026-11-07 F35 — 시작 (브랜치 `f35-android-login`)
- 2026-11-07 F34 — PR #29 CI 통과·머지. 완료
- 2026-11-07 F34 — 사용자 PC 확인(`todo/F34-확인.md` 1~7) 전부 통과. 시험 중 Pi compose 프로젝트 이름 충돌 고침. 확인대기
- 2026-11-06 F34 — 계정 서버 `GET /v1/auth/google/desktop`·`POST /v1/auth/google/code`(데스크톱 클라이언트 보안 비밀은 서버만, 루프백 돌아올 주소만, D-020), 데스크톱 `account`(시스템 브라우저 + 127.0.0.1 루프백 + PKCE·`state`, 갱신 토큰 keyring `account/session`, 접속 토큰 메모리, 갱신 한 번에 하나, 거절되면 로그인 화면), 로그인 전 연결 안 함, 로그인 화면·계정 칸·로그아웃(그룹 유지), 설정 `account_url`, 디버그 빌드 개발용 로그인, 기기 등록. 테스트: 계정 +4·데스크톱 +8(실제 계정 서버로 로그인·동시 갱신·재시작·로그아웃). 확인 문서 `todo/F34-확인.md`. 진행중
- 2026-11-06 F34 — 시작 (브랜치 `f34-desktop-login`)
- 2026-11-06 F33 — PR #28 CI 통과·머지. 완료
- 2026-11-06 F33 — 사용자 PC 확인(`todo/F33-확인.md` 1~6) 전부 통과. 확인대기
- 2026-11-06 F33 — 릴레이 `auth.rs`(계정 서버 JWKS로 접속 토큰 확인, 401·503·429, 계정별 연결 수·분당 바이트, 계정별 사용량 1분마다 `POST /internal/usage`, 실패 시 되돌림), 코어 `TransportConfig.token`(`TokenSource`)·`Event::Unauthorized`·`EventSink::on_auth_required`, 계정 서버 `/internal/usage`·`usage_daily` 누적·`/v1/me`에 사용량, CLI `login --dev`·`account`·`logout`, `deploy/compose.relay-auth.yml`. 테스트: 릴레이 인증 7·계정 +1(MariaDB 포함). 실제 실행 파일(계정 서버+릴레이+CLI)로 미로그인 거부·로그인 연결·사용량 쌓임 확인. 진행중
- 2026-11-06 F33 — 시작 (브랜치 `f33-relay-auth`)
- 2026-11-06 F32 — 사용자 승인, PR #27 머지, 완료 처리
- 2026-11-06 F31 — `v0.1.0` 태그(처음엔 옛 커밋에 붙어 Release가 안 돌아 다시 붙임) → Releases에 파일 5개(exe·dmg·deb·AppImage·apk). 남은 것: 폰·PC에 설치해 보기(`todo/F31-확인.md` 4절)
- 2026-11-06 F32 — 시험 페이지 끔(Pi), PR #27 CI 4개 작업 통과(세 OS + MariaDB). 확인대기
- 2026-11-06 F32 — Pi(MariaDB) 확인 통과: 실제 구글 로그인·기기 등록, 컨테이너 `down`/`up` 뒤 같은 계정·기기, DB 클라이언트로 3307 접속(tailscale serve TCP). 시험 중 수정: 0002 정렬 규칙(BINARY로 읽히던 문자열 칸), 마이그레이션 재컴파일(`build.rs`), DB 데이터를 Pi 폴더로. 남은 것: PR CI
- 2026-11-06 F32 — PC(메모리 저장소)에서 실제 구글 계정으로 시험 페이지 확인 통과(로그인·내 계정·기기 등록·갱신 2번·로그아웃·테스트 사용자 외 차단). 시험 페이지 상대 경로 404 수정. 남은 것: CI `account (MariaDB)`, (선택) Pi 배포
- 2026-11-06 F32 — `account/`(axum + MariaDB `sqlx`, 메모리 저장소 겸용): 구글 ID 토큰 검증(JWKS 캐시·`aud`·`iss`·만료·이메일 확인), 계정 upsert, 접속 토큰(EdDSA JWT 1시간, `GET /v1/keys`), 갱신 토큰(해시만 저장·교체·재사용 시 계정 전체 무효화), `/v1/me`·기기 등록, 개발용 로그인·시험 페이지, 마이그레이션 `0001_init.sql`, `deploy/compose.account.yml`·`Dockerfile.account`, CI `account (MariaDB)`. 테스트 18개 통과. 진행중
- 2026-11-06 M7 — Apple 로그인 다시 포함(App Store 지침 4.8, 사용자 결정). iOS는 F36, 데스크톱·Android는 도메인 뒤 F41
- 2026-11-06 M7 — 사용자 결정 D-019(구글 로그인 필수, 계정 서버에 계정·기기·사용량 저장, 클립보드 내용 암호화 유지, iOS는 Apple 로그인 포함). F32~F40 명세(04-features), 원칙 3·7 수정(01-overview). 개발·시험은 Pi, 도메인·클라우드는 F40 전
- 2026-11-06 F31 — PR #24 머지. 첫 Release 시험 실행에서 Android NDK 경로 문제(`env` 컨텍스트는 러너 변수를 못 읽음) 수정 후 4개 작업 모두 성공. 다음: 수정 PR 머지 → `v0.1.0` 태그
- 2026-11-05 F31 — 배포 워크플로 `release.yml`(v* 태그 → Releases: Windows NSIS, macOS 유니버설 dmg, Linux deb·AppImage, Android 서명 APK), Android 배포 서명 설정(환경 변수, 없으면 디버그 키), 배포 키 생성(저장소 밖), 문서 `docs/07-release.md`. Windows에서 `tauri build`(NSIS)·`assembleRelease`(배포 키 서명 확인) 성공. 진행중
- 2026-11-05 F20 — 보류(사용자 결정, D-017). O-01은 기본안 A(D-016), iOS 번들 ID `app.clipferry.ios`(O-03)
- 2026-11-05 F19 — PR #23 머지(세 OS CI 통과), 완료 처리
- 2026-11-05 F19 — 7번(직접 공유 바로가기) 실기기 확인 통과. 확인대기
- 2026-11-05 F19 — 공유 시트 맨 윗줄에 직접 공유 바로가기 `다른 기기로` 추가(`ShareShortcut`, `res/xml/shortcuts.xml` share-target → `ShareActivity`). Chrome 이미지 메뉴에는 끼어들 수 없어 대신 공유 시트에서 찾는 수고를 줄임. 진행중
- 2026-11-05 F19 — 실기기 확인 통과(1~6번, 삼성 One UI). 텍스트 선택 메뉴가 intent-filter 이름 대신 액티비티 이름(`clip-ferry`)을 보여 `PROCESS_TEXT`를 별칭 액티비티 `ProcessTextAlias`(`PC로 보내기`)로 옮김. 확인대기
- 2026-11-05 F19 — 투명 `ShareActivity`(`ACTION_SEND` 텍스트·이미지, `SEND_MULTIPLE` 이미지, `PROCESS_TEXT` "PC로 보내기"; 이미지는 액티비티가 떠 있는 동안 `cache/outgoing/`에 복사, 설정 변경으로 다시 만들어지지 않게 함), `SendClipboard.sendBatch`(한 건씩 차례로, 실패해도 나머지 계속, 결과 토스트 하나, 보낸 파일 삭제), 앱 시작 시 남은 파일 정리. 코어·FFI 변경 없음. 진행중
- 2026-11-05 F18 — PR #22 머지(세 OS CI 통과), 완료 처리
- 2026-11-05 F18 — 실기기 확인 통과(1~6번, 삼성 One UI). 타일이 패널 맨 뒤에 추가되고 보조 글씨가 안 보이는 것은 문서에 반영. 확인대기
- 2026-11-05 F18 — FFI `send_text`·`send_image`(형식·크기는 머리말로, 세션은 `Arc`로 꺼내 잠금을 오래 쥐지 않음), FFI 테스트 +1(폰 → PC 텍스트·이미지와 오류). Android: 투명 `SendActivity`(초점 받을 때 클립보드 읽기, 전환 효과 없음), `SendTileService`(안드로이드 14+ `PendingIntent`로 열기, 잠금 시 해제 요청), 상시 알림 `클립보드 보내기`, `SendClipboard`(받은 내용 되돌려 보내지 않음, 민감 표시 설정, 이미지 권한 실패·20 MiB 초과 안내, HEIC 등은 JPEG로, 꺼져 있으면 연결 켜고 8초 대기), 메인 화면 `보내기` 카드(타일 추가 요청·지금 보내기·민감한 내용 설정). 진행중
- 2026-11-05 F17 — PR #21 머지(세 OS CI 통과), 완료 처리
- 2026-11-05 F17 — 실기기 확인 통과(1~6번). 시험 중 문제 없음(같은 내용 재복사는 데스크톱 에코 방지로 안 가는 것이 정상). 확인대기
- 2026-11-05 F17 — FFI `CoreEvents`에 `on_text`·`on_image`(이미지 형식은 `transfer::sniff`로 다시 확인, 모르면 버림), FFI 테스트 +1(실제 릴레이로 PC → 폰 텍스트·이미지). Android: `ReceivedClips`(텍스트 그대로, 이미지는 `cache/received/` + `FileProvider` URI, 최근 1개만 유지, `EXTRA_IS_REMOTE_DEVICE`, 클립보드 쓰기 실패 시 알림), 받으면 알림(설정, 내용 없이 기기 이름·종류만), 메인 화면 `받은 클립보드` 카드. 진행중
- 2026-11-05 F16 — PR #20 머지(세 OS CI 통과; macOS에서 드러난 relay `group` 테스트 경쟁 상태 수정 포함), 완료 처리
- 2026-11-05 F16 — 실기기 확인 통과(1~6번). 시험 중 막힌 것은 폰 Tailscale 꺼짐뿐(릴레이 연결 대기 시간 제한 없음은 개선 후보로 기록). 확인대기
- 2026-11-04 F16 — FFI에 페어링(`start_invite`·`decide_invite`·`start_join`·`cancel_pairing`, 늦은 알림 버림)·`devices`(온라인 표시)·`leave_group`·`qr_matrix`, FFI 테스트 10개(실제 릴레이로 QR 참여·승인·서로 온라인, 코드 참여·거절, 취소, 나가기). Android: 자동 그룹 생성 제거, 첫 실행 화면(이름·QR로 참여·코드 입력·새 그룹), CameraX + ML Kit(번들) QR 스캔, 카메라 거부 시 코드 입력 안내, 초대 화면(QR·코드·남은 시간)·확인 숫자 승인/거절, 기기 목록(읽기 전용), 혼자/제거된 그룹 나가기. QR 참여 시 QR의 릴레이 주소를 저장. 진행중
- 2026-11-04 F15 — PR #19 머지(세 OS CI 통과), 완료 처리
- 2026-11-03 F15 — 실기기 확인 통과. 시험 중 수정: `저장하고 다시 연결` 버튼 항상 활성(중지 뒤 재연결 불가 해소), 회선이 모두 사라지면 바로 `재연결 중` 표시, VPN(Tailscale)을 뺀 실제 회선만 감시. 회선 소실 표시는 약 15초 지연(허용). 확인대기
- 2026-11-02 F15 — `ffi/`(UniFFI 바인딩, D-015)·`tools/uniffi-bindgen`, `scripts/build-android-core`(.ps1/.sh), `apps/android`(Kotlin·Compose, Keystore 비밀 저장소, 포그라운드 서비스, 네트워크 변경 시 재연결, 배터리 최적화 안내, 릴레이 주소 설정), FFI 테스트 4개, `assembleDebug` 성공. 에뮬레이터는 PC 메모리 부족으로 접고 실기기 확인 대기. 진행중
- 2026-11-02 F12 — 사용자 Windows 직접 확인, 완료 처리(예전 완료 커밋이 머지 뒤 브랜치에만 남아 main에 빠져 있던 것을 반영). macOS 미확인
- 2026-11-02 F13 — 재부팅 후 앱 자동 시작 사용자 확인(Windows). macOS는 미확인
- 2026-11-02 F14·F14b — 사용자 확인, main 머지(PR #18), 완료 처리
- 2026-11-02 F14b — 릴레이 `--trust-proxy`(`CF_RELAY_TRUST_PROXY`): 루프백·사설 주소에서 직접 온 연결만 `X-Forwarded-For` 신뢰(가장 오른쪽 값), 단위 테스트 6·통합 2, compose·README에 `tailscale serve` wss 절차, Pi에서 serve가 인증서·XFF를 붙이는 것 확인
- 2026-11-02 F14 — 클라이언트 `wss://`(rustls), `deploy/`(Dockerfile·compose·README), D-014(Pi + Tailscale 먼저), Pi 5에서 사용자 시험 통과, 인증서·`wss://` 종단·프록시 IP는 F14b로 분리, F15 선행을 F14b로, 확인대기
- 2026-11-01 F13 — 사용자 직접 확인(Windows 실제 앱+CLI), 완료 처리. 재부팅 후 자동 시작·macOS 미확인, CLI에 그룹 나가기 명령 없음(제거된 프로필은 지우고 init부터)은 알려진 사항
- 2026-11-01 F12 — core/transfer(청크 분할·조립·해시·60초 타임아웃·기기당 동시 2개·머리말 판별), `seal_image`/`Session::send_image`(속도 제한 재시도)/`EventSink::on_image`/`DropReason::BadImage`, CLI `send-image`·`listen` 이미지 저장(./received), 데스크톱 이미지 감지·쓰기(arboard)·PNG 인코딩/안전한 디코딩(image)·20 MiB 초과 알림(notification 플러그인)·쓴 직후 변경 무시, 코어 테스트 +25·릴레이 통합 4·데스크톱 +11, 실제 앱+CLI 14항목, 확인대기
- 2026-10-31 F11 정책 변경 — 모바일↔데스크톱 비밀번호 옮기기를 위해 민감 표시가 붙은 복사도 기본 동기화. 설정 `sync_sensitive`(기본 true, 끄면 기존처럼 읽지도 보내지도 않음), 설정 없던 파일은 기본값, 테스트 +2, 실제 앱+CLI 재검증, 문서(02·04) 반영, F11 완료
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
