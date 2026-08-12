$ErrorActionPreference = "Continue"
$g = "C:\Program Files (x86)\GARbro"
Set-Location $g
[Reflection.Assembly]::LoadFrom("$g\GameRes.dll") | Out-Null
[Reflection.Assembly]::LoadFrom("$g\ArcFormats.dll") | Out-Null
$cat = [GameRes.FormatCatalog]::Instance
try { $fs=[IO.File]::OpenRead("$g\GameData\Formats.dat"); $cat.DeserializeScheme($fs); $fs.Close() } catch {}
$xp3 = $cat.ArcFormats | Where-Object { $_.Tag -eq "XP3" } | Select-Object -First 1

$TARGET = $args[0]
$OUTDIR = $args[1]
New-Item -ItemType Directory -Force -Path $OUTDIR | Out-Null

$optType = [GameRes.Formats.KiriKiri.Xp3Options]
$script:chosen = $null
$handler = [GameRes.ParametersRequestEventHandler]{
    param($s, $e)
    $o = [Activator]::CreateInstance($optType)
    $o.Version = 2; $o.Scheme = $script:chosen
    $o.CompressIndex = $false; $o.CompressContents = $false; $o.RetainDirs = $true
    $e.Options = $o; $e.InputResult = $true
}
$cat.add_ParametersRequest($handler)
$scheme = $xp3.GetType().GetProperty("Scheme").GetValue($xp3)
$names = @(($scheme.GetType().GetField("KnownSchemes").GetValue($scheme)).Keys)
Write-Host ("schemes: " + $names.Count)

$i = 0; $saved = 0
foreach ($name in $names) {
    $i++
    $script:chosen = [GameRes.Formats.KiriKiri.Xp3Opener]::GetScheme($name)
    if ($script:chosen -is [GameRes.Formats.KiriKiri.NoCrypt]) { continue }
    try {
        $arc = [GameRes.ArcFile]::TryOpen($TARGET)
        if ($null -eq $arc) { continue }
        $ent = $arc.Dir | Where-Object { $_.Name -match '\.ks$' } | Sort-Object Size -Descending | Select-Object -First 1
        if ($null -eq $ent) { $ent = $arc.Dir | Where-Object { $_.Name -match '\.(scn|txt)$' } | Sort-Object Size -Descending | Select-Object -First 1 }
        if ($null -ne $ent) {
            $st = $arc.OpenEntry($ent)
            $buf = New-Object byte[] 131072
            $r = $st.Read($buf, 0, 131072); $st.Close()
            $safe = ($name -replace '[^\w]', '_')
            [IO.File]::WriteAllBytes((Join-Path $OUTDIR ("{0:D3}__{1}.bin" -f $i, $safe)), $buf[0..($r-1)])
            $saved++
        }
        $arc.Dispose()
    } catch {}
    if ($i % 90 -eq 0) { Write-Host ("  ...$i") }
}
Write-Host ("saved samples: " + $saved + " -> " + $OUTDIR)
