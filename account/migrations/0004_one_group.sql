-- 계정 하나당 그룹 하나(F44, D-027).
-- in_group: 이 기기가 그룹에 들어 있다고 알렸다(그룹 ID·키는 모른다).
-- retired: 다른 기기에서 "처음부터 다시 시작"을 해 예전 그룹을 떠나야 하는 기기.
ALTER TABLE devices
    ADD COLUMN in_group TINYINT(1) NOT NULL DEFAULT 0,
    ADD COLUMN retired  TINYINT(1) NOT NULL DEFAULT 0;
