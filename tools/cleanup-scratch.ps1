<#
.SYNOPSIS
  Remove Locust scratch folders under %TEMP% and keep the shared cargo target small.

.DESCRIPTION
  Scratch roots are named locust-<cycle>-<role> (for example locust-c113-unity-audit).
  A folder is removed when it is older than -MinAgeHours and has no ".keep" file inside.
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
  [string]$Repo = 'C:\Projects\Locust'
)

$ErrorActionPreference = 'Continue'

function Get-DirGB([string]$Path) {
  $sum = (Get-ChildItem -LiteralPath $Path -Recurse -Force -File -ErrorAction SilentlyContinue |
    Measure-Object Length -Sum).Sum
  if ($null -eq $sum) { return 0 }
  return [math]::Round($sum / 1GB, 2)
}

$cutoff = (Get-Date).AddHours(-$MinAgeHours)
$freed = 0.0

Get-ChildItem -LiteralPath $env:TEMP -Directory -Filter "$Prefix*" -ErrorAction SilentlyContinue | ForEach-Object {
  $dir = $_
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
