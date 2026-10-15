param(
    [string]$OutputDirectory = 'tmp/native-japanese-vn/unicode-sandbox-20261014',
    [switch]$RunTests
)
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
function File-Sha256([string]$Path) {
    $hash = [Security.Cryptography.SHA256]::Create()
    $stream = [IO.File]::OpenRead($Path)
    try { return ([BitConverter]::ToString($hash.ComputeHash($stream))).Replace('-', '').ToLowerInvariant() }
    finally { $stream.Dispose(); $hash.Dispose() }
}
function Resolve-OwnedOutput([string]$Relative) {
    if ([string]::IsNullOrWhiteSpace($Relative) -or [IO.Path]::IsPathRooted($Relative)) {
        throw 'Output policy: repository-relative path required.'
    }
    $parts = $Relative -split '[\\/]'
    foreach ($part in $parts) {
        if ($part -notmatch '^[A-Za-z0-9_-][A-Za-z0-9_. -]*$' -or $part -match '[. ]$' -or
            $part -match '^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])([. ]|$)') {
            throw 'Output policy: unsafe component.'
        }
    }
    $full = [IO.Path]::GetFullPath((Join-Path $repo ($parts -join '\')))
    # Same logical namespace as build-proof.ps1: descendants only, not the root
    # itself or a similarly prefixed sibling. The host still pins its exact run.
    $root = [IO.Path]::GetFullPath((Join-Path $repo 'tmp/native-japanese-vn')) + '\'
    if (-not $full.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Output policy: expected a descendant of repo/tmp/native-japanese-vn.'
    }
    $ancestor = $full
    while ($ancestor) {
        try {
            $attributes = [IO.File]::GetAttributes($ancestor)
            if (($attributes -band [IO.FileAttributes]::ReparsePoint) -or
                -not ($attributes -band [IO.FileAttributes]::Directory)) {
                throw 'Output policy: reparse or non-directory ancestor.'
            }
        } catch [IO.FileNotFoundException] {
        } catch [IO.DirectoryNotFoundException] {
        }
        $ancestor = [IO.Path]::GetDirectoryName($ancestor)
    }
    return $full
}
$out = Resolve-OwnedOutput $OutputDirectory
New-Item -ItemType Directory -Force -Path $out | Out-Null
$attempt = Join-Path $out ('run-' + [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss-fffffff'))
New-Item -ItemType Directory -Path $attempt | Out-Null
# Retain each attempted source, including failed experiments; no filesystem cleanup.
foreach ($name in @('sandbox_probe.cpp', 'build-sandbox.ps1')) {
    [IO.File]::Copy((Join-Path $PSScriptRoot $name), (Join-Path $attempt $name), $false)
}
$metadata = [ordered]@{
    schema = 1; buildExit = $null; tests = @(); passed = $false
    scope = 'Owned profileless x86 fixture only; no games, profile APIs, registry writes, or file deletion'
    sources = @(); binaries = @(); limits = @{ hostWallSeconds = 30; logBytes = 65536 }
}
foreach ($name in @('sandbox_probe.cpp', 'build-sandbox.ps1', 'README.md')) {
    $path = Join-Path $PSScriptRoot $name
    $metadata.sources += @{ path = "tools/yuris-unicode-runtime/$name"; sha256 = File-Sha256 $path;
        lines = @(Get-Content -LiteralPath $path).Count }
}
function Save-Metadata {
    $metadata | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $attempt 'metadata.json') -Encoding UTF8
}
function Invoke-Build([string]$Name, [string]$Command) {
    $batch = @"
@echo off
set "TEMP=$attempt"
set "TMP=$attempt"
set "PATH=$(Split-Path -Parent $vswhere);%PATH%"
call "$vcvars" x86
if errorlevel 1 exit /b 1
cd /d "$attempt"
$Command
exit /b %errorlevel%
"@
    $path = Join-Path $attempt "$Name.cmd"
    [IO.File]::WriteAllText($path, $batch, [Text.Encoding]::ASCII)
    $ErrorActionPreference = 'Continue'
    & cmd.exe /d /c $path 2>&1 | Tee-Object -FilePath (Join-Path $attempt "$Name.log") | Write-Host
    $result = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    return $result
}
function Invoke-Probe([string]$Label, [string]$Mode) {
    # Fixed host/modes, no program selection or payload arguments. This outer
    # deadline complements the host's job/child deadlines and bounds diagnostics.
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = Join-Path $attempt 'sandbox_host.exe'
    $info.Arguments = $Mode
    $info.WorkingDirectory = $attempt
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $process = New-Object Diagnostics.Process
    $process.StartInfo = $info
    try {
        if (-not $process.Start()) { throw 'Could not start owned host.' }
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        $timedOut = -not $process.WaitForExit(30000)
        if ($timedOut) { $process.Kill(); $process.WaitForExit(5000) | Out-Null }
        if (-not $process.HasExited) { throw 'Owned host did not terminate after its deadline.' }
        $text = $stdout.GetAwaiter().GetResult() + $stderr.GetAwaiter().GetResult()
        $oversize = $text.Length -gt 65536
        if ($oversize) { $text = $text.Substring(0, 65536) + "`nFAIL log bound exceeded`n" }
        [IO.File]::WriteAllText((Join-Path $attempt "$Label.log"), $text, [Text.Encoding]::UTF8)
        Write-Host $text
        return [ordered]@{ label = $Label; mode = $Mode; exit = $process.ExitCode;
            timeout = $timedOut; logOverflow = $oversize; log = "$Label.log"; text = $text }
    } finally { $process.Dispose() }
}
try {
    if ($RunTests) {
        $pathChecks = 0
        # Pure resolver calls: independent destinations are validated, not created.
        foreach ($valid in @('tmp/native-japanese-vn/unicode-sandbox-verify-20261014',
            'tmp/native-japanese-vn/unicode-sandbox-20261014',
            'tmp\native-japanese-vn\fresh sandbox\nested-output',
            'TMP/NATIVE-JAPANESE-VN/Unicode-Sandbox-Verify-20261014')) {
            try { $resolved = Resolve-OwnedOutput $valid }
            catch { throw "Positive resolver check rejected safe descendant: $valid; $($_.Exception.Message)" }
            $expected = [IO.Path]::GetFullPath((Join-Path $repo $valid))
            if (-not $resolved.Equals($expected, [StringComparison]::OrdinalIgnoreCase)) { throw 'Resolver changed the destination.' }
            ++$pathChecks
        }
        foreach ($invalid in @('', '../tmp/native-japanese-vn/escape', 'tmp/native-japanese-vn/../escape',
            'tmp/native-japanese-vn', 'tmp/native-japanese-vn-sibling/escape', 'tmp/elsewhere/escape',
            'tmp/native-japanese-vn/a*', 'tmp/native-japanese-vn/a?', 'tmp/native-japanese-vn/[a]',
            'tmp/native-japanese-vn/a:stream', 'tmp/native-japanese-vn/a.', 'tmp/native-japanese-vn/a ',
            'tmp/native-japanese-vn/NUL', 'tmp/native-japanese-vn/COM1.txt', 'tmp/native-japanese-vn/a%TEMP%',
            'tmp/native-japanese-vn/a&b', 'tmp//native-japanese-vn/a', '\\server\share\escape',
            (Join-Path $repo 'tmp/outside-output-policy'))) {
            $rejected = $false
            try { $null = Resolve-OwnedOutput $invalid }
            catch { if (-not $_.Exception.Message.StartsWith('Output policy:')) { throw }; $rejected = $true }
            if (-not $rejected) { throw "Invalid output accepted: $invalid" }
            ++$pathChecks
        }
        # Entrypoint negatives must fail before any output creation. No recursion.
        foreach ($invalid in @('tmp/native-japanese-vn-sibling/output-policy-negative',
            (Join-Path $repo 'tmp/outside-output-policy'))) {
            $inspect = if ([IO.Path]::IsPathRooted($invalid)) { $invalid } else { Join-Path $repo $invalid }
            $before = Test-Path -LiteralPath $inspect
            $rejected = $false
            try { & $PSCommandPath -OutputDirectory $invalid }
            catch { if (-not $_.Exception.Message.StartsWith('Output policy:')) { throw }; $rejected = $true }
            if (-not $rejected -or (Test-Path -LiteralPath $inspect) -ne $before) {
                throw 'Output entrypoint did not reject before creation.'
            }
            ++$pathChecks
        }
        # Junction and target are fresh retained fixtures inside this run only.
        $target = Join-Path $attempt 'path-test-target'
        $junction = Join-Path $attempt 'path-test-junction'
        New-Item -ItemType Directory -Path $target | Out-Null
        New-Item -ItemType Junction -Path $junction -Target $target | Out-Null
        foreach ($candidate in @($junction, (Join-Path $junction 'never-created'))) {
            $rejected = $false
            try { $null = Resolve-OwnedOutput $candidate.Substring($repo.Length + 1) }
            catch {
                if ($_.Exception.Message -ne 'Output policy: reparse or non-directory ancestor.') { throw }
                $rejected = $true
            }
            if (-not $rejected) { throw 'Output policy accepted a junction.' }
            ++$pathChecks
        }
        if (Test-Path -LiteralPath (Join-Path $target 'never-created')) { throw 'Junction test unexpectedly wrote through.' }
        $metadata.outputPolicy = @{ passed = $pathChecks; failed = 0; outsideWrites = 0 }
        Write-Host "OUTPUT_POLICY passed=$pathChecks failed=0 outside_writes=0"
    }
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere)) { throw 'Installed vswhere required; no installation attempted.' }
    $vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($LASTEXITCODE -ne 0 -or -not $vs) { throw 'Installed x86 MSVC required.' }
    $vcvars = Join-Path $vs 'VC/Auxiliary/Build/vcvarsall.bat'
    if (-not (Test-Path -LiteralPath $vcvars)) { throw 'Installed vcvarsall.bat required.' }
    $source = Join-Path $attempt 'sandbox_probe.cpp'
    $common = '/nologo /W4 /WX /std:c++17 /EHsc /MT /utf-8 /Od'
    $libs = '/link /MACHINE:X86 /INCREMENTAL:NO userenv.lib advapi32.lib bcrypt.lib user32.lib ws2_32.lib'
    $metadata.buildExit = Invoke-Build 'build-fixture' "cl $common /DLOCUST_FIXTURE `"$source`" /Fo`"fixture.obj`" /Fe`"fixture.exe`" $libs"
    if ($metadata.buildExit -ne 0) { Save-Metadata; exit $metadata.buildExit }
    $pin = File-Sha256 (Join-Path $attempt 'fixture.exe')
    $header = Join-Path $attempt 'fixture_hash.h'
    $literalRoot = $attempt.Replace('\', '\\')
    [IO.File]::WriteAllText($header, "#define LOCUST_FIXTURE_SHA `"$pin`"`r`n#define LOCUST_OUTPUT_ROOT L`"$literalRoot`"`r`n", [Text.Encoding]::ASCII)
    $metadata.buildExit = Invoke-Build 'build-host' "cl $common /FI`"$header`" `"$source`" /Fo`"sandbox_host.obj`" /Fe`"sandbox_host.exe`" $libs"
    if ($metadata.buildExit -ne 0) { Save-Metadata; exit $metadata.buildExit }
    foreach ($name in @('fixture.exe', 'sandbox_host.exe')) {
        $path = Join-Path $attempt $name
        $bytes = [IO.File]::ReadAllBytes($path)
        $pe = [BitConverter]::ToInt32($bytes, 60)
        if ([BitConverter]::ToUInt16($bytes, $pe + 4) -ne 0x14c -or
            [BitConverter]::ToUInt16($bytes, $pe + 24) -ne 0x10b) { throw 'Expected i386 PE32.' }
        $metadata.binaries += @{ path = $name; sha256 = File-Sha256 $path; machine = 'i386'; bytes = $bytes.Length }
    }
    if ($RunTests) {
        # Exercise strict setup/unwinding first, before any diagnostic variant.
        $construction = Invoke-Probe 'partial-construction' '--fail-objects'
        $metadata.tests += $construction
        $red = Invoke-Probe 'red' '--baseline'
        $metadata.tests += $red
        $redOk = $red.exit -eq 23 -and -not $red.timeout -and -not $red.logOverflow -and
            $red.text.Contains('RED observed: containment assertion failed; appcontainer=0 outside_write=allowed owned_sentinel=changed allowed_write=1')
        $green = Invoke-Probe 'green' '--verify'
        $metadata.tests += $green
        $recheck = Invoke-Probe 'recheck' '--verify'
        $metadata.tests += $recheck
        # Diagnostic completion is NOT containment success. The distinct exit 20
        # is accepted only as evidence collection, never as a strict-test pass.
        $diagnostic = Invoke-Probe 'diagnostic-only' '--diagnose-objects-only'
        $metadata.tests += $diagnostic
        $diagnosticOk = $diagnostic.exit -eq 20 -and -not $diagnostic.timeout -and -not $diagnostic.logOverflow -and
            $diagnostic.text.Contains('DIAGNOSTIC_ONLY complete cases=5 children=0 containment_verdict=none')
        $metadata.passed = $diagnosticOk -and $redOk -and $green.exit -eq 0 -and $recheck.exit -eq 0 -and
            $green.text.Contains('PASS --verify profileless=1') -and $recheck.text.Contains('PASS --verify profileless=1') -and
            -not $green.timeout -and -not $recheck.timeout -and -not $green.logOverflow -and -not $recheck.logOverflow -and
            $construction.exit -eq 0 -and -not $construction.timeout -and -not $construction.logOverflow -and
            $construction.text.Contains('PASS injected_constructor_failure owned_objects_released=1')
        if ($metadata.passed) {
            foreach ($case in @(@('deadline', '--timeout'), @('kill-close', '--kill-close'))) {
                $result = Invoke-Probe $case[0] $case[1]
                $metadata.tests += $result
                if ($result.exit -ne 0 -or $result.timeout -or $result.logOverflow -or
                    -not $result.text.Contains("PASS $($case[1]) profileless=1")) { $metadata.passed = $false }
            }
        } else {
            $metadata.unexecuted = 'Deadline and kill-close behavioral tests require successful containment; overall proof remains failed.'
        }
        foreach ($binary in $metadata.binaries) {
            $binary.afterSha256 = File-Sha256 (Join-Path $attempt $binary.path)
            if ($binary.afterSha256 -ne $binary.sha256) { $metadata.passed = $false }
        }
    }
    Save-Metadata
    Write-Host "Evidence: $attempt"
    if ($RunTests -and -not $metadata.passed) { exit 1 }
    exit 0
} catch {
    $metadata.error = $_.Exception.Message
    Save-Metadata
    Write-Host "FAIL: $($_.Exception.Message)"
    Write-Host "Evidence: $attempt"
    exit 1
}
