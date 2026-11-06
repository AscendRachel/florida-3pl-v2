# Hardening the Rust app on a cloud VM

Two parts: what the app and its containers already do (in this repository), and what has to be done on the VM itself (your IT head).
Do the VM part before the domain goes live.

## 1. Already built in

**The app**
- Sign-in: passwords are salted PBKDF2-SHA256 (200,000 rounds). New passwords must be 10 to 200 characters.
- Guessing is slowed: 8 wrong tries per address, and 30 per account name from anywhere, then a 10-minute block. Every failed sign-in is logged (`failed sign-in ip=... user=...`) so a tool such as fail2ban can read it.
- The address used for that limit comes from `X-Forwarded-For` only when the request comes from our own proxy (a private/Docker address), so nobody can fake it.
- Sessions: random 256-bit token, `HttpOnly`, `Secure` (always, with `HTTPS_ONLY=1`), `SameSite=Lax`, 14 days, removed on logout, on password change and when an account is removed.
- Requests that change data are refused unless they come from the app's own pages (`Origin` / `Sec-Fetch-Site` check): no cross-site forgery.
- Browser headers on every answer: `Content-Security-Policy` (only this site's scripts; label files only from Google Drive; no framing; no plugins), `Strict-Transport-Security`, `X-Content-Type-Options`, `X-Frame-Options: DENY`, `Referrer-Policy`, `Permissions-Policy`, cross-origin isolation.
- `/api/health` tells the internet only "ok". The numbers (orders, tables) are only shown to someone on the server itself.
- Database errors are logged, never shown to the person. All database values are bound parameters.
- The first admin can only be made on the server (`create-admin`), never through the web page.
- The Google API key lives in `data/drive_key.txt` on the server, not in the code or in git.

**The containers (`docker-compose.server.yml`)**
- Only Caddy is reachable from the internet (ports 80 and 443). MariaDB, the app and phpMyAdmin publish no ports.
- phpMyAdmin is reached only through Caddy at `https://DOMAIN/phpmyadmin`, over HTTPS, and only with a JWT (HS256, signed with `JWT_SECRET` from `.env`, valid 8 hours) that the app gives to a signed-in admin at `/pma-login`. Caddy asks the app (`/api/pma-auth`) before every request; no valid JWT means the visitor is sent to the sign-in. The check also confirms the person is still an active admin, so removing an admin closes the door. phpMyAdmin then still asks for the database user and password. Keep `JWT_SECRET` secret (32+ random characters, `openssl rand -hex 32`); the app refuses to open the gate when it is missing or shorter.
- MariaDB is on a private Docker network with no route to the internet; the root account works only inside its own container.
- The app runs as a normal user, with a read-only filesystem, all Linux capabilities dropped, `no-new-privileges`, a 1 GB memory limit and a process limit.
- Caddy handles the HTTPS certificate and renews it by itself; HTTP is redirected to HTTPS.
- Logs are size-limited, so they cannot fill the disk.

## 2. On the VM (to do)

1. **Firewall at the cloud provider and on the VM:** allow inbound only 22 (SSH, ideally only from your IT head's address), 80 and 443. Nothing else. Never open 3306 or 8081.
   ```
   sudo ufw default deny incoming
   sudo ufw default allow outgoing
   sudo ufw allow 22/tcp
   sudo ufw allow 80/tcp
   sudo ufw allow 443/tcp
   sudo ufw enable
   ```
   Note: Docker publishes ports around ufw. That is why this setup publishes only 80 and 443 on purpose; do not add `ports:` for the database or the app.
2. **SSH:** keys only. In `/etc/ssh/sshd_config`: `PasswordAuthentication no`, `PermitRootLogin no`. Use a normal user with sudo. Restart: `sudo systemctl restart ssh`.
3. **fail2ban** for SSH (`sudo apt install fail2ban`). Optional second jail on Caddy's log for repeated 401/429 on `/api/login`.
4. **Automatic security updates:** `sudo apt install unattended-upgrades` and turn it on. Reboot for kernel updates now and then.
5. **Strong secrets:** `.env` passwords of 24+ random characters (`openssl rand -base64 24`). `chmod 600 .env data/drive_key.txt`. Never commit them.
6. **Google API key:** in Google Cloud, restrict it to the Drive API only, and to the server's public IP address.
7. **Accounts:** one account per person; remove people who leave (Team page). Only a few admins. Everyone uses a password of 10+ characters that they use nowhere else.
8. **Backups:** a daily `mariadb-dump` (see `SWITCH-OVER.md`), kept 14 days, and a copy stored off the VM (another account or region). Encrypt the copy if it leaves your control. A backup that was never restored is not a backup: test a restore once.
9. **Snapshots:** turn on the provider's disk snapshots as a second safety net.
10. **Updates of the app:** `git pull`, then `docker compose -f docker-compose.server.yml up -d --build`. Update the base images now and then (`docker compose pull`), and rebuild.
11. **Watch the logs:** `docker compose -f docker-compose.server.yml logs -f app caddy`. Look for repeated `failed sign-in` lines from one address.
12. **Check it from outside** once it is live: https://www.ssllabs.com/ssltest/ (expect A or A+) and https://securityheaders.com/ (expect A). Confirm that ports 3306 and 8765 are closed from the internet (`nmap` from another machine).

## 3. What is not covered

- No two-factor sign-in yet. If the IT head wants it, it is a separate piece of work.
- No automatic account lock for the admin: the 10-minute block applies to everybody.
- The pages use a few small inline scripts, so the Content-Security-Policy has to allow inline scripts. Moving them into files would let it be stricter.
