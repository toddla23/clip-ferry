#!/bin/sh
# 공개 서버의 최근 백업을 Pi로 가져온다 (F40, 서버 밖 사본). Pi에서 매일 cron으로 돌린다:
#   30 3 * * * /home/<사용자>/clip-ferry/deploy/pull-backup.sh <서버 이름> >> $HOME/clip-ferry-backups/pull.log 2>&1
# 서버 쪽은 백업 전용 키가 backup-send.sh만 실행하게 묶여 있다. 30일 지난 사본은 지운다.
set -eu

HOST=${1:?사용법: pull-backup.sh <서버 이름>}
KEY=${CF_BACKUP_KEY:-$HOME/.ssh/clipferry_backup}
DIR=${CF_BACKUP_DIR:-$HOME/clip-ferry-backups}
KEEP_DAYS=30

umask 077
mkdir -p "$DIR"
OUT="$DIR/account-$(date -u +%Y%m%d).sql.gz"
TMP="$OUT.tmp"

ssh -i "$KEY" -o BatchMode=yes -o StrictHostKeyChecking=accept-new "cf@$HOST" > "$TMP"

if gzip -dc "$TMP" | tail -n 1 | grep -q 'Dump completed'; then
  mv "$TMP" "$OUT"
  echo "$(date -u +%FT%TZ) ok $OUT $(wc -c < "$OUT") bytes"
else
  rm -f "$TMP"
  echo "$(date -u +%FT%TZ) FAILED: incomplete download" >&2
  exit 1
fi

find "$DIR" -name 'account-*.sql.gz' -mtime +"$KEEP_DAYS" -delete
