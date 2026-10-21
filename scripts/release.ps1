# Builds, signs and publishes a Quest Ops release to GitHub and Gitee so
# installed apps can update themselves (Gitee serves users in mainland China).
#
# Usage (from the project folder, after bumping the version and committing):
#   powershell -ExecutionPolicy Bypass -File scripts\release.ps1 -Notes "What changed"
#   ... -GiteeOnly   re-publish an already built and GitHub-released version to Gitee
#
# Requires: GitHub CLI logged in (gh auth login); git remotes "origin" (GitHub)
# and "gitee"; a clean, committed working tree; and, outside the repo:
#   %USERPROFILE%\.tauri\questops-updater.key   updater signing key
#   %USERPROFILE%\.tauri\gitee-token.txt        Gitee personal access token
param(
    [string]$Notes = "",
    [switch]$GiteeOnly
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

# Calls the Gitee API with curl (multipart) and returns the parsed JSON.
function Invoke-Gitee([string]$Url, [string[]]$Fields) {
    $ErrorActionPreference = 'Continue'
    $curlArgs = @('-s', '-X', 'POST', $Url) + $Fields
    $out = & curl.exe @curlArgs
    if ($LASTEXITCODE -ne 0) { throw "curl failed calling $Url" }
    $json = ($out -join "`n") | ConvertFrom-Json
    if ($json.message -and -not $json.id -and -not $json.browser_download_url) { throw "Gitee API error: $($json.message)" }
    return $json
}

function Write-Utf8NoBom([string]$Path, [string]$Text) {
    # The updater's JSON parser rejects a BOM.
    [IO.File]::WriteAllText($Path, $Text, (New-Object Text.UTF8Encoding $false))
}

function New-Manifest([string]$Url, [string]$Signature) {
    $m = [ordered]@{
        version   = $version
        notes     = $Notes
        pub_date  = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
        platforms = [ordered]@{ "windows-x86_64" = [ordered]@{ signature = $Signature; url = $Url } }
    }
    return ($m | ConvertTo-Json -Depth 5)
}

Set-Location $root
$version = (Get-Content "$root\src-tauri\tauri.conf.json" -Raw | ConvertFrom-Json).version
$tag = "v$version"
$keyPath = "$env:USERPROFILE\.tauri\questops-updater.key"
$giteeTokenPath = "$env:USERPROFILE\.tauri\gitee-token.txt"
if (-not (Test-Path $keyPath)) { throw "Updater signing key not found: $keyPath" }
if (-not (Test-Path $giteeTokenPath)) { throw "Gitee token not found: $giteeTokenPath" }
$giteeToken = (Get-Content $giteeTokenPath -Raw).Trim()

function Get-RepoPath([string]$Remote, [string]$HostName) {
    $url = git remote get-url $Remote
    if ($url -notmatch "$HostName[:/]([^/]+)/([^/]+?)(\.git)?$") { throw "Remote '$Remote' is not a $HostName repository: $url" }
    return "$($Matches[1])/$($Matches[2])"
}
$repo = Get-RepoPath "origin" "github\.com"
$giteeRepo = Get-RepoPath "gitee" "gitee\.com"

if (git status --porcelain) { throw "Commit or stash your changes first; releases are built from committed code." }

$share = "$root\share"
$setupName = "QuestOps_${version}_x64-setup.exe"
$portableName = "QuestOps_${version}_x64_portable.zip"
$nsis = "$root\src-tauri\target\release\bundle\nsis"

if (-not $GiteeOnly) {
    $exists = Invoke-Native { gh release view $tag --repo $repo --json tagName } | Select-Object -Last 1
    if ($exists -eq 0) { throw "Release $tag already exists on GitHub. Bump the version in tauri.conf.json, package.json and Cargo.toml first." }

    Write-Host "== Building Quest Ops $version" -ForegroundColor Yellow
    $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content $keyPath -Raw
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""
    if ((Invoke-Native { npm run tauri build } | Select-Object -Last 1) -ne 0) { throw "Build failed." }
}

$setup = Get-ChildItem $nsis -Filter "*_${version}_x64-setup.exe" | Select-Object -First 1
if (-not $setup -or -not (Test-Path "$($setup.FullName).sig")) { throw "Signed installer for $version not found in $nsis" }
$signature = (Get-Content "$($setup.FullName).sig" -Raw).Trim()

if (-not $GiteeOnly) {
    # Shareable files (asset names without spaces keep download URLs simple).
    New-Item -ItemType Directory -Force $share | Out-Null
    Get-ChildItem $share -Filter "QuestOps_*" | Where-Object { $_.Name -notlike "*_${version}_*" } | Remove-Item
    Copy-Item $setup.FullName "$share\$setupName" -Force
    $stage = Join-Path $env:TEMP "questops-portable"
    Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $stage | Out-Null
    Copy-Item "$root\src-tauri\target\release\quest-ops.exe" "$stage\Quest Ops.exe"
    if (Test-Path "$share\README.txt") { Copy-Item "$share\README.txt" "$stage\README.txt" }
    Compress-Archive -Path "$stage\*" -DestinationPath "$share\$portableName" -Force
    Remove-Item -Recurse -Force $stage

    Write-Utf8NoBom "$share\latest.json" (New-Manifest "https://github.com/$repo/releases/download/$tag/$setupName" $signature)

    Write-Host "== Publishing $tag to GitHub ($repo)" -ForegroundColor Yellow
    if ((Invoke-Native { git push origin HEAD } | Select-Object -Last 1) -ne 0) { throw "git push to GitHub failed." }
    $releaseNotes = if ($Notes) { $Notes } else { "Quest Ops $version" }
    $assets = @("$share\$setupName", "$share\$portableName", "$share\latest.json")
    $commit = git rev-parse HEAD
    $created = Invoke-Native {
        gh release create $tag @assets --repo $repo --title "Quest Ops $version" --notes $releaseNotes --target $commit
    } | Select-Object -Last 1
    if ($created -ne 0) { throw "gh release create failed." }
}

# ------------------------------------------------------------------ Gitee
Write-Host "== Publishing $tag to Gitee ($giteeRepo)" -ForegroundColor Yellow
if ((Invoke-Native { git fetch origin --tags } | Select-Object -Last 1) -ne 0) { throw "git fetch from GitHub failed." }
if ((Invoke-Native { git push gitee HEAD:main } | Select-Object -Last 1) -ne 0) { throw "git push to Gitee failed." }
if ((Invoke-Native { git push gitee $tag } | Select-Object -Last 1) -ne 0) { throw "pushing tag $tag to Gitee failed." }

$api = "https://gitee.com/api/v5/repos/$giteeRepo/releases"
$releaseNotes = if ($Notes) { $Notes } else { "Quest Ops $version" }

# Reuse the release and attachments if an earlier run already created them.
$release = $null
try { $release = Invoke-RestMethod "$api/tags/${tag}?access_token=$giteeToken" } catch { $release = $null }
if (-not $release -or -not $release.id) {
    $release = Invoke-Gitee $api @(
        '--form-string', "access_token=$giteeToken",
        '--form-string', "tag_name=$tag",
        '--form-string', "name=Quest Ops $version",
        '--form-string', "body=$releaseNotes",
        '--form-string', "target_commitish=main"
    )
}
# ForEach-Object unrolls the list; Windows PowerShell returns a JSON array as one object.
$attached = @(Invoke-RestMethod "$api/$($release.id)/attach_files?access_token=$giteeToken" | ForEach-Object { $_ })
$setupUrl = $null
foreach ($file in @("$share\$setupName", "$share\$portableName")) {
    $name = Split-Path $file -Leaf
    $existing = $attached | Where-Object { $_.name -eq $name } | Select-Object -First 1
    $url = if ($existing) {
        Write-Host "   $name already on Gitee"
        $existing.browser_download_url
    } else {
        Write-Host "   uploading $name"
        (Invoke-Gitee "$api/$($release.id)/attach_files" @('--form-string', "access_token=$giteeToken", '-F', "file=@$file")).browser_download_url
    }
    if ($name -eq $setupName) { $setupUrl = $url }
}
if ("$setupUrl" -notmatch "^https://\S+/$([regex]::Escape($setupName))$") { throw "Unexpected Gitee installer URL: '$setupUrl'" }
# Never publish a manifest whose download does not work anonymously.
$probe = curl.exe -sL -o NUL -r 0-1023 -w "%{http_code}" $setupUrl
if ($probe -notin @('200', '206')) { throw "Gitee installer download check failed (HTTP $probe): $setupUrl" }

# The Gitee manifest lives alone on the "updates" branch, so its address
# (raw/updates/latest.json) never changes. Built with git plumbing and a
# temporary index (no stdin, which Windows PowerShell prefixes with a BOM),
# so the working tree and real index are untouched.
$giteeManifest = Join-Path $env:TEMP "latest-gitee.json"
$tempIndex = Join-Path $env:TEMP "questops-updates.index"
Write-Utf8NoBom $giteeManifest (New-Manifest $setupUrl $signature)
try {
    $blob = (git hash-object -w --no-filters $giteeManifest).Trim()
    if ($blob -notmatch '^[0-9a-f]{40}$') { throw "git hash-object failed: $blob" }
    $env:GIT_INDEX_FILE = $tempIndex
    Remove-Item $tempIndex -ErrorAction SilentlyContinue
    git update-index --add --cacheinfo "100644,$blob,latest.json"
    $tree = (git write-tree).Trim()
} finally {
    Remove-Item Env:GIT_INDEX_FILE -ErrorAction SilentlyContinue
    Remove-Item $tempIndex -ErrorAction SilentlyContinue
}
if ($tree -notmatch '^[0-9a-f]{40}$') { throw "git write-tree failed: $tree" }
$updateCommit = (git commit-tree $tree -m "Update manifest for $tag").Trim()
if ($updateCommit -notmatch '^[0-9a-f]{40}$') { throw "git commit-tree failed: $updateCommit" }
if ((Invoke-Native { git push gitee "${updateCommit}:refs/heads/updates" --force } | Select-Object -Last 1) -ne 0) { throw "Publishing the Gitee update manifest failed." }
Copy-Item $giteeManifest "$share\latest-gitee.json" -Force
Remove-Item $giteeManifest

# Confirm what installed apps will actually read.
$live = curl.exe -sL "https://gitee.com/$giteeRepo/raw/updates/latest.json" | ConvertFrom-Json
if ($live.version -ne $version) { throw "Gitee manifest check failed: expected $version, got '$($live.version)'" }
Write-Host "   Gitee manifest live: version $($live.version)"

Write-Host "== Done" -ForegroundColor Green
if (-not $GiteeOnly) { Write-Host "   GitHub: https://github.com/$repo/releases/tag/$tag" }
Write-Host "   Gitee:  https://gitee.com/$giteeRepo/releases/tag/$tag"
