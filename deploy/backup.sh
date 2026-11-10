#!/bin/sh
# 계정 DB 백업 (F40). 공개 서버에서 매일 cron으로 돌린다.
#   0 3 * * * /home/<사용자>/clip-ferry/deploy/backup.sh >> /var/backups/clip-ferry/backup.log 2>&1
# 덤프는 /var/backups/clip-ferry/account-<UTC 날짜>.sql.gz, 14일 지난 것은 지운다.
# 계정 이메일 등 개인정보가 들어 있으므로 폴더 700·파일 600으로 둔다.
set -eu

REPO=$(cd "$(dirname "$0")/.." && pwd)
DIR=/var/backups/clip-ferry
KEEP_DAYS=14

umask 077
mkdir -p "$DIR"
OUT="$DIR/account-$(date -u +%Y%m%d).sql.gz"
TMP="$OUT.tmp"

docker compose --env-file "$REPO/deploy/.env.cloud" -f "$REPO/deploy/compose.cloud.yml" exec -T mariadb \
  sh -c 'mariadb-dump --single-transaction -u clipferry -p"$MARIADB_PASSWORD" clipferry' | gzip > "$TMP"

# 덤프가 끝까지 됐는지 확인한 뒤에만 바꿔 넣는다(마지막 줄 "-- Dump completed")
if gzip -dc "$TMP" | tail -n 1 | grep -q 'Dump completed'; then
  mv "$TMP" "$OUT"
  echo "$(date -u +%FT%TZ) ok $OUT $(wc -c < "$OUT") bytes"
else
  rm -f "$TMP"
  echo "$(date -u +%FT%TZ) FAILED: dump incomplete" >&2
  exit 1
fi

find "$DIR" -name 'account-*.sql.gz' -mtime +"$KEEP_DAYS" -delete
