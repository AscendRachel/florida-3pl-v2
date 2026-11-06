@echo off
rem Stops the Rust app (v2) and its database. The data is kept.
cd /d "%~dp0"
docker compose stop
pause
