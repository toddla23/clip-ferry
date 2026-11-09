-- 운영자 통계(F38): 날짜 범위로 모으는 질의에 쓰는 색인.
-- usage_daily의 기본 키는 (user_id, day)라 날짜만으로 찾을 때는 표 전체를 읽는다.
CREATE INDEX usage_daily_day ON usage_daily (day);
CREATE INDEX users_created ON users (created_at_ms);
