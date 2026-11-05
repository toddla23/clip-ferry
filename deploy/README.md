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

## iOS 푸시 켜기 (APNs, F22)

앱이 꺼진 아이폰에도 PC 클립이 알림으로 가게 하려면 릴레이에 APNs 키를 준다. 릴레이는 토큰을 저장하지 않고, 대상 기기가 오프라인일 때만 Apple에 알림을 보낸다(03-protocol 5절).

1. Apple 개발자 계정 → Certificates, Identifiers & Profiles → **Keys** → `+` → "Apple Push Notifications service (APNs)" 체크 → 만들고 `AuthKey_<Key ID>.p8`을 받는다(한 번만 받을 수 있다). Key ID와 팀 ID(오른쪽 위 계정 이름 아래)를 적어 둔다.
2. Pi로 옮기고 컨테이너가 읽을 수 있게 한다.
   ```sh
   mkdir -p ~/apns && mv AuthKey_*.p8 ~/apns/ && chmod 644 ~/apns/AuthKey_*.p8
   ```
3. `deploy/.env`에 더한다.
   ```sh
   cat >> deploy/.env <<'ENV'
   CF_APNS_KEY_FILE=/home/<사용자>/apns/AuthKey_XXXXXXXXXX.p8
   CF_APNS_KEY_ID=XXXXXXXXXX
   CF_APNS_TEAM_ID=YYYYYYYYYY
   CF_APNS_SANDBOX=true
   ENV
   ```
   - Xcode에서 직접 설치한 앱(개발 빌드)은 `CF_APNS_SANDBOX=true`, TestFlight·App Store 빌드는 `false`.
4. 겹쳐서 다시 띄운다.
   ```sh
   docker compose -f deploy/compose.yml -f deploy/compose.apns.yml up -d --build
   docker compose -f deploy/compose.yml logs | grep "push gateway"   # "push gateway: apns"가 보이면 켜진 것
   ```
- 1분마다 남는 집계 로그의 `pushed=`·`push_failed=`로 보낸 수·실패 수를 본다. 토큰·내용은 로그에 남지 않는다.

| 변수 (릴레이) | compose 변수 | 설명 |
|---|---|---|
| `CF_RELAY_APNS_KEY` | `CF_APNS_KEY_FILE` | `.p8` 키 파일 경로 |
| `CF_RELAY_APNS_KEY_ID` | `CF_APNS_KEY_ID` | 키 ID |
| `CF_RELAY_APNS_TEAM_ID` | `CF_APNS_TEAM_ID` | 팀 ID |
| `CF_RELAY_APNS_TOPIC` | `CF_APNS_TOPIC` | 앱 번들 ID (기본 `app.clipferry.ios`) |
| `CF_RELAY_APNS_SANDBOX` | `CF_APNS_SANDBOX` | 샌드박스 서버로 보낸다 (개발 빌드) |
