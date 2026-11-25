param([int]$Pr)

$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent

if ($Pr) {
    throw 'Build upstream PRs separately on clean upstream branches.'
}

$branch = git -C $repo branch --show-current
if ($branch -cne 'keshii/distribution') {
    throw 'Switch to keshii/distribution before building a release viewer.'
}

if (git -C $repo status --porcelain --untracked-files=no) {
    throw 'The distribution checkout has uncommitted changes.'
}

git -C $repo fetch fork --quiet
if ($LASTEXITCODE -ne 0) {
    throw 'Could not fetch the distribution branch.'
}

$sha = git -C $repo rev-parse HEAD
$remoteSha = git -C $repo rev-parse refs/remotes/fork/keshii/distribution

if ($sha -cne $remoteSha) {
    throw 'The local and remote distribution commits differ.'
}

$rel = Join-Path (Split-Path $repo -Parent) 'RELEASE'
$out = Join-Path $rel 'Aurora-Viewer'
New-Item -ItemType Directory -Force -Path $out | Out-Null

$previousTag = $env:AURORA_VERSION_TAG
$previousStrip = $env:CARGO_PROFILE_RELEASE_STRIP

$tag = git -C $repo describe --tags --exact-match --match 'v*-Keshii' HEAD 2>$null

if ($LASTEXITCODE -ne 0 -or -not $tag) {
    throw 'The distribution commit has no Keshii release tag.'
}

$sw = [Diagnostics.Stopwatch]::StartNew()

Push-Location $repo
try {
    $env:AURORA_VERSION_TAG = $tag
    $features = @()

    if ($IsWindows) {
        $features = @('--features', 'xr-static-loader')
    } else {
        $env:CARGO_PROFILE_RELEASE_STRIP = 'debuginfo'
    }

    cargo build --release --locked -p aurora-viewer @features

    if ($LASTEXITCODE -ne 0) {
        throw 'Release build failed.'
    }

    $exeName = if ($IsWindows) {
        'aurora-viewer-Keshii.exe'
    } else {
        'aurora-viewer-Keshii'
    }

    $exe = Join-Path $repo "target/release/$exeName"

    & "$PSScriptRoot/verify-version.ps1" -Exe $exe -Tag $tag

    if ($IsMacOS) {
        $app = Join-Path $out 'Aurora Viewer-Keshii.app'
        & "$PSScriptRoot/package-macos.ps1" -Exe $exe -Out $app

        codesign --force --sign - $app
        if ($LASTEXITCODE -ne 0) { throw 'Application signing failed.' }

        codesign --verify --strict --verbose=2 $app
        if ($LASTEXITCODE -ne 0) { throw 'Application signature verification failed.' }
    } else {
        & "$PSScriptRoot/package.ps1" -Exe $exe -Out $out
    }

    @(
        "Aurora Viewer-Keshii $($tag.TrimStart('v'))"
        "distribution: $((git log -1 --format='%h %s (%ci)').Trim())"
        "built on $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')"
    ) | Set-Content -LiteralPath (Join-Path $out 'version.txt') -Encoding UTF8

    Write-Host "Release viewer ready: $out" -ForegroundColor Green
    Write-Host "Built in $([math]::Floor($sw.Elapsed.TotalMinutes)) min $($sw.Elapsed.Seconds) s."
} finally {
    Pop-Location

    if ($null -eq $previousTag) {
        Remove-Item Env:AURORA_VERSION_TAG -ErrorAction SilentlyContinue
    } else {
        $env:AURORA_VERSION_TAG = $previousTag
    }

    if ($null -eq $previousStrip) {
        Remove-Item Env:CARGO_PROFILE_RELEASE_STRIP -ErrorAction SilentlyContinue
    } else {
        $env:CARGO_PROFILE_RELEASE_STRIP = $previousStrip
    }
}
