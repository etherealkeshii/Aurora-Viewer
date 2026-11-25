# Aurora Tools: manage the GitHub releases — create one (a SemVer tag on the
# latest main: the Release workflow builds and publishes it) or delete one
# (dot-sourced by aurora-tools.ps1). Needs write access to the repository.
# $DryRun (aurora-tools.ps1 -DryRun) prints the GitHub writes instead.

# gh's JSON output as objects, one per array item: Windows PowerShell 5.1
# passes a whole JSON array down the pipeline as a single object, so
# @(gh … | ConvertFrom-Json) would count 1. Writing out the assigned value
# enumerates it.
function Invoke-GhJson([string[]]$Arguments) {
    $json = gh @Arguments 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $json) { return }
    $items = ($json -join "`n") | ConvertFrom-Json
    $items
}

function Test-ReleaseRights($Repo) {
    if ($Repo.Slug -cne 'etherealkeshii/Aurora-Viewer') { return $false }
    $actor = gh api user --jq .login 2>$null
    if ($LASTEXITCODE -ne 0 -or $actor -cne 'etherealkeshii') { return $false }
    $perm = gh api "repos/$($Repo.Slug)" --jq '.permissions' 2>$null | ConvertFrom-Json
    [bool]($perm -and $perm.admin)
}

function Get-ReleaseList($Repo) {
    @(Invoke-GhJson @('release', 'list', '-R', $Repo.Slug, '--limit', '30', '--json', 'tagName,name,isPrerelease,isLatest,isDraft,publishedAt'))
}

# A version tag: vMAJOR.MINOR.PATCH, optionally -suffix (a pre-release).
function ConvertTo-Version([string]$Tag) {
    if ($Tag -match '^v?(\d+)\.(\d+)\.(\d+)(-[0-9A-Za-z.-]+)?$') {
        [pscustomobject]@{ Major = [int]$Matches[1]; Minor = [int]$Matches[2]; Patch = [int]$Matches[3]; Suffix = $Matches[4] }
    }
}

# The last stable version (no suffix), its date, and the PRs merged since.
function Get-ReleaseContext($Repo) {
    git -C $Paths.Repo fetch fork --tags --quiet 2>$null
    if ($LASTEXITCODE -ne 0) { throw 'Could not fetch the distribution repository.' }
    $last = git -C $Paths.Repo tag --list 'v*' | ForEach-Object { $v = ConvertTo-Version $_; if ($v -and $_ -cmatch '-Keshii$') { [pscustomobject]@{ Tag = $_; V = $v } } } |
        Sort-Object { $_.V.Major }, { $_.V.Minor }, { $_.V.Patch }, { $_.Tag } | Select-Object -Last 1
    $since = if ($last) { (git -C $Paths.Repo log -1 --format=%cI $last.Tag).Trim() }
    $filter = if ($since) { @('--search', "merged:>$since") } else { @() }
    $prs = @(Invoke-GhJson (@('pr', 'list', '-R', $Repo.Slug, '--state', 'merged', '--limit', '200', '--json', 'number,title,labels') + $filter))
    [pscustomobject]@{ Last = $last; Since = $since; Prs = $prs }
}

function Get-PrType($Pr) {
    $names = @($Pr.labels | ForEach-Object name)
    foreach ($t in 'breaking', 'feature', 'fix', 'performance', 'maintenance', 'docs') { if ($names -contains $t) { return $t } }
    'other'
}

function Show-ReleaseList($Releases) {
    $tagWidth = [Math]::Max(32, ($Releases | ForEach-Object { $_.tagName.Length + 3 } | Measure-Object -Maximum).Maximum)
    $versionPattern = '^v?[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?-Keshii$'
    Write-Rule ($tagWidth + 76)
    Write-Color ('  ' + 'Tag'.PadRight($tagWidth) + 'Name'.PadRight(40) + 'Published'.PadRight(14) + 'Type') muted
    Write-Rule ($tagWidth + 76)
    if (-not $Releases) { Write-Color '  No release.' muted }
    foreach ($r in $Releases) {
        $kind = if ($r.tagName -cnotmatch $versionPattern) { 'assets (protected)' } elseif ($r.isDraft) { 'draft' } elseif ($r.isPrerelease) { 'pre-release' } elseif ($r.isLatest) { 'latest' } else { 'release' }
        $name = if ($r.name.Length -gt 38) { $r.name.Substring(0, 37) + '…' } else { $r.name }
        Write-Color ('  ' + $r.tagName.PadRight($tagWidth)) violet_light -NoNewline
        Write-Color $name.PadRight(40) ink -NoNewline
        Write-Color ([datetime]$r.publishedAt).ToString('yyyy-MM-dd').PadRight(14) muted -NoNewline
        Write-Color $kind $(if ($kind -eq 'latest') { 'success' } elseif ($kind -like 'assets*') { 'teal' } else { 'muted' })
    }
    Write-Rule ($tagWidth + 76)
}

function Invoke-Releases {
    $repo = [pscustomobject]@{ Slug = 'etherealkeshii/Aurora-Viewer' }
    if (-not $repo) { Write-Color '  GitHub repository not found (remote origin).' danger; Wait-Back; return }
    while ($true) {
        Show-Logo 'Releases'
        Write-Color "  Repository: $($repo.Slug)" muted
        Write-Color '  Reading the releases and your rights on GitHub…' muted_dim
        $releases = Get-ReleaseList $repo
        $rights = Test-ReleaseRights $repo
        if (-not $rights) {
            Show-Logo 'Releases'
            Write-Color "  Repository: $($repo.Slug)" muted
            Show-ReleaseList $releases
            Write-Color '  Your GitHub account has no write access to this repository: creating and deleting are unavailable.' warn
            Wait-Back
            return
        }
        $pick = Show-Menu 'Manage releases' @(
            @{ Key = '1'; Label = 'Create a release'; Hint = 'Keshii version tag on the distribution branch, published by CI' }
            @{ Key = '2'; Label = 'Delete a release'; Hint = 'release and tag, cannot be undone' }
        ) { Show-Logo 'Releases'; Write-Color "  Repository: $($repo.Slug)" muted; Show-ReleaseList $releases }
        switch ($pick.Key) {
            '1' { New-AuroraRelease $repo }
            '2' { Remove-AuroraRelease $repo $releases }
            default { return }
        }
    }
}

function New-AuroraRelease($Repo) {
    Show-Logo 'New release'
    if (-not (Test-ReleaseRights $Repo)) { Write-Color '  Release permission denied.' danger; Wait-Back; return }
    $remote = git -C $Paths.Repo remote get-url --push fork
    if ($remote -notin @('https://github.com/etherealkeshii/Aurora-Viewer.git', 'https://github.com/etherealkeshii/Aurora-Viewer', 'git@github.com:etherealkeshii/Aurora-Viewer.git')) {
        Write-Color '  Unsafe release destination.' danger; Wait-Back; return
    }
    git -C $Paths.Repo fetch fork --quiet
    if ($LASTEXITCODE -ne 0) { Write-Color '  Could not fetch the distribution branch.' danger; Wait-Back; return }
    $commit = git -C $Paths.Repo rev-parse --verify refs/remotes/fork/keshii/distribution
    if ($LASTEXITCODE -ne 0) { Write-Color '  Distribution branch not found.' danger; Wait-Back; return }
    Write-Color "  Distribution commit: $commit" muted
    $ctx = Get-ReleaseContext $Repo
    $summary = "$($ctx.Prs.Count) merged PRs"
    git -C $Paths.Repo fetch origin --tags --quiet
    if ($LASTEXITCODE -ne 0) {
        Write-Color '  Could not fetch upstream Aurora version tags.' danger
        Wait-Back; return
    }

    $tags = @(git -C $Paths.Repo tag --list)
    if ($LASTEXITCODE -ne 0) {
        throw 'Could not retrieve version tags.'
    }

    $suggested = Get-NextKeshiiBetaTag $tags ([datetime]::UtcNow)

    $items = @(
        @{ Key = '1'; Label = $suggested; Hint = 'next Aurora beta version with Keshii suffix'; Tag = $suggested }
        @{ Key = 'n'; Label = 'Other number…'; Hint = 'enter the Aurora version number'; Tag = $null }
    )
    $pick = Show-Menu 'Step 1/4 · Which number?' $items {
        Show-Logo 'New release'
        Write-Color "  Last release: $(if ($ctx.Last) { $ctx.Last.Tag } else { 'none' })" muted
        Write-Color "  PRs merged since: $($ctx.Prs.Count) ($summary)" muted
        $limit = if ($Interactive) { [Math]::Max(0, [Math]::Min(12, $Host.UI.RawUI.WindowSize.Height - 17)) } else { 12 }
        foreach ($p in $ctx.Prs | Select-Object -First $limit) {
            $line = "    #$($p.number)".PadRight(10) + "$($p.title)"
            if ($Interactive -and $line.Length -ge $Host.UI.RawUI.WindowSize.Width) { $line = $line.Substring(0, [Math]::Max(0, $Host.UI.RawUI.WindowSize.Width - 2)) + '…' }
            Write-Color $line muted_dim
        }
        if ($ctx.Prs.Count -gt $limit) { Write-Color "    … and $($ctx.Prs.Count - $limit) more" muted_dim }
    }
    if (-not $pick) { return }
    $tag = $pick.Tag
    if (-not $tag) {
        $tag = Read-Text 'Aurora version number (empty: cancel):'
        if (-not $tag) { return }
        if ($tag -notmatch '^v') { $tag = "v$tag" }
        if (-not (ConvertTo-Version $tag)) { Write-Color "  '$tag' is not a SemVer number." danger; Wait-Back; return }
    }
    if ($tag -cnotmatch '-Keshii$') { $tag += '-Keshii' }
    if (git -C $Paths.Repo tag --list $tag) { Write-Color "  The tag $tag already exists." danger; Wait-Back; return }
    $policy = Get-UpdatePolicy -RepoPath $Paths.Repo -Tag $tag -BaseTag $(if ($ctx.Last) { $ctx.Last.Tag }) -Ref $commit
    $policy = Read-ReleaseMaintenance $policy
    if (-not $policy) { return }
    Show-Logo 'Confirmation of the release'
    Write-Color "  $tag, from distribution commit $commit." ink
    Write-Color '  Windows, Linux and macOS must all build successfully before publication.' ink
    Write-Color '  The completed release will be marked Latest.' teal
    Write-Color "  Update: $(if ($policy.minimum_supported_version) { 'required for versions before ' + $policy.minimum_supported_version + " ($(Get-GraceText $policy.grace_hours) until they update)" } else { 'optional for every version' })." ink
    Write-Color "  Cache: $(if ($policy.cache_invalidation_before) { 'cleared for versions before ' + $policy.cache_invalidation_before } else { 'kept' })." ink
    Write-Color "  Settings: $(if ($policy.settings_reset_before) { 'reset for versions before ' + $policy.settings_reset_before } else { 'kept and migrated automatically if needed' })." ink
    if (-not (Read-YesNo "Create the release ${tag}?")) { return }

    $message = "Aurora Viewer-Keshii $($tag.TrimStart('v'))`n`nAurora-Update-Policy: $($policy | ConvertTo-Json -Compress -Depth 4)"

    if ($DryRun) {
        Write-Color "  [dry run] tag $tag on $commit with policy: $($policy | ConvertTo-Json -Compress -Depth 4)" teal
        Wait-Back; return
    }
    $tagMessageFile = [IO.Path]::GetTempFileName()
    try {
        [IO.File]::WriteAllText($tagMessageFile, $message, [Text.UTF8Encoding]::new($false))
        git -C $Paths.Repo tag -a $tag $commit -F $tagMessageFile
        $tagExit = $LASTEXITCODE
    } finally {
        Remove-Item -LiteralPath $tagMessageFile -Force
    }
    if ($tagExit -ne 0) { Write-Color '  Could not create the tag.' danger; Wait-Back; return }
    git -C $Paths.Repo push fork "refs/tags/$tag"
    if ($LASTEXITCODE -ne 0) {
        git -C $Paths.Repo tag -d $tag | Out-Null
        Write-Color '  GitHub refused the tag push: nothing was published.' danger; Wait-Back; return
    }
    Write-Color "  Tag $tag pushed. Building and publishing the release…" success
    $id = $null
    for ($i = 0; $i -lt 30 -and -not $id; $i++) {
        Start-Sleep -Seconds 2
        $id = Invoke-GhJson @('run', 'list', '-R', $Repo.Slug, '--workflow', 'Release', '--limit', '5', '--json', 'databaseId,headBranch') |
            Where-Object { $_.headBranch -eq $tag } | Select-Object -First 1 -ExpandProperty databaseId
    }
    if ($id) {
        $releaseSucceeded = $false
        $maxAttempts = 5

        while ($true) {
            gh run watch $id -R $Repo.Slug --exit-status
            if ($LASTEXITCODE -eq 0) {
                $releaseSucceeded = $true
                break
            }

            if ($Repo.Slug -cne 'etherealkeshii/Aurora-Viewer') { break }

            $run = Invoke-GhJson @('api', "repos/$($Repo.Slug)/actions/runs/$id")
            if (-not $run -or $run.status -ne 'completed') { break }
            if ($run.run_attempt -ge $maxAttempts) {
                Write-Color '  Maximum release attempts reached.' danger
                break
            }

            $jobData = Invoke-GhJson @('api', "repos/$($Repo.Slug)/actions/runs/$id/attempts/$($run.run_attempt)/jobs?per_page=100")
            $retryJob = $null

            foreach ($job in $jobData.jobs) {
                if ($job.name -notin @('windows', 'linux', 'macos')) { continue }
                if ($job.conclusion -notin @('cancelled', 'failure')) { continue }
                if (@($job.steps | Where-Object { $_.name -eq 'Build' -and $_.started_at }).Count -gt 0) { continue }

                $annotations = Invoke-GhJson @('api', "repos/$($Repo.Slug)/check-runs/$($job.id)/annotations?per_page=100")
                if (@($annotations | Where-Object {
                    $_.message -match 'not acquired by Runner of type hosted'
                }).Count -gt 0) {
                    $retryJob = $job
                    break
                }
            }

            if (-not $retryJob) { break }

            Write-Color "  Hosted runner unavailable for $($retryJob.name)." warn
            Write-Color "  Retrying platform job in 60 seconds (attempt $($run.run_attempt + 1) of $maxAttempts)." teal
            Start-Sleep -Seconds 60

            $currentRun = Invoke-GhJson @('api', "repos/$($Repo.Slug)/actions/runs/$id")
            if (-not $currentRun -or $currentRun.status -ne 'completed' -or $currentRun.run_attempt -ne $run.run_attempt) { break }

            gh api --method POST "repos/$($Repo.Slug)/actions/jobs/$($retryJob.id)/rerun"
            if ($LASTEXITCODE -ne 0) {
                Write-Color '  GitHub refused the job retry.' danger
                break
            }

            Start-Sleep -Seconds 10
        }

        if ($releaseSucceeded) {
            $url = gh release view $tag -R $Repo.Slug --json url --jq .url 2>$null
            Write-Color "  Release published: $url" success
            if ($url -and (Read-YesNo 'Open it in the browser?')) { Open-External $url }
        } else {
            Write-Color "  Release workflow failed. Inspect the failed jobs: gh run view $id -R $($Repo.Slug) --log-failed" danger
            Write-Color '  No partial platform release should be published.' warn
        }
    } else {
        Write-Color '  Release workflow not found yet: follow it on GitHub (Actions tab).' warn
    }
    Wait-Back
}

function Remove-AuroraRelease($Repo, $Releases) {
    if (-not (Test-ReleaseRights $Repo)) { Write-Color '  Release permission denied.' danger; Wait-Back; return }
    if (-not $Releases) { Write-Color '  No release to delete.' muted; Wait-Back; return }
    $shortcuts = @(1..9 | ForEach-Object { [string]$_ }) + @(97..122 | ForEach-Object { [string][char]$_ })
    $items = @()
    for ($i = 0; $i -lt $Releases.Count -and $i -lt $shortcuts.Count; $i++) {
        $r = $Releases[$i]
        $items += @{ Key = $shortcuts[$i]; Label = $r.tagName; Hint = $r.name; Release = $r }
    }
    $pick = Show-Menu 'Which release to delete?' $items { Show-Logo 'Delete a release' }
    if (-not $pick) { return }
    $tag = $pick.Release.tagName
    # releases that hold assets (assets-1: the 3D emoji font that older commits download)
    if ($tag -cnotmatch '^v?[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?-Keshii$') {
        Write-Color "  $tag is not a version: it holds files older commits need (the 3D emoji font for assets-1). Protected." warn
        Wait-Back; return
    }
    Write-Host ''
    Write-Color "  Deleting $tag erases the release, its files and the tag, on GitHub and here. This cannot be undone." warn
    $typed = Read-Text "To confirm, type '$tag' (empty: cancel):"
    if ($typed -cne $tag) { Write-Color '  Cancelled: nothing was deleted.' muted; Wait-Back; return }
    if ($DryRun) {
        Write-Color "  [dry run] gh release delete $tag --cleanup-tag --yes ; git tag -d $tag" teal
        Wait-Back; return
    }
    gh release delete $tag -R $Repo.Slug --cleanup-tag --yes
    if ($LASTEXITCODE -eq 0) {
        git -C $Paths.Repo tag -d $tag 2>$null | Out-Null
        Write-Color "  Release $tag deleted." success
    } else {
        Write-Color '  GitHub refused the deletion (see above).' danger
    }
    Wait-Back
}
