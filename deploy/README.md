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

구글 로그인을 검증하고 릴레이 접속 토큰을 발급하는 서버와 MariaDB를 같은 Pi에 올린다. 릴레이와 별도의 compose 파일(`compose.account.yml`)이다. MariaDB 데이터는 Docker 볼륨 `account-db`에 남고, 포트는 밖으로 열지 않는다.

### 1. 구글 클라이언트 ID (한 번만)

1. https://console.cloud.google.com/ → 새 프로젝트(예: `clip-ferry`)
2. **API 및 서비스 → OAuth 동의 화면**(Google Auth Platform): 사용자 유형 **외부**, 앱 이름 `clip-ferry`, 지원 이메일. 게시 상태는 **테스트**로 둔다 → **대상(테스트 사용자)**에 로그인할 구글 계정을 넣는다(100명까지, 도메인·방침 URL 없이 된다)
3. **사용자 인증 정보 → 사용자 인증 정보 만들기 → OAuth 클라이언트 ID → 웹 애플리케이션**
   - 승인된 JavaScript 원본: `https://<기기>.<tailnet>.ts.net:8443` (Pi), `http://localhost:8788` (PC에서 시험)
   - 리디렉션 URI는 비워 둔다
4. 만들어진 **클라이언트 ID**(`…apps.googleusercontent.com`)를 적는다. 클라이언트 보안 비밀은 쓰지 않는다
   - 데스크톱·Android·iOS 클라이언트 ID는 F34~F36에서 더한다(`CF_GOOGLE_CLIENT_IDS`에 쉼표로)

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

### 3. `https://…:8443`으로 열기

```bash
sudo tailscale serve --bg --https=8443 http://127.0.0.1:8788
curl https://<기기>.<tailnet>.ts.net:8443/healthz      # ok
```

시험 페이지: `https://<기기>.<tailnet>.ts.net:8443/dev/google` (`CF_DEV_PAGES=true`일 때). 시험이 끝나면 `CF_DEV_PAGES=false`.

### 백업

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
| `CF_ACCOUNT_DEV_PAGES` | false | `/dev/google` 시험 페이지 (compose는 `CF_DEV_PAGES`) |
| `CF_ACCOUNT_DEV_LOGIN` | false | 구글 없이 이메일만으로 계정을 만드는 `POST /v1/auth/dev` (개발 전용, compose는 `CF_DEV_LOGIN`) |

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
