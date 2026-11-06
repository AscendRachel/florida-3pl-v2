# Florida 3PL: Rust version

The rewrite of the Florida 3PL orders app (Python + SQLite) in **Rust**, on **MariaDB**, managed with **phpMyAdmin**.
The old app stays where it is (`greentek-orders`, its GitHub repository and its Docker image are not touched).
This is a separate project. It is not on GitHub yet.

## Status

| Phase | What | State |
|---|---|---|
| 1 | Project, MariaDB + phpMyAdmin in Docker, the database design, the server serves the web pages | done |
| 2 | Import tool (old `data/` folder into MariaDB), sign-in, roles, tab access, Team page | **done** |
| 3 | All Orders, Scan, Order Cards, Dashboard | **done**: every action checked against the Python app with the same changes on the same data (`tools/compare.py`, `tools/compare_write.py`) |
| 4 | Returns, Materials, Kohl's, Work Log, Notifications, Amazon import, Admin tab, Import page | **done**: checked against the Python app (`tools/compare_tabs.py`, `tools/compare_import.py`) |
| 5 | Test against a copy of the real data, then switch over | **plan written**: see `SWITCH-OVER.md` (the real-world tests and the switch itself are still to do) |

The web pages in `web/` are the old app's pages, unchanged. They talk to the server through `/api/...`, so the Rust server has to
answer the same addresses the same way. That is what phases 2 to 4 do, one tab at a time.

## Run it

1. Install Rust (rustup) and Docker Desktop.
2. Copy `.env.example` to `.env` and set the passwords (already done on this PC).
3. Start the database and phpMyAdmin:
   ```
   docker compose up -d
   ```
4. Build and start the app (it is built inside Docker, because Windows Smart App Control blocks native Rust builds on this PC):
   ```
   docker compose up -d --build app
   ```
   The first start builds every table from `migrations/`. Open http://localhost:8766 (8765 is left to the old Python app).
   To only check that the code compiles while you work: `docker compose run --rm dev cargo check`.
5. Look at the data: http://localhost:8081 (phpMyAdmin; user and password are `DB_USER` / `DB_PASSWORD` in `.env`).

## Bring the old app's data over

1. Make a copy of the old app's `data` folder in `import-data/` (this project's folder). Use the SQLite backup so a running app does not matter.
2. Stop the app first (its returns sync writes during an import and breaks it), then copy the data in. Everything already in the new database is deleted with `--replace`:
   ```
   docker compose stop app
   docker compose run --rm app /app/florida-3pl import /import --replace
   ```
   It prints the number of rows copied for each table. Dates and numbers that do not fit their column are kept in `import_rejects`.
3. People sign in with the passwords they already have (same PBKDF2 hashes as the Python app).

For a fresh database with no old data, make the first admin on the server:
```
docker compose run --rm app /app/florida-3pl create-admin
```

Check: http://localhost:8766/api/health shows the number of tables and orders.

## The database (`migrations/`)

One database, `florida3pl`, instead of the old app's 8 SQLite files and 4 JSON files.

| Migration | Tables |
|---|---|
| `0001_orders.sql` | `orders`: one real, typed column for each of the 54 sheet fields (dates are dates, ticks are 1/0, QTY is a number) |
| `0002_orders_support.sql` | `column_defs`, `order_custom_values`, `dropdown_lists`, `dropdown_cells`, `order_edits`, `edit_log`, `scans`, `ship_events`, `boxes`, `trash_rows`, `app_settings` |
| `0003_accounts_and_team.sql` | `users`, `sessions`, `work_log_messages`, `notifications`, `notification_seen`, `drive_names` |
| `0004_returns_materials_kohls.sql` | `returns`, `returns_sync_log`, `kohls`, `materials`, `stock_sorts` |

Differences from the old app, on purpose:

- Columns have names (`shipped_to_customer`), not numbers (`c19`). An admin's own extra columns go in `order_custom_values`.
- Real types and real foreign keys (deleting an order removes its dropdown cells, edits, custom values).
- The old `branding.json`, `dashboard.json`, `locks.json` and `schema.json` become rows in `app_settings` / `column_defs`.
- Passwords: both apps use salted PBKDF2-SHA256 (200,000 rounds), so imported people sign in with the passwords they already have.
- phpMyAdmin and MariaDB listen on this PC only (`127.0.0.1`). Never publish those ports. Put the app behind HTTPS (Caddy) when it goes on a server.

## On the server (domain + HTTPS with Caddy)

`docker-compose.server.yml` runs MariaDB, the app and Caddy. Only Caddy is open to the internet (ports 80 and 443); MariaDB and the app are not published, and phpMyAdmin is left out.
1. `.env`: set the passwords and add `DOMAIN=orders.example.com` (a DNS A record for it must point to the server).
2. `docker compose -f docker-compose.server.yml up -d --build`
3. If the server already runs its own Caddy, start only `mariadb` and `app` and add the block from `Caddyfile` to the existing one.

## Back up

The database files live in `db-data/` (not in git). For a copy that can be restored anywhere:
```
docker exec florida-3pl-db sh -c "mariadb-dump -uroot -p$MARIADB_ROOT_PASSWORD florida3pl" > backup.sql
```

