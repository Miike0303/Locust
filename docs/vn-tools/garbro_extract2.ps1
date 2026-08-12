$ErrorActionPreference = "Continue"
$g = "C:\Program Files (x86)\GARbro"
Set-Location $g
[Reflection.Assembly]::LoadFrom("$g\GameRes.dll") | Out-Null
[Reflection.Assembly]::LoadFrom("$g\ArcFormats.dll") | Out-Null
$cat = [GameRes.FormatCatalog]::Instance
try { $fs=[IO.File]::OpenRead("$g\GameData\Formats.dat"); $cat.DeserializeScheme($fs); $fs.Close() } catch {}
$xp3 = $cat.ArcFormats | Where-Object { $_.Tag -eq "XP3" } | Select-Object -First 1

$TARGET     = $args[0]
$SCHEMENAME = $args[1]
$OUT        = $args[2]
New-Item -ItemType Directory -Force -Path $OUT | Out-Null

$optType = [GameRes.Formats.KiriKiri.Xp3Options]
$script:chosen = [GameRes.Formats.KiriKiri.Xp3Opener]::GetScheme($SCHEMENAME)
$handler = [GameRes.ParametersRequestEventHandler]{
    param($s, $e)
    $o = [Activator]::CreateInstance($optType)
    $o.Version = 2; $o.Scheme = $script:chosen
    $o.CompressIndex = $false; $o.CompressContents = $false; $o.RetainDirs = $true
    $e.Options = $o; $e.InputResult = $true
}
$cat.add_ParametersRequest($handler)

$arc = [GameRes.ArcFile]::TryOpen($TARGET)
if ($null -eq $arc) { Write-Host "FAILED to open $TARGET with scheme '$SCHEMENAME'"; exit 1 }

$n = 0
foreach ($ent in $arc.Dir) {
    if ($ent.Name -notmatch '\.(ks|txt|scn|asd)$') { continue }
    $flat = ($ent.Name -replace '[\\/]', '_')
    try {
        $st = $arc.OpenEntry($ent)
        $ms = New-Object IO.MemoryStream
        $st.CopyTo($ms); $st.Close()
        [IO.File]::WriteAllBytes((Join-Path $OUT $flat), $ms.ToArray())
        $ms.Close(); $n++
    } catch {}
}
$arc.Dispose()
Write-Host ("extracted text entries: " + $n + " -> " + $OUT)
