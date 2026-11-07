#!/usr/bin/env bash
# Daily backup of the Florida 3PL database (and the data/ folder: logo, Google API key, uploads).
#
#   sudo ./tools/backup.sh                     make one backup now
#   BACKUP_DIR=/somewhere KEEP_DAYS=30 sudo -E ./tools/backup.sh
#
# Run it as root (it talks to Docker), from cron: see SWITCH-OVER.md. Backups go to /var/backups/florida-3pl (only root can read them),
# the newest KEEP_DAYS days are kept. Each database dump is checked (gzip test, size, and it must contain the orders table) before an
# old one can be deleted, so a failed backup never removes a good one.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIR="${BACKUP_DIR:-/var/backups/florida-3pl}"
KEEP="${KEEP_DAYS:-14}"
DB_CONTAINER="${DB_CONTAINER:-florida-3pl-db}"
DB_NAME="${DB_NAME:-florida3pl}"
STAMP="$(date +%Y-%m-%d_%H%M)"

umask 077
mkdir -p "$DIR"
chmod 700 "$DIR"

log() { echo "$(date '+%F %T') $*"; }

SQL="$DIR/florida3pl-$STAMP.sql.gz"
TMP="$SQL.partial"

log "dumping $DB_NAME from $DB_CONTAINER ..."
# --single-transaction: a consistent copy while the app keeps running; the root password is read inside the container, never shown here
if ! docker exec "$DB_CONTAINER" sh -c "mariadb-dump -uroot -p\"\$MARIADB_ROOT_PASSWORD\" --single-transaction --quick --routines --events --default-character-set=utf8mb4 $DB_NAME" | gzip -6 > "$TMP"; then
  rm -f "$TMP"; log "FAILED: the dump did not finish"; exit 1
fi

# check it before trusting it
# (grep -q stops reading early, which makes gzip report a broken pipe: so count the matches instead of using -q)
if ! gzip -t "$TMP"; then rm -f "$TMP"; log "FAILED: the backup file is damaged"; exit 1; fi
SIZE=$(stat -c %s "$TMP")
if [ "$SIZE" -lt 20000 ]; then rm -f "$TMP"; log "FAILED: the backup is only $SIZE bytes"; exit 1; fi
HAS_ORDERS=$(gzip -dc "$TMP" | grep -c 'CREATE TABLE `orders`' || true)
if [ "${HAS_ORDERS:-0}" -lt 1 ]; then rm -f "$TMP"; log "FAILED: the backup has no orders table"; exit 1; fi
COMPLETE=$(gzip -dc "$TMP" | tail -n 3 | grep -c 'Dump completed' || true)
if [ "${COMPLETE:-0}" -lt 1 ]; then rm -f "$TMP"; log "FAILED: the backup is cut short"; exit 1; fi
mv "$TMP" "$SQL"
log "database backup ok: $SQL ($(du -h "$SQL" | cut -f1))"

# the data/ folder (logo, drive_key.txt, uploads). Small, so a copy each time.
if [ -d "$ROOT/data" ]; then
  tar -czf "$DIR/data-$STAMP.tar.gz" -C "$ROOT" data && log "data folder backup ok: data-$STAMP.tar.gz"
fi

# remove the ones older than KEEP days (only after this backup succeeded)
find "$DIR" -maxdepth 1 -type f \( -name 'florida3pl-*.sql.gz' -o -name 'data-*.tar.gz' \) -mtime +"$KEEP" -print -delete | sed 's/^/removed old backup: /'
log "done. $(ls -1 "$DIR"/florida3pl-*.sql.gz | wc -l) database backups kept in $DIR"
