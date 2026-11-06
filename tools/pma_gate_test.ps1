# Local end-to-end test of the phpMyAdmin gate (JWT): makes a temporary admin and member, walks the flow, removes them.
# Run from the project folder:  powershell -NoProfile -File tools\pma_gate_test.ps1
$ErrorActionPreference = "Continue"   # docker writes progress to stderr
Set-Location (Split-Path $PSScriptRoot)
$tmp = $env:TEMP
@"
services:
  app:
    environment:
      JWT_SECRET: "test-secret-test-secret-test-secret-0123"
"@ | Set-Content "$tmp\pma-override.yml" -Encoding ascii
$rootpw = ((Get-Content .env | Where-Object { $_ -like 'DB_ROOT_PASSWORD=*' }) -replace '^DB_ROOT_PASSWORD=', '')
$dbname = ((Get-Content .env | Where-Object { $_ -like 'DB_NAME=*' }) -replace '^DB_NAME=', '')
function Sql($q) { docker exec florida-3pl-db mariadb -uroot "-p$rootpw" $dbname -e $q 2>&1 | Out-Null }
$salt = "00112233445566778899aabbccddeeff"
$h = python -c "import hashlib;print(hashlib.pbkdf2_hmac('sha256',b'gate-test-pass-1',bytes.fromhex('$salt'),200000).hex())"
Sql "DELETE FROM users WHERE username IN ('gatetest_admin','gatetest_member')"
Sql "INSERT INTO users (username,name,pw_hash,salt,role,created_at) VALUES ('gatetest_admin','Gate Admin','$h','$salt','admin',NOW()),('gatetest_member','Gate Member','$h','$salt','member',NOW())"
docker compose -f docker-compose.yml -f "$tmp\pma-override.yml" up -d app 2>&1 | Out-Null
Start-Sleep 8
$base = "http://localhost:8766"
function Login($user) {
  $jar = "$tmp\gate-jar-$user.txt"; Remove-Item $jar -ErrorAction SilentlyContinue
  (@{ username = $user; password = "gate-test-pass-1" } | ConvertTo-Json -Compress) | Set-Content "$tmp\gate-body.json" -Encoding ascii
  curl.exe -s -o NUL -c $jar -H "Origin: $base" -H "Content-Type: application/json" --data "@$tmp\gate-body.json" "$base/api/login"
  $jar
}
function Call($label, $path, $extra) {
  $out = curl.exe -s -i -o - @extra "$base$path" | Out-String
  $code = ($out -split "`r?`n")[0]
  $loc = ([regex]::Match($out, "(?im)^location: (.+)$")).Groups[1].Value.Trim()
  Write-Host ("{0,-48} {1}  {2}" -f $label, $code, $loc)
  $out
}
$r = Call "no cookie: /api/pma-auth" "/api/pma-auth" @()
$r = Call "not signed in: /pma-login" "/pma-login" @()
$memjar = Login "gatetest_member"
$r = Call "member: /pma-login" "/pma-login" @("-b", $memjar)
$admjar = Login "gatetest_admin"
$r = Call "admin signed in, no pass: /api/pma-auth" "/api/pma-auth" @("-b", $admjar)
$r = Call "admin: /pma-login (gets the pass)" "/pma-login" @("-b", $admjar)
$setc = ([regex]::Match($r, "(?im)^set-cookie: (.+)$")).Groups[1].Value.Trim()
Write-Host ("Set-Cookie flags: " + (($setc -split ';' | Select-Object -Skip 1 | ForEach-Object { $_.Trim() }) -join ', '))
$pass = ($setc -split ';')[0]
$r = Call "admin with pass: /api/pma-auth" "/api/pma-auth" @("-H", "Cookie: $pass")
$tampered = $pass.Substring(0, $pass.Length - 3) + $(if ($pass[-3] -eq 'A') { 'B' } else { 'A' }) + $pass.Substring($pass.Length - 2)
$r = Call "pass with one character changed" "/api/pma-auth" @("-H", "Cookie: $tampered")
$r = Call "pass made up with the wrong key" "/api/pma-auth" @("-H", "Cookie: f3pl_pma=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJnYXRldGVzdF9hZG1pbiIsImF1ZCI6InBocG15YWRtaW4iLCJleHAiOjk5OTk5OTk5OTk5fQ.AAAA")
Sql "UPDATE users SET disabled = 1 WHERE username = 'gatetest_admin'"
$r = Call "admin removed, same pass: /api/pma-auth" "/api/pma-auth" @("-H", "Cookie: $pass")
Sql "DELETE FROM sessions WHERE username IN ('gatetest_admin','gatetest_member')"
Sql "DELETE FROM users WHERE username IN ('gatetest_admin','gatetest_member')"
Remove-Item "$tmp\gate-jar-*.txt", "$tmp\gate-body.json" -ErrorAction SilentlyContinue
docker compose up -d app 2>&1 | Out-Null
"cleaned up; app restarted without the test secret"
