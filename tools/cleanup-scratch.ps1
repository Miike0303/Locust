<#
.SYNOPSIS
  Remove Locust scratch and test leftovers and keep the shared cargo target small.

.DESCRIPTION
  Scratch roots are named locust-<cycle>-<role> (for example locust-c113-unity-audit).
  A folder is removed when it is older than -MinAgeHours and has no ".keep" file inside.
  Also removes aged entries in <Repo>\.test-tmp (except .gitignore) and legacy
  %TEMP%\locust_* test folders ending in a UUID or numeric suffix, never locust_bak.
  The shared cargo target (see -SharedTarget) is deleted when it exceeds -TargetCapGB.
  Dry run by default; pass -Apply to delete. Always ends with `git worktree prune`.

.EXAMPLE
  powershell -NoProfile -File tools/cleanup-scratch.ps1            # report only
  powershell -NoProfile -File tools/cleanup-scratch.ps1 -Apply     # delete
  powershell -NoProfile -File tools/cleanup-scratch.ps1 -Apply -MinAgeHours 0 -Prefix locust-c113-
#>
param(
  [switch]$Apply,
  [double]$MinAgeHours = 2,
  [string]$Prefix = 'locust-',
  [string]$SharedTarget = (Join-Path $env:LOCALAPPDATA 'locust-shared-target'),
  [double]$TargetCapGB = 15,
  [string]$Repo = (Split-Path -Parent $PSScriptRoot)
)

$ErrorActionPreference = 'Continue'

function Get-DirGB([string]$Path) {
  $sum = (Get-ChildItem -LiteralPath $Path -Recurse -Force -File -ErrorAction SilentlyContinue |
    Measure-Object Length -Sum).Sum
  if ($null -eq $sum) { return 0 }
  return [math]::Round($sum / 1GB, 2)
}

function Remove-TestEntry($Entry, [string]$Root) {
  # Only direct children of the named root are eligible for recursive removal.
  $rootPath = [IO.Path]::GetFullPath($Root).TrimEnd('\', '/')
  $entryPath = [IO.Path]::GetFullPath($Entry.FullName)
  if ((Split-Path -Parent $entryPath) -ne $rootPath) { throw "Outside cleanup root: $entryPath" }
  if ($Entry.Attributes -band [IO.FileAttributes]::ReparsePoint) { "skip (link): $entryPath"; return }
  if ($Entry.LastWriteTime -gt $cutoff) { "skip (recent): $entryPath"; return }
  $gb = Get-DirGB $entryPath
  if ($Apply) {
    Remove-Item -LiteralPath $entryPath -Recurse -Force -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $entryPath) { "LEFT (in use?): $entryPath $gb GB" }
    else { "removed: $entryPath $gb GB"; $script:freed += $gb }
  } else {
    "would remove: $entryPath $gb GB"; $script:freed += $gb
  }
}

$cutoff = (Get-Date).AddHours(-$MinAgeHours)
$freed = 0.0
$legacySuffix = '^locust_(?:.*_)?(?:[0-9]+|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$'

Get-ChildItem -LiteralPath $env:TEMP -Directory -Filter "$Prefix*" -ErrorAction SilentlyContinue | ForEach-Object {
  $dir = $_
  if ($dir.Name -eq 'locust_bak') { "keep (legacy backup root): $($dir.Name)"; return }
  if ($dir.Name -like 'locust_*' -and $dir.Name -notmatch $legacySuffix) { "keep (no test suffix): $($dir.Name)"; return }
  if (Test-Path -LiteralPath (Join-Path $dir.FullName '.keep')) { "keep (marker): $($dir.Name)"; return }
  if ($dir.LastWriteTime -gt $cutoff) { "skip (recent): $($dir.Name)"; return }
  $gb = Get-DirGB $dir.FullName
  if ($Apply) {
    Remove-Item -LiteralPath $dir.FullName -Recurse -Force -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $dir.FullName) { "LEFT (in use?): $($dir.Name) $gb GB" }
    else { "removed: $($dir.Name) $gb GB"; $freed += $gb }
  } else {
    "would remove: $($dir.Name) $gb GB"; $freed += $gb
  }
}

$testTemp = Join-Path $Repo '.test-tmp'
Get-ChildItem -LiteralPath $testTemp -Force -ErrorAction SilentlyContinue |
  Where-Object { $_.Name -ne '.gitignore' } |
  ForEach-Object { Remove-TestEntry $_ $testTemp }

Get-ChildItem -LiteralPath $env:TEMP -Directory -Filter 'locust_*' -ErrorAction SilentlyContinue |
  Where-Object { $_.Name -ne 'locust_bak' -and $_.Name -match $legacySuffix -and $_.Name -notlike "$Prefix*" } |
  ForEach-Object { Remove-TestEntry $_ $env:TEMP }

if (Test-Path -LiteralPath $SharedTarget) {
  $tgb = Get-DirGB $SharedTarget
  if ($tgb -gt $TargetCapGB) {
    if ($Apply) {
      Remove-Item -LiteralPath $SharedTarget -Recurse -Force -ErrorAction SilentlyContinue
      "shared target over cap ($tgb GB > $TargetCapGB GB): removed"; $freed += $tgb
    } else { "shared target over cap ($tgb GB > $TargetCapGB GB): would remove" }
  } else { "shared target ok: $tgb GB (cap $TargetCapGB GB)" }
}

if (Test-Path -LiteralPath (Join-Path $Repo '.git')) { git -C $Repo worktree prune }
"freed GB: $([math]::Round($freed, 2)) | free on C: $([math]::Round((Get-PSDrive C).Free / 1GB, 1)) GB"
