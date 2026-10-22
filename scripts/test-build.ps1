# Builds the current code (committed or not) as a TEST build and opens it.
# Test builds show a "Test build" badge in the sidebar with the version and
# commit, and are never published. They use the same data as the installed app.
#
# Usage: powershell -ExecutionPolicy Bypass -File scripts\test-build.ps1
$ErrorActionPreference = 'Continue'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

# Mark the build: commit hash, plus "+changes" when there are uncommitted edits.
$commit = (git rev-parse --short HEAD).Trim()
if (git status --porcelain) { $commit = "$commit+changes" }
$env:VITE_BUILD_COMMIT = $commit
Remove-Item Env:VITE_OFFICIAL_RELEASE -ErrorAction SilentlyContinue

# The build replaces the exe, so close any running copy first (data is saved).
Get-Process quest-ops -ErrorAction SilentlyContinue | ForEach-Object { $_.CloseMainWindow() | Out-Null }
Start-Sleep -Seconds 3
Get-Process quest-ops -ErrorAction SilentlyContinue | Stop-Process -Force

Write-Host "== Building test build ($commit)" -ForegroundColor Yellow
npm run tauri build -- --no-bundle 2>&1 | ForEach-Object { "$_" } | Select-String -Pattern "^error|Finished|Built application"
if ($LASTEXITCODE -ne 0) { Write-Host "Build failed." -ForegroundColor Red; exit 1 }

# Windows Smart App Control sometimes blocks a freshly built exe for a moment.
$exe = "$root\src-tauri\target\release\quest-ops.exe"
foreach ($try in 1..3) {
    try {
        Start-Process $exe -ErrorAction Stop
        Write-Host "== Test build running: $exe" -ForegroundColor Green
        exit 0
    } catch {
        Write-Host "   launch blocked (try $try), retrying..."
        Start-Sleep -Seconds 8
    }
}
Write-Host "Could not launch $exe" -ForegroundColor Red
exit 1
