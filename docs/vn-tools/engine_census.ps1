# Engine census over a game library: which engine is each game built on?
#
#   .\engine_census.ps1 -Root D:\juegos -Out census.csv
#
# Detection is strictly LOCAL to a candidate dir (no deep recursion), so container
# folders like rpgm\en are never mistaken for a single game. Containers are then
# removed by dropping any dir that is an ancestor of a classified game.
#
# Two traps this exists to avoid, both of which produce confidently wrong counts:
#   1. RPG Maker MV/MZ and Electron builds ship Chromium resource paks
#      (resources.pak, chrome_100_percent.pak, locales\*.pak). A bare *.pak glob
#      reports them as Unreal games.
#   2. Running a RECURSIVE detector on a container dir matches a descendant and
#      swallows the whole tree, collapsing a hundred games into a handful.
param(
    [string] $Root = 'D:\juegos',
    [string] $Out  = 'census.csv',
    # Regex of paths to treat as non-games: patch workspaces, extracted output,
    # anything that looks like a game but is not one you want to translate.
    [string] $ExcludePath = '\\parches\\|\\locust-tests\\'
)

$CHROMIUM_PAK = '^(resources|chrome_\d+_percent|snapshot_blob|v8_context_snapshot|icudtl)\.pak$'

function LocalFile([string]$p, [string]$filter) {
    return [bool](Get-ChildItem -LiteralPath $p -Filter $filter -File -EA 0 | Select-Object -First 1)
}

function Get-Engine([string]$p) {
    # --- Ren'Py ---
    if (Test-Path -LiteralPath (Join-Path $p 'renpy\common')) { return 'renpy' }
    $g = Join-Path $p 'game'
    if (Test-Path -LiteralPath $g) {
        if ((LocalFile $g '*.rpa') -or (LocalFile $g '*.rpyc') -or (LocalFile $g '*.rpy')) { return 'renpy' }
    }

    # --- RPG Maker MV / MZ ---
    foreach ($base in @($p, (Join-Path $p 'www'))) {
        if (Test-Path -LiteralPath (Join-Path $base 'data\System.json')) {
            if (Test-Path -LiteralPath (Join-Path $base 'js\rmmz_core.js')) { return 'rpgm_mz' }
            if (Test-Path -LiteralPath (Join-Path $base 'js\rpg_core.js'))  { return 'rpgm_mv' }
            return 'rpgm_mvmz'
        }
    }

    # --- RPG Maker XP / VX / VX Ace ---
    if (LocalFile $p '*.rgss3a') { return 'rpgm_vxace' }
    if (LocalFile $p '*.rgss2a') { return 'rpgm_vx' }
    if (LocalFile $p '*.rgssad') { return 'rpgm_xp' }
    $dataDir = Join-Path $p 'Data'
    if (Test-Path -LiteralPath $dataDir) {
        if (LocalFile $dataDir '*.rvdata2') { return 'rpgm_vxace' }
        if (LocalFile $dataDir '*.rvdata')  { return 'rpgm_vx' }
        if (LocalFile $dataDir '*.rxdata')  { return 'rpgm_xp' }
    }

    # --- Unity ---
    foreach ($d in (Get-ChildItem -LiteralPath $p -Directory -Filter '*_Data' -EA 0)) {
        if ((Test-Path -LiteralPath (Join-Path $d.FullName 'globalgamemanagers')) -or
            (Test-Path -LiteralPath (Join-Path $d.FullName 'resources.assets'))   -or
            (Test-Path -LiteralPath (Join-Path $d.FullName 'data.unity3d'))) { return 'unity' }
    }

    # --- Unreal: <Game>/Content/Paks/*.pak (packaged layout) ---
    foreach ($d in (Get-ChildItem -LiteralPath $p -Directory -EA 0)) {
        $paks = Join-Path $d.FullName 'Content\Paks'
        if ((Test-Path -LiteralPath $paks) -and (LocalFile $paks '*.pak')) { return 'unreal' }
    }
    # loose non-Chromium .pak sitting directly in the game root
    if (Get-ChildItem -LiteralPath $p -Filter '*.pak' -File -EA 0 |
        Where-Object { $_.Name -notmatch $CHROMIUM_PAK } | Select-Object -First 1) { return 'unreal' }

    # --- Others ---
    if (LocalFile $p '*.pck') { return 'godot' }
    if (LocalFile $p '*.xp3') { return 'kirikiri' }
    if (LocalFile $p '*.ypf') { return 'yuris' }
    if ((Test-Path -LiteralPath (Join-Path $p 'pac')) -and (LocalFile (Join-Path $p 'pac') '*.ypf')) { return 'yuris' }
    if (LocalFile $p '*.wolf') { return 'wolf_rpg' }
    if ((Test-Path -LiteralPath $dataDir) -and (LocalFile $dataDir '*.wolf')) { return 'wolf_rpg' }
    if (Test-Path -LiteralPath (Join-Path $p 'index.html')) { return 'html' }
    return 'unknown'
}

# Collect every candidate dir down to depth 3 (D:\juegos\rpgm\en\<game>)
$cands = @()
foreach ($d1 in (Get-ChildItem $Root -Directory -EA 0)) {
    $cands += $d1
    foreach ($d2 in (Get-ChildItem -LiteralPath $d1.FullName -Directory -EA 0)) {
        $cands += $d2
        foreach ($d3 in (Get-ChildItem -LiteralPath $d2.FullName -Directory -EA 0)) { $cands += $d3 }
    }
}

# Dirs that are engine PARTS or mod payloads, never games in their own right.
# `locales` holds Electron/NW.js locale paks, which otherwise read as Unreal.
$SKIP_NAME = '^(locales|ModFiles|OriginalFiles|pac|www|game|Data|renpy|lib)$'

$classified = @()
foreach ($c in $cands) {
    if ($c.Name -match $SKIP_NAME)           { continue }
    if ($c.FullName -match $ExcludePath)     { continue }
    $eng = Get-Engine $c.FullName
    if ($eng -ne 'unknown') { $classified += [pscustomobject]@{ Engine = $eng; Name = $c.Name; Path = $c.FullName } }
}

# Drop containers: any classified dir that is an ancestor of another classified dir
$paths = $classified.Path
$games = $classified | Where-Object {
    $self = $_.Path
    -not ($paths | Where-Object { $_ -ne $self -and $_.StartsWith($self + '\') } | Select-Object -First 1)
}

$games | Sort-Object Engine, Name | Export-Csv -NoTypeInformation -Encoding UTF8 $Out
$games | Group-Object Engine | Sort-Object Count -Descending | Select-Object Count, Name | Format-Table -AutoSize
"TOTAL GAMES: $($games.Count)"
