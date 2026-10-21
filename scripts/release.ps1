# Builds, signs and publishes a Quest Ops release to GitHub so installed apps
# can update themselves.
#
# Usage (from the project folder, after bumping the version and committing):
#   powershell -ExecutionPolicy Bypass -File scripts\release.ps1 -Notes "What changed"
#
# Requires: GitHub CLI logged in (gh auth login), a clean, committed working
# tree, and the updater signing key at %USERPROFILE%\.tauri\questops-updater.key.
param(
    [string]$Notes = ""
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent

# Runs an external tool and returns its exit code. Windows PowerShell 5.1
# would otherwise treat any stderr output (progress, "not found") as fatal.
function Invoke-Native([scriptblock]$Block) {
    $ErrorActionPreference = 'Continue'
    & $Block 2>&1 | ForEach-Object { Write-Host "$_" }
    return $LASTEXITCODE
}
Set-Location $root

$version = (Get-Content "$root\src-tauri\tauri.conf.json" -Raw | ConvertFrom-Json).version
$tag = "v$version"
$keyPath = "$env:USERPROFILE\.tauri\questops-updater.key"
if (-not (Test-Path $keyPath)) { throw "Updater signing key not found: $keyPath" }

$remote = git remote get-url origin
if ($remote -notmatch 'github\.com[:/]([^/]+)/([^/]+?)(\.git)?$') { throw "origin is not a GitHub repository: $remote" }
$repo = "$($Matches[1])/$($Matches[2])"

if (git status --porcelain) { throw "Commit or stash your changes first; releases are built from committed code." }
$exists = Invoke-Native { gh release view $tag --repo $repo --json tagName } | Select-Object -Last 1
if ($exists -eq 0) { throw "Release $tag already exists. Bump the version in tauri.conf.json, package.json and Cargo.toml first." }

Write-Host "== Building Quest Ops $version" -ForegroundColor Yellow
$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content $keyPath -Raw
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""
if ((Invoke-Native { npm run tauri build } | Select-Object -Last 1) -ne 0) { throw "Build failed." }

$nsis = "$root\src-tauri\target\release\bundle\nsis"
$setup = Get-ChildItem $nsis -Filter "*_${version}_x64-setup.exe" | Select-Object -First 1
if (-not $setup -or -not (Test-Path "$($setup.FullName).sig")) { throw "Signed installer for $version not found in $nsis" }

# Shareable files (asset names without spaces keep download URLs simple).
$share = "$root\share"
New-Item -ItemType Directory -Force $share | Out-Null
Get-ChildItem $share -Filter "QuestOps_*" | Where-Object { $_.Name -notlike "*_${version}_*" } | Remove-Item
$setupName = "QuestOps_${version}_x64-setup.exe"
$portableName = "QuestOps_${version}_x64_portable.zip"
Copy-Item $setup.FullName "$share\$setupName" -Force
$stage = Join-Path $env:TEMP "questops-portable"
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item "$root\src-tauri\target\release\quest-ops.exe" "$stage\Quest Ops.exe"
if (Test-Path "$share\README.txt") { Copy-Item "$share\README.txt" "$stage\README.txt" }
Compress-Archive -Path "$stage\*" -DestinationPath "$share\$portableName" -Force
Remove-Item -Recurse -Force $stage

# Update manifest read by installed apps.
$manifest = [ordered]@{
    version   = $version
    notes     = $Notes
    pub_date  = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    platforms = [ordered]@{
        "windows-x86_64" = [ordered]@{
            signature = (Get-Content "$($setup.FullName).sig" -Raw).Trim()
            url       = "https://github.com/$repo/releases/download/$tag/$setupName"
        }
    }
}
# UTF-8 without BOM: the updater's JSON parser rejects a BOM.
[IO.File]::WriteAllText("$share\latest.json", ($manifest | ConvertTo-Json -Depth 5), (New-Object Text.UTF8Encoding $false))

Write-Host "== Publishing $tag to $repo" -ForegroundColor Yellow
if ((Invoke-Native { git push origin HEAD } | Select-Object -Last 1) -ne 0) { throw "git push failed." }
$releaseNotes = if ($Notes) { $Notes } else { "Quest Ops $version" }
$assets = @("$share\$setupName", "$share\$portableName", "$share\latest.json")
$commit = git rev-parse HEAD
$created = Invoke-Native {
    gh release create $tag @assets --repo $repo --title "Quest Ops $version" --notes $releaseNotes --target $commit
} | Select-Object -Last 1
if ($created -ne 0) { throw "gh release create failed." }

Write-Host "== Done: https://github.com/$repo/releases/tag/$tag" -ForegroundColor Green
