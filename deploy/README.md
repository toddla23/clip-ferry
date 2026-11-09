# 릴레이 배포

릴레이는 상태를 디스크에 쓰지 않는다. 컨테이너는 읽기 전용 루트 파일시스템과 비루트 사용자로 돌고, 쓰기 가능한 볼륨이 없다.

## 라즈베리파이 + Tailscale (인증서 없이 시험)

전제: Pi(64비트 OS)에 Docker와 Tailscale이 설치되어 있고, 앱을 돌릴 PC도 같은 tailnet에 로그인되어 있다.
Tailscale 구간은 WireGuard로 암호화되므로 평문 `ws://`여도 구간은 보호된다. 클립보드 내용은 어차피 종단간 암호화다.
**`wss://`(TLS)는 서버에 인증서가 있어야 한다.** 인증서를 받으면 같은 방식으로 주소만 `wss://`로 바꾼다(아래 "나중에").

1. Pi에서 코드를 받는다.

   ```bash
   git clone https://github.com/toddla23/clip-ferry.git
   cd clip-ferry
   git checkout f14-relay-deploy   # main에 머지된 뒤에는 필요 없음
   ```

2. Pi의 Tailscale 주소를 확인하고, 그 주소에만 포트를 연다(집 LAN 등 다른 곳에는 열리지 않는다).

   ```bash
   export CF_BIND=$(tailscale ip -4)
   docker compose -f deploy/compose.yml up -d --build
   ```

   첫 빌드는 Pi 5에서 몇 분 걸린다.

3. 확인한다.

   ```bash
   curl http://$CF_BIND:8787/healthz     # ok
   docker compose -f deploy/compose.yml logs -f
   ```

   PC(PowerShell)에서도 같은 주소가 열려야 한다: `curl.exe http://<Pi의 Tailscale IP>:8787/healthz` → `ok`.

4. 앱이 쓸 릴레이 주소는 `ws://<Pi의 Tailscale IP>:8787`이다.
   - 데스크톱 앱: 창의 릴레이 주소 설정에 입력한다(또는 환경 변수 `CF_RELAY`).
   - CLI: `--relay ws://<Pi의 Tailscale IP>:8787`

5. 재부팅 후에도 자동으로 뜬다(`restart: unless-stopped`). 이 때 `CF_BIND`가 사라지므로 한 번 고정해 둔다.

   ```bash
   echo "CF_BIND=$(tailscale ip -4)" > deploy/.env
   ```

   (`deploy/.env`는 compose가 자동으로 읽는다. 커밋하지 않는다.)

### 업데이트와 중지

```bash
git pull && docker compose -f deploy/compose.yml up -d --build   # 업데이트
docker compose -f deploy/compose.yml down                        # 중지
```

## `wss://` 켜기 (Tailscale 인증서, 무료·도메인 불필요)

`tailscale serve`가 인증서(Let's Encrypt) 발급·갱신과 TLS 종단을 맡고, 릴레이는 같은 Pi의 `127.0.0.1`에서 평문으로 받는다. 접속 주소는 `wss://<기기>.<tailnet>.ts.net` 형태가 된다.

1. 한 번만: Tailscale 관리 콘솔 → DNS(https://login.tailscale.com/admin/dns)에서 `MagicDNS`가 켜져 있는지 보고 `HTTPS Certificates`를 `Enable HTTPS`로 켠다. 기기 이름이 공개 인증서 기록에 남는다는 안내에 동의해야 한다.
2. 릴레이를 `127.0.0.1`에만 열고 프록시 헤더를 신뢰하게 한다 (Tailscale IP로 직접 열 필요가 없어진다).

   ```bash
   printf 'CF_BIND=127.0.0.1\nCF_TRUST_PROXY=true\n' > deploy/.env
   docker compose -f deploy/compose.yml up -d --build
   ```

3. `tailscale serve`로 HTTPS 443을 릴레이에 연결한다 (재부팅 후에도 유지된다).

   ```bash
   sudo tailscale serve --bg --https=443 http://127.0.0.1:8787
   tailscale serve status        # 접속 주소가 나온다
   ```

4. 확인한다 (PC에서). 인증서 오류 없이 `ok`가 나와야 한다.

   ```powershell
   curl.exe -v https://<기기>.<tailnet>.ts.net/healthz
   ```

5. 앱·CLI 릴레이 주소를 `wss://<기기>.<tailnet>.ts.net`으로 바꾼다 (포트 없음).

끄려면 `sudo tailscale serve --https=443 off`.

### 프록시 뒤에서의 클라이언트 IP

- `tailscale serve`는 요청에 `X-Forwarded-For: <기기의 Tailscale IP>`를 붙인다. `CF_TRUST_PROXY=true`이면 릴레이가 이 값으로 IP당 연결 제한(`CF_RELAY_MAX_CONNS_PER_IP`)을 기기별로 센다. 꺼져 있으면 모든 기기가 `127.0.0.1` 하나로 세어진다.
- 헤더는 **루프백·사설 주소에서 직접 온 연결**에서만 믿고, 값이 여러 개면 가장 오른쪽을 쓴다(인터넷에서 온 연결이 위조해도 소용없다). 그래서 포트를 `127.0.0.1`에만 열어 둬야 한다. LAN이나 Tailscale IP로 같이 열면서 켜지 않는다.
- `tailscale serve`는 `Tailscale-User-Login`(계정 이메일), `Tailscale-User-Name`, `Tailscale-User-Profile-Pic` 헤더도 붙인다. 릴레이는 헤더를 읽지도 로그에 남기지도 않는다. 앞으로도 헤더를 로그에 쓰지 않는다.

## 계정 서버 (F32, D-019)

구글 로그인을 검증하고 릴레이 접속 토큰을 발급하는 서버와 MariaDB를 같은 Pi에 올린다. 릴레이와 별도의 compose 파일(`compose.account.yml`)이다. MariaDB 포트는 밖으로 열지 않는다.

**데이터 위치**: Pi의 폴더 `/var/lib/clip-ferry/mariadb`(바꾸려면 `.env.account`에 `CF_DB_DIR`). Docker 볼륨이 아니라서 컨테이너 재시작·삭제(`down`), 이미지 업데이트, `down -v`, `docker system prune --volumes`를 해도 남는다. 지워지는 것은 이 폴더를 직접 지울 때뿐이다. 폴더는 처음 기동 때 자동으로 만들어진다(MariaDB가 소유자를 바꾼다).

### 1. 구글 클라이언트 ID (한 번만)

1. https://console.cloud.google.com/ → 새 프로젝트(예: `clip-ferry`)
2. **API 및 서비스 → OAuth 동의 화면**(Google Auth Platform): 사용자 유형 **외부**, 앱 이름 `clip-ferry`, 지원 이메일. 게시 상태는 **테스트**로 둔다 → **대상(테스트 사용자)**에 로그인할 구글 계정을 넣는다(100명까지, 도메인·방침 URL 없이 된다)
3. **사용자 인증 정보 → 사용자 인증 정보 만들기 → OAuth 클라이언트 ID → 웹 애플리케이션**
   - 승인된 JavaScript 원본: `https://<기기>.<tailnet>.ts.net:8443` (Pi), `http://localhost:8788` (PC에서 시험)
   - 리디렉션 URI는 비워 둔다
4. 만들어진 **클라이언트 ID**(`…apps.googleusercontent.com`)를 적는다. 클라이언트 보안 비밀은 쓰지 않는다
   - **`CF_GOOGLE_CLIENT_IDS`의 첫 번째는 이 웹 클라이언트로 둔다.** Android 앱이 구글 로그인 창에 이 ID를 넘기고(Credential Manager의 `serverClientId`), 받은 ID 토큰의 `aud`도 이 값이다(F35). 다른 값을 쓰려면 `CF_GOOGLE_WEB_CLIENT_ID`
   - iOS 클라이언트 ID는 F36에서 더한다(`CF_GOOGLE_CLIENT_IDS`에 쉼표로)
5. **데스크톱 앱용 클라이언트 (F34)**: 사용자 인증 정보 만들기 → OAuth 클라이언트 ID → 유형 **데스크톱 앱**(이름 예: `clip-ferry desktop`). 리디렉션 주소는 넣지 않는다(구글이 `http://127.0.0.1:<포트>`를 알아서 받는다)
   - 만들어진 **클라이언트 ID**와 **클라이언트 보안 비밀**을 `.env.account`의 `CF_GOOGLE_DESKTOP_CLIENT_ID`, `CF_GOOGLE_DESKTOP_CLIENT_SECRET`에 넣는다. 보안 비밀은 계정 서버만 갖고 앱·저장소에는 넣지 않는다(D-020). `CF_GOOGLE_CLIENT_IDS`에는 따로 넣지 않아도 된다(서버가 더한다)
6. **Android 앱용 클라이언트 (F35)**: 사용자 인증 정보 만들기 → OAuth 클라이언트 ID → 유형 **Android**. 패키지 이름 `app.clipferry.android`, **SHA-1 인증서 지문**은 APK에 서명한 키의 것이다. 키마다 하나씩 만든다(디버그 키·배포 키)
   - 디버그 키(Android Studio·`gradlew installDebug`로 설치한 앱): `apps/android`에서 `.\gradlew.bat signingReport` → `Variant: debug` 아래 `SHA1:` 줄. (`keytool -list -v`는 한국어 Windows에서 `IllegalFormatConversionException`으로 멈출 수 있다)
   - 배포 키(GitHub Releases APK, D-018): 배포 키 환경 변수(`CLIPFERRY_KEYSTORE` 등, `docs/07-release.md`)를 둔 채 `signingReport`의 `Variant: release` 줄
   - 이 클라이언트 ID는 **서버 설정에 넣지 않는다.** 앱 코드에도 없다. 구글이 "이 패키지·서명의 앱이 이 프로젝트 소속인지" 확인하는 데만 쓴다. 없거나 SHA-1이 틀리면 앱에서 `Developer console is not set up correctly` 오류가 난다

### 2. 설정 파일과 기동 (Pi)

```bash
cd ~/clip-ferry && git pull
cat > deploy/.env.account <<EOF
CF_DB_PASSWORD=$(openssl rand -hex 24)
CF_ACCOUNT_SIGNING_KEY=$(openssl rand 32 | base64 | tr '+/' '-_' | tr -d '=')
CF_GOOGLE_CLIENT_IDS=<1-4의 클라이언트 ID>
CF_DEV_PAGES=true
EOF
chmod 600 deploy/.env.account
docker compose --env-file deploy/.env.account -f deploy/compose.account.yml up -d --build
docker compose --env-file deploy/.env.account -f deploy/compose.account.yml logs account   # listening on 0.0.0.0:8788
```

- `.env.account`는 git에 넣지 않는다(`.gitignore`). **서명 키를 바꾸면** 이미 발급한 접속 토큰이 모두 무효가 된다(앱은 갱신 토큰으로 새로 받는다). DB 암호는 볼륨을 처음 만들 때 정해지므로 나중에 바꾸려면 MariaDB 안에서 바꾼다.
- 처음 빌드는 Pi에서 10~20분 걸린다.
- 이 compose는 프로젝트 이름 `clip-ferry-account`를 쓴다(릴레이는 `deploy`). 그전에 `deploy` 프로젝트로 올린 계정 서버가 있으면 `network deploy_default has active endpoints` 오류가 난다. 옛 컨테이너를 지우고 다시 올린다(데이터는 Pi 폴더라 남는다): `docker rm -f deploy-account-1 deploy-mariadb-1`

### 3. `https://…:8443`으로 열기

```bash
sudo tailscale serve --bg --https=8443 http://127.0.0.1:8788
curl https://<기기>.<tailnet>.ts.net:8443/healthz      # ok
```

시험 페이지: `https://<기기>.<tailnet>.ts.net:8443/dev/google` (`CF_DEV_PAGES=true`일 때). 시험이 끝나면 `CF_DEV_PAGES=false`.

### 운영자 페이지 (F38)

가입자 수·활성 사용자·플랫폼별 기기·일별 전송량을 브라우저로 본다. 허용 목록의 구글 계정으로만 들어온다.

```bash
echo "CF_ADMIN_EMAILS=you@gmail.com" >> deploy/.env.account   # 여러 명이면 쉼표로
docker compose --env-file deploy/.env.account -f deploy/compose.account.yml up -d
```

- 주소: `https://<기기>.<tailnet>.ts.net:8443/admin`. 로그인 버튼은 1절의 웹 클라이언트를 쓰므로, 그 클라이언트의 승인된 JavaScript 원본에 이 주소(`https://<기기>.<tailnet>.ts.net:8443`)가 있어야 한다.
- 비우면 `/admin`은 404다. 목록에서 빼면 이미 로그인한 운영자도 다음 요청부터 403이다(운영자 토큰은 1시간).
- 클립보드 내용은 서버에 없어 이 페이지에서도 볼 수 없다. 사용자별로는 이메일·이름·기기 목록·사용량(횟수·바이트)까지 보인다.

### DB 클라이언트로 접속 (HeidiSQL·DBeaver 등)

MariaDB는 Pi의 `127.0.0.1:3307`에만 열려 있다(포트는 `.env.account`의 `CF_DB_PORT`). 다른 PC에서는 `tailscale serve`로 **tailnet 안에서만** 연다. 공유기 포트포워딩이나 `0.0.0.0` 바인드는 하지 않는다(계정 이메일 등 개인정보가 있다).

```bash
sudo tailscale serve --bg --tcp 3307 tcp://127.0.0.1:3307
tailscale serve status          # |-- tcp://raspberry-pi.…:3307 → tcp://127.0.0.1:3307
grep CF_DB_PASSWORD deploy/.env.account   # 접속 암호
```

클라이언트 설정: 호스트 `<기기>.<tailnet>.ts.net`(또는 Pi의 Tailscale IP), 포트 `3307`, 사용자 `clipferry`, 암호는 위 값, DB `clipferry`. 끄려면 `sudo tailscale serve --tcp 3307 off`.

- `clipferry` 사용자는 계정 서버가 쓰는 계정이라 모든 표를 고칠 수 있다. 행을 손으로 바꾸면 서버 동작이 꼬일 수 있으니 주로 조회에 쓴다. 갱신 토큰 표에는 해시만 있다.

### 백업

돌고 있는 DB의 폴더를 그대로 복사하면 깨질 수 있으므로 덤프로 뜬다. 매일 돌리려면 `crontab -e`에 같은 명령을 넣는다.

```bash
docker compose --env-file deploy/.env.account -f deploy/compose.account.yml exec mariadb \
  sh -c 'mariadb-dump -u clipferry -p"$MARIADB_PASSWORD" clipferry' > account-backup.sql
```

### 계정 서버 환경 변수

| 이름 | 기본값 | 뜻 |
|---|---|---|
| `CF_ACCOUNT_LISTEN` | 컨테이너 `0.0.0.0:8788` | 바인드 주소 (직접 실행 시 `127.0.0.1:8788`) |
| `CF_ACCOUNT_DATABASE_URL` | 없음 | `mysql://사용자:암호@호스트:3306/DB` (compose가 채운다) |
| `CF_ACCOUNT_MEMORY_STORE` | false | DB 없이 메모리 (개발 전용) |
| `CF_ACCOUNT_SIGNING_KEY` | 없음(필수) | 접속 토큰 서명 키, base64url 32바이트 (`clip-ferry-account gen-key`로도 만든다) |
| `CF_ACCOUNT_GOOGLE_CLIENT_IDS` | 없음 | 받아 줄 구글 클라이언트 ID, 쉼표로 여러 개 (compose는 `CF_GOOGLE_CLIENT_IDS`) |
| `CF_ACCOUNT_GOOGLE_WEB_CLIENT_ID` | 위 목록의 첫 번째 | Android 앱이 구글 로그인 창에 넘길 "웹 애플리케이션" 클라이언트 ID (F35, compose는 `CF_GOOGLE_WEB_CLIENT_ID`). `GET /v1/auth/google/android`로 알려 준다. 받아 줄 목록에도 더한다 |
| `CF_ACCOUNT_GOOGLE_DESKTOP_CLIENT_ID` | 없음 | 데스크톱 앱 구글 로그인용 "데스크톱 앱" 클라이언트 ID (F34, compose는 `CF_GOOGLE_DESKTOP_CLIENT_ID`). 비우면 데스크톱 로그인을 끈다 |
| `CF_ACCOUNT_GOOGLE_DESKTOP_CLIENT_SECRET` | 없음 | 그 클라이언트의 보안 비밀. ID와 함께 줘야 한다 (compose는 `CF_GOOGLE_DESKTOP_CLIENT_SECRET`) |
| `CF_ACCOUNT_DEV_PAGES` | false | `/dev/google` 시험 페이지 (compose는 `CF_DEV_PAGES`) |
| `CF_ACCOUNT_DEV_LOGIN` | false | 구글 없이 이메일만으로 계정을 만드는 `POST /v1/auth/dev` (개발 전용, compose는 `CF_DEV_LOGIN`) |
| `CF_ACCOUNT_ADMIN_EMAILS` | 없음 | 운영자 페이지 `/admin`에 들어올 구글 계정 이메일(쉼표). 비우면 `/admin`을 열지 않는다 (compose는 `CF_ADMIN_EMAILS`, F38) |
| `CF_ACCOUNT_USAGE_SECRET` | 없음 | 릴레이가 사용량을 보낼 때 쓰는 공유 비밀 (`POST /internal/usage`). 비우면 그 경로를 열지 않는다 (compose는 `CF_USAGE_SECRET`) |

## 릴레이가 접속 토큰을 확인하게 하기 (F33)

릴레이에 계정 서버 주소를 주면 **로그인한 기기(접속 토큰)만** 연결할 수 있다. 계정별 동시 연결 수·분당 바이트를 따로 세고, 계정별 게시 수·바이트(내용 없음)를 1분마다 계정 서버에 보낸다.

> 데스크톱 앱은 F34부터, Android 앱은 F35부터 로그인해야 동작한다. 켜기 전에 **쓰는 모든 기기(PC·폰)를 로그인하는 버전으로 업데이트**한다. 예전 버전 앱은 토큰이 없어 연결이 끊긴다.

켤 때 (Pi):

```bash
echo "CF_USAGE_SECRET=$(openssl rand -hex 24)" >> deploy/.env.account
# 계정 서버를 다시 올린다 (네트워크 이름 clip-ferry-account, 사용량 비밀)
docker compose --env-file deploy/.env.account -f deploy/compose.account.yml up -d
# 릴레이를 덧붙임과 함께 다시 올린다
docker compose --env-file deploy/.env.account -f deploy/compose.yml -f deploy/compose.relay-auth.yml up -d
docker compose -f deploy/compose.yml logs --tail 5 relay    # access tokens required (account server set)
```

끄려면 덧붙임 없이 `docker compose -f deploy/compose.yml up -d`.

## 클라우드로 옮길 때

- 호스팅(Fly.io 등)이 TLS를 종단하면 이 `Dockerfile`을 그대로 쓴다. 호스팅의 프록시가 어떤 헤더로 클라이언트 IP를 주는지 확인하고 `CF_TRUST_PROXY`를 정한다(현재는 `X-Forwarded-For`만 지원).
- 도메인·번들 ID·호스팅 결정은 `docs/06-decisions.md`의 O-03, O-04.

## 환경 변수

| 이름 | 기본값 | 뜻 |
|---|---|---|
| `CF_RELAY_LISTEN` | 컨테이너 `0.0.0.0:8787` | 바인드 주소 (직접 실행 시 `127.0.0.1:8787`) |
| `CF_RELAY_MAX_FRAME_BYTES` | 2 MiB | 프레임 최대 크기 |
| `CF_RELAY_MAX_CHANNELS` | 8 | 연결당 구독 채널 수 |
| `CF_RELAY_MAX_FRAMES_PER_SEC` | 50 | 연결당 초당 프레임 |
| `CF_RELAY_MAX_BYTES_PER_MIN` | 64 MiB | 연결당 분당 바이트 |
| `CF_RELAY_MAX_CONNS_PER_IP` | 32 | IP당 동시 연결 |
| `CF_RELAY_TRUST_PROXY` | false | 프록시 뒤에서 `X-Forwarded-For`로 IP를 센다 (compose는 `CF_TRUST_PROXY`) |
| `CF_RELAY_ACCOUNT_URL` | 없음 | 계정 서버 주소. 주면 접속 토큰이 있는 연결만 받는다 (F33). 비우면 누구나 접속 |
| `CF_RELAY_USAGE_SECRET` | 없음 | 사용량 보고 공유 비밀 (계정 서버의 `CF_ACCOUNT_USAGE_SECRET`과 같게). 비우면 보고하지 않는다 |
| `CF_RELAY_MAX_CONNS_PER_ACCOUNT` | 16 | 계정당 동시 연결 (넘으면 HTTP 429) |
| `CF_RELAY_MAX_BYTES_PER_MIN_PER_ACCOUNT` | 256 MiB | 계정당 분당 바이트 (넘으면 `RATE_LIMITED`) |
