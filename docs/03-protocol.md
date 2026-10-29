# 03. 프로토콜

> 이 문서의 `(초안)` 표시는 아직 구현으로 확정되지 않은 부분이다. 해당 기능을 구현하면서 확정하고 표시를 지운다.
> 와이어 포맷의 원본은 `proto/clip_ferry.proto`이며(F03에서 생성), 이 문서와 항상 일치해야 한다.

모든 다중 바이트 정수는 little-endian. `||`는 바이트 이어붙이기. HKDF는 HKDF-SHA256, salt 없음(명시한 경우 제외).

---

## 1. 키와 식별자 (초안)

### 1.1 기기 키

각 기기는 처음 실행 시 두 키 쌍을 만든다.

- `sign_key`: Ed25519 — 모든 페이로드 서명
- `dh_key`: X25519 — 다른 기기가 이 기기에게 비밀(그룹 키)을 보낼 때 사용
- `device_id = SHA-256(sign_pub)[0..16]` (16바이트)

### 1.2 그룹 키와 파생값

- `group_key`: 32바이트 무작위. 에포크마다 새로 생성.
- `epoch`: u64, 그룹 생성 시 1.

| 이름 | 파생 | 길이 | 누가 보나 |
|---|---|---|---|
| `content_key` | `HKDF(group_key, info="clip-ferry v1 content" ‖ epoch)` | 32 | 구성원 |
| `channel_id` | `HKDF(group_key, info="clip-ferry v1 channel" ‖ epoch)` | 32 | 구성원 + **릴레이** |
| `device_tag(d)` | `HKDF(group_key, info="clip-ferry v1 tag" ‖ epoch ‖ device_id(d))` | 16 | 구성원 + **릴레이** |

- `channel_id`는 추측 불가능한 값이므로 "아는 것 = 구성원"이라는 능력(capability)으로 쓴다. 릴레이는 구성원 목록을 몰라도 된다.
- 기기를 제거하면 그룹 키가 바뀌므로 채널 ID도 바뀐다 → 제거된 기기는 새 채널을 모른다.
- 릴레이는 `device_id`를 보지 못하고, 에포크마다 바뀌는 `device_tag`만 본다.

### 1.3 기기 간 키 래핑 (`wrap_for`)

특정 기기에게만 비밀을 전달할 때 (환영, 키 회전).

```
eph = 새 X25519 키 쌍
shared = X25519(eph_priv, recipient_dh_pub)
k = HKDF(shared, salt = eph_pub ‖ recipient_dh_pub, info = "clip-ferry v1 wrap")
nonce = 무작위 24바이트
ct = XChaCha20-Poly1305(k, nonce, plaintext, aad = recipient_device_id)
Wrapped { eph_pub, nonce, ct }
```

래핑된 메시지는 항상 서명된 페이로드 안에 담겨 전달되므로 보낸 기기 인증은 서명으로 한다.

---

## 2. 봉투 (Envelope) (초안)

### 2.1 구조

```protobuf
// 릴레이에게 보이는 부분 (Publish.payload 바이트)
message SealedEnvelope {
  uint32 version = 1;        // 현재 1
  uint64 epoch   = 2;
  bytes  nonce   = 3;        // 24바이트, 매번 무작위
  bytes  ciphertext = 4;     // XChaCha20-Poly1305(content_key, nonce, SignedPayload, aad)
}
// aad = "clip-ferry/v1" ‖ channel_id ‖ epoch

// 복호화 후
message SignedPayload {
  bytes body      = 1;       // 직렬화된 PayloadBody
  bytes signature = 2;       // Ed25519(sender sign_key, "clip-ferry v1 sig" ‖ body)
}

message PayloadBody {
  bytes  sender_device_id = 1;   // 16
  bytes  msg_id           = 2;   // 16, 무작위
  uint64 created_at_ms    = 3;
  uint64 epoch            = 4;   // 봉투의 epoch와 같아야 함
  oneof kind {
    ClipText    text        = 10;
    ImageStart  image_start = 11;
    ImageChunk  image_chunk = 12;
    Control     control     = 20;
  }
}

message ClipText   { string text = 1; }
message ImageStart { bytes transfer_id = 1; string mime = 2; uint64 total_len = 3;
                     uint32 chunk_count = 4; bytes sha256 = 5; uint32 width = 6; uint32 height = 7; }
message ImageChunk { bytes transfer_id = 1; uint32 index = 2; bytes data = 3; }
```

### 2.2 수신 검증 순서 (하나라도 실패하면 조용히 버리고 카운터만 증가)

1. `version == 1`, `epoch`가 아는 에포크(현재 또는 유예 중인 이전 에포크)
2. AEAD 복호화 성공
3. `PayloadBody` 파싱 성공, `body.epoch == envelope.epoch`
4. `sender_device_id`가 해당 에포크 로스터에 있음
5. 서명 검증 성공
6. `msg_id`가 최근 4096개 중복 목록에 없음
7. `created_at_ms`가 현재 시각 ±10분 이내 (푸시 경로는 과거 1시간까지 허용)
8. 자기 자신이 보낸 메시지면 버림

### 2.3 이미지 전송

- 허용 MIME: `image/png`, `image/jpeg`, `image/gif`, `image/webp`. 데스크톱이 클립보드의 비트맵을 보낼 땐 PNG로 인코딩.
- 보낸 쪽: `ImageStart` 다음 `ImageChunk` 0..n-1 (청크 256 KiB). 각각 별도의 봉투.
- 받는 쪽: 메모리에서 조립, 전체 길이·청크 수·SHA-256 검증. 60초 안에 다 안 오면 폐기. 동시에 조립 중인 전송은 기기당 2개까지.
- 최대 20 MiB.

---

## 3. 그룹 관리 (초안)

### 3.1 로스터

```protobuf
message Member {
  bytes  device_id = 1; bytes sign_pub = 2; bytes dh_pub = 3;
  string name = 4; uint64 added_at_ms = 5;
  PushInfo push = 6;            // 선택. 플랫폼 + 토큰
}
message Roster { uint64 epoch = 1; uint64 version = 2; repeated Member members = 3; }

message Control {
  oneof kind {
    RosterUpdate roster_update = 1;   // 이름 변경, 푸시 토큰 갱신, 기기 추가 알림
    Rekey        rekey         = 2;
    EpochQuery   epoch_query   = 3;
    ResendRequest resend       = 4;   // 4.3 참고
  }
}
message RosterUpdate { Roster roster = 1; }
message Rekey { uint64 new_epoch = 1; Roster new_roster = 2; repeated WrappedKey keys = 3; }
message WrappedKey { bytes device_id = 1; bytes eph_pub = 2; bytes nonce = 3; bytes ct = 4; }
message EpochQuery { uint64 known_epoch = 1; }
message ResendRequest { bytes msg_id = 1; }
```

- 로스터 수정은 아무 구성원이나 할 수 있다(모든 기기가 동등). `version`은 1씩 증가.
- 받은 `RosterUpdate`는 `version`이 더 크면 채택. 같으면 `SHA-256(직렬화된 roster)`가 큰 쪽을 채택(동시 수정 충돌 해소).

### 3.2 기기 제거와 키 회전

제거를 실행하는 기기 R:

1. 새 `group_key'`, `epoch' = epoch + 1`, 제거 대상을 뺀 `roster'` 생성
2. 남은 각 기기에 대해 `wrap_for(member, group_key')` → `Rekey` 를 **이전 채널**에 게시
3. 자기도 새 에포크로 전환하고 새 채널 구독

받는 기기: 자기 몫을 풀어서 전환. 제거된 기기는 자기 몫이 없으므로 전환 불가.

### 3.3 이전 에포크 유예와 밀린 기기 복구

- 모든 기기는 이전 에포크 키를 **30일** 보관하고, 그 채널도 계속 구독한다(구독 수 제한 안에서 최근 것부터).
- 오프라인이었다가 돌아온 기기 D가 자기 에포크 채널에 연결하면 `EpochQuery{known_epoch}` 를 게시한다.
- 더 새 에포크를 가진 온라인 구성원은, D가 최신 로스터에 있으면 최신 키를 D에게 래핑한 `Rekey`(keys에 D만)를 응답한다. 여러 기기가 응답해도 무해(중복은 무시).
- 응답이 없으면 D는 재연결할 때마다 다시 묻는다. 30일이 지나도록 응답이 없으면 셸에 "그룹에서 제거되었거나 너무 오래 연결되지 않았습니다. 다시 페어링하세요"를 표시한다.

---

## 4. 페어링 (초안)

### 4.1 초대

기존 기기 A가 초대를 만든다.

- `invite_secret`: 32바이트 무작위. 유효 10분, 1회용.
- `pair_key = HKDF(invite_secret, info="clip-ferry v1 pair-key")`
- `pair_channel = HKDF(invite_secret, info="clip-ferry v1 pair-channel")`
- A는 `pair_channel`을 구독.

표현 형식:

- QR: `clipferry://pair/1/<base64url(invite_secret)>[?relay=<url-encoded wss URL>]`
- 텍스트 코드: Crockford base32로 52자, 4자씩 `-`로 묶음. 입력 시 대소문자·하이픈·공백 무시, `O→0`, `I/L→1` 보정. (짧은 코드는 열린 질문 O-02)

### 4.2 흐름

메시지는 `pair_key`로 봉인한 `PairEnvelope{nonce, ct}` (aad = `"clip-ferry/v1/pair" ‖ pair_channel`).

```
B → A  PairHello   { sign_pub_B, dh_pub_B, name_B }
A → B  PairOffer   { sign_pub_A, dh_pub_A, name_A }
       양쪽 화면에 SAS 표시:
       SAS = u32(SHA-256("clip-ferry v1 sas" ‖ invite_secret ‖ sign_pub_A ‖ sign_pub_B)[0..4]) mod 1_000_000  (6자리, 앞자리 0 유지)
       사용자가 A에서 "같음, 승인" 선택
A → B  PairWelcome { wrap_for(B, group_key ‖ epoch ‖ 직렬화된 roster(B 포함)) }  + 서명(A)
A → 그룹  RosterUpdate (B 추가)
```

- 같은 초대로 두 번째 `PairHello`가 오면 초대를 무효화하고 A에 경고를 띄운다.
- 사용자가 거절하거나 10분이 지나면 `PairReject` 전송 후 `pair_channel` 구독 해제.
- `PairWelcome`이 B의 공개키로 래핑되므로, 초대 코드를 엿본 제3자도 (SAS 승인이 없으면) 그룹 키를 얻지 못한다.

### 4.3 그룹 생성

첫 기기는 `group_key`, `epoch=1`, 자기만 있는 로스터를 만들고 채널을 구독한다.

---

## 5. 릴레이 프로토콜 (초안)

- 전송: WebSocket (`wss://`), 서브프로토콜 `clip-ferry.v1`, 바이너리 프레임에 protobuf.
- 프레임 최대 512 KiB.

```protobuf
message ClientFrame {
  oneof kind {
    Subscribe   subscribe   = 1;  // { bytes channel_id(32); bytes device_tag(16); }
    Unsubscribe unsubscribe = 2;  // { bytes channel_id; }
    Publish     publish     = 3;
    Ping        ping        = 4;
  }
}
message Publish {
  uint32 request_id = 1;
  bytes  channel_id = 2;
  bytes  payload    = 3;                  // SealedEnvelope 또는 PairEnvelope 직렬화 바이트
  repeated bytes to = 4;                  // 비어 있으면 같은 채널의 나 외 전원
  repeated PushTarget push = 5;           // 대상이 오프라인일 때만 사용
}
message PushTarget { bytes device_tag = 1; Platform platform = 2; string token = 3; }
enum Platform { PLATFORM_UNSPECIFIED = 0; APNS = 1; FCM = 2; }

message RelayFrame {
  oneof kind {
    ServerHello hello    = 1;  // { string server_version; Limits limits; }
    Deliver     deliver  = 2;  // { bytes channel_id; bytes from_tag; bytes payload; }
    Ack         ack      = 3;  // { uint32 request_id; uint32 delivered; uint32 pushed; }
    Error       error    = 4;  // { uint32 request_id; ErrorCode code; }
    Presence    presence = 5;  // { bytes channel_id; repeated bytes online_tags; }
    Pong        pong     = 6;
  }
}
enum ErrorCode { ERROR_UNSPECIFIED = 0; BAD_FRAME = 1; TOO_LARGE = 2; RATE_LIMITED = 3;
                 TOO_MANY_CHANNELS = 4; NOT_SUBSCRIBED = 5; UNSUPPORTED_VERSION = 6; PUSH_FAILED = 7; }
```

### 릴레이 규칙

- `Publish`는 그 연결이 해당 채널을 구독 중일 때만 허용 (`NOT_SUBSCRIBED`).
- 전달 대상이 없으면 **버린다.** 저장·재시도 없음. `Ack.delivered = 0`.
- `push` 대상 중 현재 그 채널에 해당 태그로 연결된 것이 없는 것만 APNs/FCM 전송. 토큰은 전송 후 즉시 잊는다.
- 푸시 페이로드는 `payload`를 base64로 담는다. 직렬화 후 3,000바이트를 넘으면 페이로드 없이 "도착 알림"만 보낸다(`need_open: true`).
- 연결 시 `ServerHello`로 제한값 전달. `Presence`는 구독 직후와 채널 구성 변화 시 전송.
- 핑: 클라이언트 25초마다 `Ping`, 60초 무응답이면 서버가 연결 종료.
- 로그: 연결 수, 프레임 수, 바이트 수, 에러 코드 집계만. 채널 ID·태그·페이로드·토큰은 절대 로그 금지.

---

## 6. 재전송 요청 (초안, 열린 질문 O-01과 연결)

푸시 알림에 내용을 다 못 담은 경우(긴 텍스트, 이미지):

- 보낸 기기는 **마지막으로 보낸 클립 1개**를 메모리에 10분간 보관한다(서버가 아니라 기기).
- 받는 기기는 사용자가 알림을 눌러 앱이 열리면 `ResendRequest{msg_id}`를 채널에 게시한다.
- 보낸 기기가 온라인이고 아직 보관 중이면 해당 기기에게만(`to`) 다시 보낸다.

---

## 7. 버전 관리

- `SealedEnvelope.version`, 서브프로토콜 이름, `invite` URI의 `/1/`이 각각의 버전.
- 필드 추가는 protobuf 호환 규칙으로. 의미가 바뀌는 변경은 버전을 올리고 `06-decisions.md`에 기록.
