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

## 나중에: 인증서와 `wss://`

- **Tailscale 인증서**: `tailscale cert <기기>.<tailnet>.ts.net`으로 발급받아 TLS 종단(예: `tailscale serve`)을 앞에 둔다. 이 경우 릴레이는 프록시 뒤에 있어 클라이언트 IP가 모두 프록시 주소로 보인다 → IP당 연결 제한이 공유된다(구현 예정, 03-protocol 5절 구현 메모).
- **클라우드**(Fly.io 등): 호스팅이 TLS를 종단한다. 이 `Dockerfile`을 그대로 쓴다.
- 클라이언트(앱·CLI)는 이미 `wss://`를 지원한다(rustls, 번들 루트 인증서).

## 환경 변수

| 이름 | 기본값 | 뜻 |
|---|---|---|
| `CF_RELAY_LISTEN` | 컨테이너 `0.0.0.0:8787` | 바인드 주소 (직접 실행 시 `127.0.0.1:8787`) |
| `CF_RELAY_MAX_FRAME_BYTES` | 2 MiB | 프레임 최대 크기 |
| `CF_RELAY_MAX_CHANNELS` | 8 | 연결당 구독 채널 수 |
| `CF_RELAY_MAX_FRAMES_PER_SEC` | 50 | 연결당 초당 프레임 |
| `CF_RELAY_MAX_BYTES_PER_MIN` | 64 MiB | 연결당 분당 바이트 |
| `CF_RELAY_MAX_CONNS_PER_IP` | 32 | IP당 동시 연결 |
