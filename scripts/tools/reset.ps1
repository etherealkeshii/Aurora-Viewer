# Aurora Tools: reset the repository to the state of a fresh clone of
# GitHub's main (dot-sourced by aurora-tools.ps1). What would be lost is
# listed first; nothing happens without typing the confirmation word.
# $DryRun (aurora-tools.ps1 -DryRun) stops after the list.
#
# The folder itself is kept: the tools run from it, and Windows does not
# delete a folder a program has open. A hard reset plus a clean of every
# untracked and ignored file leaves exactly what a clone holds.

$ResetWord = 'RESET'

# Commits of $Branch that exist nowhere on GitHub: ahead of its upstream, or
# not in origin/main when it was never pushed. A branch whose upstream was
# deleted on GitHub (merged PR) is safe.
function Get-UnpushedCount([string]$Repo, [string]$Branch, [string]$Upstream) {
    if ($Upstream) {
        git -C $Repo rev-parse --verify --quiet "refs/remotes/$Upstream" *> $null
        if ($LASTEXITCODE -ne 0) { return 0 }
        return [int](git -C $Repo rev-list --count "$Upstream..$Branch" 2>$null)
    }
    [int](git -C $Repo rev-list --count "origin/main..$Branch" 2>$null)
}

# A path as this system writes it, for comparing two of them.
function Get-NormalPath([string]$Path) {
    [IO.Path]::GetFullPath($Path).TrimEnd([IO.Path]::DirectorySeparatorChar)
}

function Get-ResetInventory {
    $repo = $Paths.Repo
    git -C $repo fetch origin --prune --quiet 2>$null
    $changes = @(git -C $repo status --porcelain 2>$null)
    $branches = @(foreach ($line in git -C $repo for-each-ref --format='%(refname:short)|%(upstream:short)' refs/heads) {
        $name, $up = $line -split '\|', 2
        $n = Get-UnpushedCount $repo $name $up
        if ($n -gt 0) { [pscustomobject]@{ Branch = $name; Count = $n; Pushed = [bool]$up } }
    })
    $stashes = @(git -C $repo stash list 2>$null)
    # ignored files that are not builds or the old downloaded emoji font (.env, editor settings…)
    $ignored = @(git -C $repo clean -ndX 2>$null | ForEach-Object { $_ -replace '^Would remove ', '' } |
        Where-Object { $_ -notmatch '^(target/|\.scratch/|assets/emoji/)' })
    $worktrees = @()
    $current = $null
    foreach ($line in git -C $repo worktree list --porcelain) {
        if ($line -like 'worktree *') { $current = [pscustomobject]@{ Path = (Get-NormalPath $line.Substring(9)); Branch = $null } ; $worktrees += $current }
        elseif ($line -like 'branch *' -and $current) { $current.Branch = $line.Substring(7) -replace '^refs/heads/', '' }
    }
    $worktrees = @($worktrees | Where-Object { $_.Path -ne (Get-NormalPath $repo) } | ForEach-Object {
        $up = if ($_.Branch) { (git -C $repo for-each-ref --format='%(upstream:short)' "refs/heads/$($_.Branch)") }
        [pscustomobject]@{
            Path = $_.Path
            Name = Split-Path $_.Path -Leaf
            Branch = $_.Branch
            Changes = @(git -C $_.Path status --porcelain 2>$null).Count
            Unpushed = if ($_.Branch) { Get-UnpushedCount $repo $_.Branch $up } else { 0 }
        }
    })
    [pscustomobject]@{ Changes = $changes; Branches = $branches; Stashes = $stashes; Ignored = $ignored; Worktrees = $worktrees }
}

# $true when something would be lost.
function Show-ResetInventory($Inv) {
    $lost = $false
    Write-Rule 90
    Write-Color '  What would be lost' ink
    Write-Rule 90
    if ($Inv.Changes) {
        $lost = $true
        Write-Color "  • $($Inv.Changes.Count) uncommitted change(s) in the repository:" warn
        foreach ($c in $Inv.Changes | Select-Object -First 8) { Write-Color "      $c" muted }
        if ($Inv.Changes.Count -gt 8) { Write-Color "      … and $($Inv.Changes.Count - 8) more" muted_dim }
    }
    foreach ($b in $Inv.Branches) {
        $lost = $true
        Write-Color "  • branch $($b.Branch): $($b.Count) commit(s) $(if ($b.Pushed) { 'not pushed yet' } else { 'never pushed to GitHub' })" warn
    }
    if ($Inv.Stashes) {
        $lost = $true
        Write-Color "  • $($Inv.Stashes.Count) stash(es) (set aside with git stash)" warn
    }
    foreach ($w in $Inv.Worktrees) {
        $risk = @()
        if ($w.Changes) { $risk += "$($w.Changes) uncommitted change(s)" }
        if ($w.Unpushed) { $risk += "$($w.Unpushed) commit(s) not on GitHub" }
        if ($risk) {
            $lost = $true
            Write-Color "  • task $($w.Name) ($($w.Branch)): $($risk -join ', ')" warn
        } else {
            Write-Color "  • task $($w.Name): nothing lost (everything is on GitHub), folder deleted" muted
        }
    }
    if ($Inv.Ignored) {
        $lost = $true
        Write-Color "  • files ignored by git, other than builds:" warn
        foreach ($f in $Inv.Ignored | Select-Object -First 8) { Write-Color "      $f" muted }
        if ($Inv.Ignored.Count -gt 8) { Write-Color "      … and $($Inv.Ignored.Count - 8) more" muted_dim }
    }
    if (-not $lost) { Write-Color '  Nothing: all your work is already on GitHub.' success }
    Write-Color '  Also erased, but rebuilt afterwards: builds (target), RELEASE folder.' muted_dim
    Write-Color '  Kept: your git identity, your GitHub login, the Firestorm sources (updated).' muted_dim
    Write-Rule 90
    $lost
}

function Invoke-Reset {
    Write-Title 'Reset the repository'
    Write-Color '  Repository reset is disabled in the Keshii distribution.' warn
    Write-Color '  This protects independent branches, release files and local development work.' muted
    Wait-Back
}
