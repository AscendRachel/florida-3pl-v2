@echo off
rem Starts the Rust app (v2) with its database, then opens it in the browser. Docker Desktop must be running.
cd /d "%~dp0"
docker info >nul 2>&1
if errorlevel 1 (
  echo Docker Desktop is not running. Open Docker Desktop, wait until it says "Engine running", then run this again.
  pause
  exit /b 1
)
docker compose up -d --build app
if errorlevel 1 (
  echo Something went wrong starting the app. Read the message above.
  pause
  exit /b 1
)
echo Waiting for the app to start...
timeout /t 8 /nobreak >nul
start http://localhost:8766
echo The app is at http://localhost:8766  (database manager: http://localhost:8081)
pause
