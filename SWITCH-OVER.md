# Switching from the Python app to the Rust app

Nothing here has been done yet. This is the plan, in order. The old app stays untouched until step 7, so you can go back at any point before then.

## Before the day

1. **Ask the IT head where the Rust app will run** (this PC, or a server). On a server, the app goes behind HTTPS (Caddy or nginx). MariaDB (3306) and phpMyAdmin (8081) must never be open to the internet. Keep them on `127.0.0.1` or a private network.
2. **Real Google Sheets export test.** In the Rust app, use the Import page with a real exported `.xlsx` into a scratch database, not the live one. This is the one reader path only tested with a hand-made workbook.
3. **Real Google Drive test.** Put the Drive API key in `data/drive_key.txt` and open the Admin tab's label check. Compare a few labels by hand with the Python app.
4. **Sign-in test with each real person.** Passwords carry over (same hashes). Ask each person (or you, as admin) to log in once on the Rust app on a test day and confirm their tabs.
5. **Returns sync test.** Turn on the returns auto-sync in the Rust app and watch `returns_sync_log` for a day.

## Switch-over day (about 1 hour)

6. **Stop writing in the old app.** Tell everyone to stop; stop the Python app.
7. **Take the final copy.** Use the SQLite backup of the old `data` folder, copy it into `import-data/`.
8. **Stop the Rust app, import, start it:**
   ```
   docker compose stop app
   docker compose run --rm app /app/florida-3pl import /import --replace
   docker compose up -d app
   ```
   (Stopping the app first matters: its returns sync writes while an import runs and breaks it.)
9. **Check:** `/api/health` shows the order count equal to the old app's; open All Orders, Dashboard and Returns; check `import_rejects` (expected: the 8 junk TAPICKER dates).
10. **Point the team at the new address** (or give the Rust app port 8765 once the Python app is stopped for good).

## Keeping the old app as a fallback

Do not delete the Python app, its `data/` folder or its Docker image for at least 2 weeks. Orders typed in the Rust app after the switch are not in the old app. To go back you would have to re-enter them, so decide in the first days.

## Backups from now on

Daily, from a scheduled task:
```
docker exec florida-3pl-db sh -c 'mariadb-dump -uroot -p"$MARIADB_ROOT_PASSWORD" --single-transaction florida3pl' > backup-YYYY-MM-DD.sql
```
Keep the last 14 and copy one a week off this PC. Also back up `data/` (logo, Drive key).

## Known differences to tell the team

- Delivery Date / Source Order Date: junk text in these columns is no longer kept in the cell (the original is in `import_rejects`).
- Row ids and lock ids are different numbers than in the old app; nothing visible changes.
- The Rust app has no Python-style single-file backup; use `mariadb-dump` above.
