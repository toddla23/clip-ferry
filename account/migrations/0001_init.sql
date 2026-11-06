-- clip-ferry 계정 서버 초기 스키마 (F32, D-019)
-- 저장하는 것: 계정 기본정보, 계정에 묶인 기기 목록, 갱신 토큰의 해시, 일별 사용량.
-- 클립보드 내용·그룹 키·기기 개인키는 어디에도 두지 않는다.
-- 시각은 모두 Unix 밀리초(BIGINT).

CREATE TABLE users (
    id            CHAR(32)      NOT NULL,             -- 무작위 16바이트 hex
    provider      VARCHAR(16)   NOT NULL,             -- 'google' (나중에 'apple', 개발용 'dev')
    provider_sub  VARCHAR(255)  NOT NULL,             -- 제공자의 사용자 ID (구글 `sub`)
    email         VARCHAR(320)  NOT NULL,
    name          VARCHAR(255)  NOT NULL DEFAULT '',
    picture_url   VARCHAR(2048) NOT NULL DEFAULT '',
    created_at_ms BIGINT        NOT NULL,
    last_login_ms BIGINT        NOT NULL,
    PRIMARY KEY (id),
    UNIQUE KEY users_provider_sub (provider, provider_sub)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_bin;

CREATE TABLE devices (
    user_id       CHAR(32)     NOT NULL,
    device_id     CHAR(32)     NOT NULL,              -- 코어의 device_id(16바이트) hex
    name          VARCHAR(255) NOT NULL,
    platform      VARCHAR(16)  NOT NULL,
    created_at_ms BIGINT       NOT NULL,
    last_seen_ms  BIGINT       NOT NULL,
    PRIMARY KEY (user_id, device_id),
    CONSTRAINT devices_user FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_bin;

CREATE TABLE refresh_tokens (
    token_hash    BINARY(32) NOT NULL,                -- SHA-256(토큰). 원문은 저장하지 않는다
    user_id       CHAR(32)   NOT NULL,
    family        CHAR(32)   NOT NULL,                -- 한 번의 로그인에서 이어지는 토큰 묶음
    created_at_ms BIGINT     NOT NULL,
    expires_at_ms BIGINT     NOT NULL,
    used_at_ms    BIGINT     NULL,                    -- 교체에 쓰인 시각 (다시 쓰이면 재사용)
    revoked       TINYINT(1) NOT NULL DEFAULT 0,
    PRIMARY KEY (token_hash),
    KEY refresh_tokens_user (user_id),
    KEY refresh_tokens_family (family),
    KEY refresh_tokens_expires (expires_at_ms),
    CONSTRAINT refresh_tokens_user FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_bin;

-- 릴레이가 보내는 계정별 사용량 (F33에서 채운다). 내용·채널·태그는 없다.
CREATE TABLE usage_daily (
    user_id  CHAR(32) NOT NULL,
    day      INT      NOT NULL,                       -- Unix 시간 기준 일 번호 (UTC)
    messages BIGINT   NOT NULL DEFAULT 0,
    bytes    BIGINT   NOT NULL DEFAULT 0,
    PRIMARY KEY (user_id, day),
    CONSTRAINT usage_daily_user FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_bin;
