param(
    [string]$OutputDirectory = 'tmp/native-japanese-vn/unicode-proof-20261014',
    [switch]$RunTests
)
$ErrorActionPreference = 'Stop'
function File-Sha256([string]$Path) {
    $hash = [Security.Cryptography.SHA256]::Create()
    $stream = [IO.File]::OpenRead($Path)
    try { return ([BitConverter]::ToString($hash.ComputeHash($stream))).Replace('-', '').ToLowerInvariant() }
    finally { $stream.Dispose(); $hash.Dispose() }
}
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
function Resolve-ProofOutput([string]$RelativePath) {
    # Validate lexically before normalization: never resolve traversal, wildcards,
    # rooted/device paths, shell expansion, ADS or Windows filename aliases.
    if ([string]::IsNullOrWhiteSpace($RelativePath) -or [IO.Path]::IsPathRooted($RelativePath)) {
        throw 'Output policy: a repository-relative descendant is required.'
    }
    $segments = $RelativePath -split '[\\/]'
    foreach ($segment in $segments) {
        if ($segment -notmatch '^[A-Za-z0-9_-][A-Za-z0-9_. -]*$' -or
            $segment -match '[. ]$' -or $segment -match '^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])([. ]|$)') {
            throw 'Output policy: unsafe path component.'
        }
    }
    $full = [IO.Path]::GetFullPath((Join-Path $repo ($segments -join '\')))
    $allowed = [IO.Path]::GetFullPath((Join-Path $repo 'tmp/native-japanese-vn')) + '\'
    if (-not $full.StartsWith($allowed, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Output policy: expected a descendant of repo/tmp/native-japanese-vn.'
    }
    # GetAttributes sees reparse entries themselves, including dangling links.
    # Missing ancestors are allowed; access errors and non-directory entries are not.
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
# This is the first output operation: every rejected argument exits before creation.
$out = Resolve-ProofOutput $OutputDirectory
New-Item -ItemType Directory -Force -Path $out | Out-Null
$attempt = Join-Path $out ('run-' + [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss-fffffff'))
New-Item -ItemType Directory -Path $attempt | Out-Null
$pathChecks = 0
if ($RunTests) {
    # Bounded negative invocations of the same no-write resolver used above.
    $invalidPaths = @('', '../tmp/native-japanese-vn/escape', 'tmp/native-japanese-vn/../escape',
        'tmp/native-japanese-vn', 'tmp/native-japanese-vn-sibling/escape', 'tmp/elsewhere/escape',
        'tmp/native-japanese-vn/a*', 'tmp/native-japanese-vn/a?', 'tmp/native-japanese-vn/[a]',
        'tmp/native-japanese-vn/a:stream', 'tmp/native-japanese-vn/a.', 'tmp/native-japanese-vn/a ',
        'tmp/native-japanese-vn/NUL', 'tmp/native-japanese-vn/COM1.txt', 'tmp/native-japanese-vn/a%TEMP%',
        'tmp/native-japanese-vn/a&b', 'tmp//native-japanese-vn/a', '\\server\share\escape',
        (Join-Path $repo 'tmp/outside-output-policy'))
    foreach ($invalid in $invalidPaths) {
        $rejected = $false
        try { $null = Resolve-ProofOutput $invalid }
        catch { if ($_.Exception.Message.StartsWith('Output policy:')) { $rejected = $true } else { throw } }
        if (-not $rejected) { throw "Output policy negative test accepted: $invalid" }
        ++$pathChecks
    }
    # Exercise the actual script entrypoint too, without -RunTests recursion.
    # Both arguments fail before its first New-Item; no outside path is created.
    foreach ($invalid in @('tmp/native-japanese-vn-sibling/output-policy-negative',
        (Join-Path $repo 'tmp/outside-output-policy'))) {
        $inspect = if ([IO.Path]::IsPathRooted($invalid)) { $invalid } else { Join-Path $repo $invalid }
        $before = Test-Path -LiteralPath $inspect
        $rejected = $false
        try { & $PSCommandPath -OutputDirectory $invalid }
        catch { if ($_.Exception.Message.StartsWith('Output policy:')) { $rejected = $true } else { throw } }
        if (-not $rejected -or (Test-Path -LiteralPath $inspect) -ne $before) {
            throw 'Output policy entrypoint failed to reject before creation.'
        }
        ++$pathChecks
    }
    foreach ($valid in @('tmp/native-japanese-vn/unicode-proof-verify-20261014',
        'tmp\native-japanese-vn\fresh-proof\nested-output')) {
        $null = Resolve-ProofOutput $valid
        ++$pathChecks
    }
    # A real, owned junction must be rejected both as leaf and as ancestor.
    $target = Join-Path $attempt 'path-test-target'
    $junction = Join-Path $attempt 'path-test-junction'
    New-Item -ItemType Directory -Path $target | Out-Null
    New-Item -ItemType Junction -Path $junction -Target $target | Out-Null
    foreach ($candidate in @($junction, (Join-Path $junction 'never-created'))) {
        $rejected = $false
        try { $null = Resolve-ProofOutput $candidate.Substring($repo.Length + 1) }
        catch { if ($_.Exception.Message -eq 'Output policy: reparse or non-directory ancestor.') { $rejected = $true } else { throw } }
        if (-not $rejected) { throw 'Output policy reparse test accepted.' }
        ++$pathChecks
    }
    if (Test-Path -LiteralPath (Join-Path $target 'never-created')) { throw 'Reparse test unexpectedly created output.' }
    Write-Host "OUTPUT_POLICY passed=$pathChecks failed=0 outside_writes=0"
}
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
if (-not (Test-Path -LiteralPath $vswhere)) { throw 'Installed vswhere is required; no installation attempted.' }
$vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if ($LASTEXITCODE -ne 0 -or -not $vs) { throw 'Installed MSVC x86 tools required.' }
$vcvars = Join-Path $vs 'VC/Auxiliary/Build/vcvarsall.bat'
if (-not (Test-Path -LiteralPath $vcvars)) { throw 'vcvarsall.bat is missing.' }
$source = $PSScriptRoot
$cmd = @"
@echo off
set "TEMP=$attempt"
set "TMP=$attempt"
set "PATH=$(Split-Path -Parent $vswhere);%PATH%"
call "$vcvars" x86
if errorlevel 1 exit /b 1
cd /d "$attempt"
cl /nologo /W4 /WX /std:c++17 /EHsc /MT /utf-8 /Od /LD "$source\runtime.cpp" /Fo"runtime.obj" /Fe"locust_unicode_proof.dll" /link /DEF:"$source\runtime.def" /MACHINE:X86 /INCREMENTAL:NO bcrypt.lib gdi32.lib
if errorlevel 1 exit /b 1
cl /nologo /W4 /WX /std:c++17 /EHsc /MT /utf-8 /Od "$source\proof.cpp" /Fo"proof.obj" /Fe"proof.exe" /link /MACHINE:X86 /INCREMENTAL:NO bcrypt.lib gdi32.lib
exit /b %errorlevel%
"@
$cmdPath = Join-Path $attempt 'build.cmd'
[IO.File]::WriteAllText($cmdPath, $cmd, [Text.Encoding]::ASCII)
$ErrorActionPreference = 'Continue'
& cmd.exe /d /c $cmdPath 2>&1 | Tee-Object -FilePath (Join-Path $attempt 'build.log') | Write-Host
$buildExit = $LASTEXITCODE
$ErrorActionPreference = 'Stop'
$sourceRecords = @('runtime_map.h', 'runtime.cpp', 'runtime.def', 'proof.cpp', 'build-proof.ps1', 'README.md') | ForEach-Object {
    $path = Join-Path $source $_
    if (Test-Path -LiteralPath $path) {
        [ordered]@{ path = "tools/yuris-unicode-runtime/$_"; sha256 = (File-Sha256 $path);
            lines = @(Get-Content -LiteralPath $path).Count }
    }
}
$metadata = [ordered]@{ schema = 1; buildExit = $buildExit; testExit = $null; sources = @($sourceRecords);
    outputPolicy = [ordered]@{ passed = $pathChecks; failed = 0; outsideWrites = 0 };
    binaries = @(); tests = $null; apiRestore = $false; noGUI = $null; noOriginalWrites = $null;
    scope = 'Synthetic owned process only; no original game files opened';
    workflowSha256 = (File-Sha256 (Join-Path $repo '.github/workflows/ci.yml')) }
if ($buildExit -eq 0) {
    foreach ($name in @('proof.exe', 'locust_unicode_proof.dll')) {
        $path = Join-Path $attempt $name
        $bytes = [IO.File]::ReadAllBytes($path)
        $pe = [BitConverter]::ToInt32($bytes, 60)
        $machine = [BitConverter]::ToUInt16($bytes, $pe + 4)
        $magic = [BitConverter]::ToUInt16($bytes, $pe + 24)
        if ($machine -ne 0x14c -or $magic -ne 0x10b) { throw 'Proof outputs must be actual i386 PE32 binaries.' }
        $metadata.binaries += [ordered]@{ path = $name; sha256 = (File-Sha256 $path);
            machine = 'i386'; optionalHeader = 'PE32'; bytes = $bytes.Length }
    }
    if ($RunTests) {
        $ErrorActionPreference = 'Continue'
        $testLines = @(& (Join-Path $attempt 'proof.exe') $attempt 2>&1 | Tee-Object -FilePath (Join-Path $attempt 'tests.log'))
        $metadata.testExit = $LASTEXITCODE
        $ErrorActionPreference = 'Stop'
        $testLines | ForEach-Object { Write-Host $_ }
        $summary = $testLines | Where-Object { "$_" -match '^SUMMARY ' } | Select-Object -Last 1
        if ($summary -match '^SUMMARY passed=(\d+) failed=(\d+) skipped=(\d+)') {
            $metadata.tests = [ordered]@{ passed = [int]$Matches[1]; failed = [int]$Matches[2]; skipped = [int]$Matches[3] }
            if ($metadata.tests.passed -eq 0 -or $metadata.tests.failed -ne 0 -or $metadata.tests.skipped -ne 0) { $metadata.testExit = 1 }
        } else { $metadata.testExit = 1 }
        $metadata.apiRestore = @($testLines | Where-Object { "$_" -eq 'API_RESTORE all_original_pointers=true dll_imports_unchanged=true' }).Count -eq 1
        $metadata.noGUI = "$summary".Contains('no_gui=true')
        $metadata.noOriginalWrites = "$summary".Contains('original_writes=0')
        if (-not $metadata.apiRestore -or -not $metadata.noGUI -or -not $metadata.noOriginalWrites) { $metadata.testExit = 1 }
        foreach ($binary in $metadata.binaries) {
            $binary.afterTestSha256 = File-Sha256 (Join-Path $attempt $binary.path)
            $binary.unchanged = $binary.sha256 -eq $binary.afterTestSha256
            if (-not $binary.unchanged) { $metadata.testExit = 1 }
        }
    }
}
$metadata | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $attempt 'metadata.json') -Encoding UTF8
Write-Host "Evidence: $attempt"
if ($buildExit -ne 0) { exit $buildExit }
if ($RunTests -and $metadata.testExit -ne 0) { exit $metadata.testExit }
exit 0
