# 07. 배포 파일: 다운로드와 배포하는 법

배포 파일은 GitHub Actions(`.github/workflows/release.yml`)가 만들어 **GitHub Releases**에 올린다.

| 기기 | 파일 | 비고 |
|---|---|---|
| Windows 10·11 (64비트) | `clip-ferry_<버전>_x64-setup.exe` | 설치 파일. 관리자 권한 없이 사용자 계정에 설치 |
| macOS | `clip-ferry_<버전>_universal.dmg` | Intel·Apple Silicon 공용 |
| Linux (Ubuntu·Debian 계열) | `clip-ferry_<버전>_amd64.deb` | |
| Linux (그 밖) | `clip-ferry_<버전>_amd64.AppImage` | 설치 없이 실행 |
| Android 8 이상 | `clip-ferry-<버전>-android.apk` | arm64 폰 + x86_64 에뮬레이터 |

iOS는 아직 배포 파일이 없다(M4 진행 중, App Store·TestFlight는 Apple 개발자 계정이 필요하다).

---

## 1. 다운로드와 설치 (쓰는 사람)

다운로드 주소: **https://github.com/toddla23/clip-ferry/releases/latest**

페이지 아래 **Assets**를 펼쳐 기기에 맞는 파일을 받는다.

> 지금 배포 파일은 **코드 서명이 없다**(인증서 비용 문제, F31 후속). 그래서 처음 열 때 운영체제가 경고를 띄운다. 아래 순서대로 열면 된다. 받은 파일은 이 저장소의 Releases에서 받은 것만 쓴다.

### Windows

1. `clip-ferry_<버전>_x64-setup.exe`를 받아 실행한다.
2. "Windows의 PC 보호" 파란 창이 뜨면 **추가 정보 → 실행**을 누른다.
3. 설치가 끝나면 작업 표시줄 트레이(시계 옆 ^ 안)에 clip-ferry 아이콘이 생긴다.

- 지우기: 설정 → 앱 → 설치된 앱 → clip-ferry → 제거.

### macOS

1. `clip-ferry_<버전>_universal.dmg`를 받아 열고, `clip-ferry`를 `Applications`(응용 프로그램)로 끌어다 놓는다.
2. 응용 프로그램에서 clip-ferry를 연다. "확인되지 않은 개발자" 또는 "열 수 없습니다" 창이 뜨면 **완료(또는 취소)**를 누른다.
3. 시스템 설정 → **개인정보 보호 및 보안** → 아래쪽 "clip-ferry이(가) 차단되었습니다" 옆 **그래도 열기** → 암호 입력.
4. "손상되었기 때문에 열 수 없습니다"가 나오면 터미널에서 한 번 실행한 뒤 다시 연다:
   ```sh
   xattr -dr com.apple.quarantine /Applications/clip-ferry.app
   ```
5. 메뉴 막대 오른쪽에 clip-ferry 아이콘이 생긴다. 클립보드 읽기를 위해 권한을 물으면 허용한다.

### Linux

- `.deb` (Ubuntu·Debian·Mint 등):
  ```sh
  sudo apt install ./clip-ferry_<버전>_amd64.deb
  ```
- `.AppImage` (그 밖의 배포판):
  ```sh
  chmod +x clip-ferry_<버전>_amd64.AppImage
  ./clip-ferry_<버전>_amd64.AppImage
  ```
- 트레이 아이콘이 안 보이면 데스크톱 환경의 앱 표시기(AppIndicator) 확장을 켠다(GNOME은 "AppIndicator and KStatusNotifierItem Support").

### Android

1. 폰 브라우저(Chrome 등)로 위 주소를 열고 `clip-ferry-<버전>-android.apk`를 받는다.
2. 다운로드 알림이나 `내 파일 → 다운로드`에서 APK를 누른다.
3. "출처를 알 수 없는 앱" 안내가 나오면 **설정 → 이 출처 허용**을 켜고 뒤로 돌아와 **설치**.
   - 삼성: 설정 → 애플리케이션 → 메뉴(⋮) → 특별한 접근 → 출처를 알 수 없는 앱 설치 → 사용한 브라우저 허용.
4. "Play 프로텍트" 경고가 뜨면 **세부정보 더보기 → 무시하고 설치**(Play 스토어에 올리지 않은 앱이라 뜬다).
5. 업데이트: 새 버전 APK를 받아 같은 방법으로 설치하면 덮어쓴다(데이터·페어링 유지). 같은 서명 키로 만든 APK끼리만 업데이트된다.

### 설치한 뒤

- 주소는 넣지 않아도 된다. 배포 빌드는 공개 서버에 바로 붙는다(F43). 구글로 로그인한다(지금은 구글 동의 화면에 등록된 테스트 사용자만 로그인된다).
  - 다른 서버(개발용·Pi)를 쓰려면 제목(`clip-ferry 로그인`·연결 상태 글자)을 3초 안에 7번 눌러 서버 설정을 연다.
- 첫 기기에서 그룹을 만들고, 나머지 기기는 QR이나 코드로 참여한다.
- Android는 배터리 최적화 제외 안내를 따르면 백그라운드에서도 계속 받는다.

---

## 2. 처음 한 번: 서명 키 등록 (배포하는 사람)

Android APK는 항상 **같은 키**로 서명해야 업데이트가 된다. 키는 저장소에 넣지 않고 GitHub Secrets에만 둔다.

- 키는 2026-11-05에 이 PC에서 만들었다: `%USERPROFILE%\.clip-ferry-release\`
  - `release.jks` — 키 파일 (별칭 `clip-ferry`, RSA 4096, 10000일)
  - `password.txt` — 키 암호
  - `release.jks.base64.txt` — Secrets에 넣을 base64 문자열
- **이 폴더를 백업한다**(USB·비밀번호 관리자 등). 키를 잃으면 이미 설치된 앱은 새 버전으로 업데이트할 수 없고, 지우고 다시 설치(다시 페어링)해야 한다. 키·암호를 git·채팅·메일에 올리지 않는다.

GitHub 저장소 → **Settings → Secrets and variables → Actions → New repository secret**에서 두 개를 만든다:

| 이름 | 값 |
|---|---|
| `ANDROID_KEYSTORE_BASE64` | `release.jks.base64.txt` 내용 전체 |
| `ANDROID_KEYSTORE_PASSWORD` | `password.txt` 내용 |

PowerShell에서 클립보드로 복사하기:

```powershell
Get-Content "$env:USERPROFILE\.clip-ferry-release\release.jks.base64.txt" -Raw | Set-Clipboard
Get-Content "$env:USERPROFILE\.clip-ferry-release\password.txt" -Raw | Set-Clipboard
```

로컬에서 서명된 APK를 만들 때는 환경 변수 `CLIPFERRY_KEYSTORE`(키 파일 경로)와 `CLIPFERRY_KEYSTORE_PASSWORD`를 주고 `./gradlew assembleRelease`. 둘이 없으면 디버그 키로 서명된다(시험용, 배포 금지).

## 3. 새 버전 배포 (배포하는 사람)

1. main에 배포할 내용이 다 머지되었는지 확인한다.
2. 태그를 붙여 푸시한다. 버전은 `v주.부.수` 꼴.
   ```powershell
   git checkout main
   git pull
   git tag v0.1.0
   git push origin v0.1.0
   ```
3. GitHub → **Actions → Release**가 돈다(20~30분). Windows·macOS·Linux·Android 4개 작업이 모두 초록이면 **Releases**에 `v0.1.0`이 생기고 파일이 붙는다.
   - 앱 버전은 태그에서 가져온다(데스크톱 `tauri.conf.json`의 `version`, Android `versionName`). Android `versionCode`는 실행 번호라 계속 커진다.
4. Releases 페이지에서 릴리스를 열어 **Edit**로 바뀐 점을 적는다.

- 시험판: `v0.2.0-rc1`처럼 `-`가 붙은 태그는 Releases에 **Pre-release**로 올라가고 `latest`(1절 주소)에는 잡히지 않는다.
- 릴리스 없이 빌드만 시험(이 워크플로가 main에 들어간 뒤부터): Actions → Release → **Run workflow**. 결과 파일은 그 실행 페이지 아래 **Artifacts**에서 받는다(로그인 필요, 90일 보관).
- 실패하면: 실패한 작업의 단계 이름을 보고, 같은 태그로 다시 하려면 태그를 지우고 다시 붙인다.
  ```powershell
  git push origin :refs/tags/v0.1.0
  git tag -d v0.1.0
  ```

## 4. 알려진 한계 (F31 후속)

- Windows 코드 서명, macOS 서명·공증이 없다 → 처음 열 때 경고(1절).
- 자동 업데이트가 없다. 새 버전은 다시 받아 설치한다.
- 앱의 기본 서버는 Azure 이름이다(F43, D-026). 서버를 옮기면 숨은 서버 설정에 새 주소를 넣거나 새 버전을 설치한다.
- Linux는 x86_64만, Android는 arm64·x86_64만.
