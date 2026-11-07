#!/usr/bin/env bash
# Restore a database backup made by backup.sh. This REPLACES the data in the database with the backup.
#
#   sudo ./tools/restore.sh /var/backups/florida-3pl/florida3pl-2026-11-07_0230.sql.gz
#
# The app is stopped while it restores and started again afterwards. Make a fresh backup first if the current data matters.
set -euo pipefail

FILE="${1:-}"
COMPOSE_FILE="${COMPOSE_FILE:-docker-compose.server.yml}"
DB_CONTAINER="${DB_CONTAINER:-florida-3pl-db}"
DB_NAME="${DB_NAME:-florida3pl}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if [ -z "$FILE" ] || [ ! -f "$FILE" ]; then
  echo "usage: sudo ./tools/restore.sh <backup file .sql.gz>"; exit 1
fi
gzip -t "$FILE"

echo "This replaces everything in the database '$DB_NAME' with:"
echo "  $FILE"
read -r -p "Type RESTORE to continue: " ok
[ "$ok" = "RESTORE" ] || { echo "cancelled"; exit 1; }

cd "$ROOT"
docker compose -f "$COMPOSE_FILE" stop app
gzip -dc "$FILE" | docker exec -i "$DB_CONTAINER" sh -c "mariadb -uroot -p\"\$MARIADB_ROOT_PASSWORD\" $DB_NAME"
docker compose -f "$COMPOSE_FILE" up -d app
echo "restored. Check https://your-domain/api/health from the server, and open All Orders."
