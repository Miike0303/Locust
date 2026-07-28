$ErrorActionPreference = "Continue"
$g = "C:\Program Files (x86)\GARbro"
Set-Location $g
[Reflection.Assembly]::LoadFrom("$g\GameRes.dll") | Out-Null
[Reflection.Assembly]::LoadFrom("$g\ArcFormats.dll") | Out-Null
$cat = [GameRes.FormatCatalog]::Instance
try { $fs=[IO.File]::OpenRead("$g\GameData\Formats.dat"); $cat.DeserializeScheme($fs); $fs.Close() } catch {}

$TARGET     = $args[0]
$OUT        = $args[1]
$SCHEMENAME = $args[2]   # optional; omit for unencrypted
New-Item -ItemType Directory -Force -Path $OUT | Out-Null

$optType = [GameRes.Formats.KiriKiri.Xp3Options]
if ($SCHEMENAME) { $script:chosen = [GameRes.Formats.KiriKiri.Xp3Opener]::GetScheme($SCHEMENAME) }
else { $script:chosen = $null }
$handler = [GameRes.ParametersRequestEventHandler]{
    param($s, $e)
    $o = [Activator]::CreateInstance($optType)
    $o.Version = 2; $o.Scheme = $script:chosen
    $o.CompressIndex = $false; $o.CompressContents = $false; $o.RetainDirs = $true
    $e.Options = $o; $e.InputResult = $true
}
$cat.add_ParametersRequest($handler)

$arc = [GameRes.ArcFile]::TryOpen($TARGET)
if ($null -eq $arc) { Write-Host "FAILED to open $TARGET"; exit 1 }

$n = 0
foreach ($ent in $arc.Dir) {
    if ($ent.Name -notmatch '\.(ks|txt|scn|asd)$') { continue }
    # PRESERVE the internal path (only normalize separators)
    $rel = $ent.Name -replace '/', '\'
    $dest = Join-Path $OUT $rel
    New-Item -ItemType Directory -Force -Path ([IO.Path]::GetDirectoryName($dest)) | Out-Null
    try {
        $st = $arc.OpenEntry($ent)
        $ms = New-Object IO.MemoryStream
        $st.CopyTo($ms); $st.Close()
        [IO.File]::WriteAllBytes($dest, $ms.ToArray())
        $ms.Close(); $n++
    } catch {}
}
$arc.Dispose()
Write-Host ("extracted (paths preserved): " + $n + " -> " + $OUT)
