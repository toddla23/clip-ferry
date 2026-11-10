#!/bin/sh
# 가장 최근 백업 파일을 표준 출력으로 내보낸다 (F40). Pi의 백업 전용 키가 이 명령만 실행하게 묶는다:
#   ~/.ssh/authorized_keys 한 줄:
#   restrict,command="/home/cf/clip-ferry/deploy/backup-send.sh" ssh-ed25519 AAAA... clip-ferry-pi-backup
# 그 키로는 셸·포트 전달·파일 쓰기가 안 되고, 이 파일 내용만 받아 갈 수 있다.
set -eu
LATEST=$(ls -1 /var/backups/clip-ferry/account-*.sql.gz 2>/dev/null | sort | tail -n 1)
[ -n "$LATEST" ] || { echo "no backup yet" >&2; exit 1; }
cat "$LATEST"
