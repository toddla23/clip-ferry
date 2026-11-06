-- 0001에서 표의 정렬 규칙을 utf8mb4_bin으로 만들었더니, MariaDB가 CHAR·VARCHAR 칸을
-- 이진(BINARY) 형식으로 알려 sqlx가 문자열로 읽지 못했다
-- ("mismatched types; Rust type String is not compatible with SQL type BINARY").
-- 이진이 아닌 utf8mb4_unicode_ci로 바꾼다. ID·토큰 묶음은 소문자 hex라 대소문자 구분이 필요 없다.
-- (0001은 이미 적용된 DB가 있어 고치지 않는다.)
--
-- MariaDB는 다른 표가 참조하는 칸(users.id)의 정렬 규칙을 바꾸지 못하게 하므로
-- (FOREIGN_KEY_CHECKS = 0이어도 오류 1833), 외래 키를 지우고 바꾼 뒤 다시 만든다.

ALTER TABLE devices DROP FOREIGN KEY devices_user;
ALTER TABLE refresh_tokens DROP FOREIGN KEY refresh_tokens_user;
ALTER TABLE usage_daily DROP FOREIGN KEY usage_daily_user;

ALTER TABLE users CONVERT TO CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
ALTER TABLE devices CONVERT TO CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
ALTER TABLE refresh_tokens CONVERT TO CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
ALTER TABLE usage_daily CONVERT TO CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;

ALTER TABLE devices ADD CONSTRAINT devices_user
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE;
ALTER TABLE refresh_tokens ADD CONSTRAINT refresh_tokens_user
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE;
ALTER TABLE usage_daily ADD CONSTRAINT usage_daily_user
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE;
