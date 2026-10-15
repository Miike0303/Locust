# Isolated x86 Unicode runtime proof — not a game runtime

This original C++17 DLL and owned console host demonstrate exact Spanish conversion,
measurement, and offscreen rendering through **three actual host imports**. They do
not enable Unicode in Locust's production backend, install a proxy, or establish
YU-RIS compatibility. Keep the existing Shift-JIS refusal in place.

## UNI-S containment experiment — currently blocked, not an execution boundary

The separate `sandbox_probe.cpp` / `build-sandbox.ps1` experiment **does not yet
prove preventive containment**. The current unnamed, create-only windowstation
request returns **Win32 error 183 (`ERROR_ALREADY_EXISTS`)** and no handle.
It refuses the existing generated-name object; it does not open, reuse or change
it. Earlier caller-named requests returned error 5 (`ERROR_ACCESS_DENIED`).
Neither result establishes profileless AppContainer support: child creation was
not reached. No game execution, production activation or sandbox CI step is
permitted by these results.

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools/yuris-unicode-runtime/build-sandbox.ps1 -OutputDirectory tmp/native-japanese-vn/unicode-sandbox-20261014 -RunTests
```

The command builds two original i386/PE32 binaries from the same source with
`/W4 /WX /std:c++17 /MT`: a parent and a separately compiled adversarial fixture.
Only installed Microsoft tools/platform APIs are used. The parent embeds the
fixture's freshly compiled SHA256 and its own exact output directory. There is
no arbitrary-program, arbitrary-payload, profile, or game-launch option.

The sandbox resolver now follows the UNI-1 logical output policy: any safe
repository-relative **descendant of `tmp/native-japanese-vn/`**, not that root
itself or a similarly prefixed sibling. Rooted/device/UNC paths, traversal,
wildcards, ADS, reserved Windows names, trailing dots/spaces, shell expansion,
and reparse ancestors are rejected before output creation. Comparisons use
Windows case-insensitive semantics; the compiled host still binds its exact
per-run directory. The 27 checks include four no-write positive resolver cases
(including `unicode-sandbox-verify-20261014`), negative entrypoint calls, and an
owned retained junction. This worker executes builds only under its authorized
`unicode-sandbox-20261014/` surface; accepting another safe path is not evidence
of a build there. This is owned-checkout confinement, not protection from a
concurrent hostile filesystem editor.

### Windowstation naming and strict ownership

Microsoft's [CreateWindowStationW contract](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-createwindowstationw)
restricts caller-specified names to Administrators; a null name requests an
OS-generated logon-derived name. The verifier supplied this precondition after
all previous named tests had failed. Both strict and diagnostic paths now pass
`nullptr`, retain `CWF_CREATE_ONLY`, request `0x017F`, and keep their supplied
security descriptors unchanged. The generated name is not a fresh random name:
a collision must fail closed, as observed here. There is no elevation or
retry without the create-only flag.

If creation ever succeeds, the held new handle supplies the actual `UOI_NAME`.
It must differ from the original station, and `UOI_FLAGS` must be non-visible.
Security-view handles, startup desktop paths and the child's identity check use
that actual name, not a nonce-derived fiction. Before child launch, strict
readback must verify the protected user/AppContainer-only DACL, owner and exact
low no-write-up label on both private objects. A missing/unreadable/mismatched
readback fails; diagnostic readbacks remain informational. None of these
successful-creation paths was reached in the collision runs.

### Intended checks versus observed evidence

| Boundary | Implementation / acceptance | Local observation |
|---|---|---|
| Output and payload | Safe descendants of the shared tmp namespace; per-run host binding; reject reparse ancestors; fixed-hash write-locked fixture | 27 resolver/entrypoint/junction checks, source snapshots, PE checks, copies and hash pin succeeded |
| Profileless identity | Random name; only `DeriveAppContainerSidFromAppContainerName`; zero `SECURITY_CAPABILITIES`; actual suspended token SID/user/low IL/capabilities/privileges and child policy must match | SID derivation succeeded; actual child token not reached |
| New file permissions | Explicit protected DACLs only at creation of owned case/allowed files; AppContainer grant and low label only on the allowed directory and its fixture | Creation succeeded; contained positive-write probe not reached |
| Private user objects | Unnamed create-only station; actual name/non-input/strict security readback gates; owned desktop; no input switching or existing ACL edits | Creation now fails with error 183; no handle or desktop |
| Process/job | No inherited handles; clean minimal environment; suspended creation; assign job and verify token before resume; active process limit 1, no breakaway, kill-on-close, 128 MiB, 2 s CPU, 5 s normal wall deadline | Not reached |
| Adversarial probes | Own outside sentinel write must return access denied; allowed file bytes must match; `RegOpenKeyExW(HKCU, Software, KEY_SET_VALUE)` must deny; no registry value read/create/set/delete | Only initial uncontained sentinel/allowed-file RED observed |
| Network | Bind only own IPv4 loopback listener; host positive control must connect; child must receive `WSAEACCES`, with zero child connections accepted | Host control succeeded; contained probe not reached |
| Children/handles | Actual child-process mitigation plus attempted own `--grandchild` launch denied; inheritable parent event absent in fixture | Not reached; job and child policy are overlapping controls, not separately attributed denial mechanisms |
| Lifetime | Injected setup-failure unwinding, fixture-ready timeout and kill-on-close cases; verify dead process/empty job and private-object disappearance after handle release | Private-object setup blocked these cases; no cleanup success inferred from absence of creation |

The script runs serially in the foreground: **strict setup-unwinding first**
(`--fail-objects`, no child), the expected-failing uncontained control, a strict
proof attempt, a focused repeat, then the diagnostic-only comparison. The
diagnostic branch returns before fixture/listener/job setup or any desktop
selection/child launch; no descriptor variant can become a fallback. Logs
record the naming mode, numeric access mask and immediate `GetLastError`.
Successful strict setup would require the security readback described above;
no privilege is adjusted to obtain it.
Timeout/kill-on-close behavioral tests require successful containment first;
unexecuted cases remain reported as unexecuted and **cannot make the proof green**.
The outer host deadline is 30 seconds and persisted per-test logs are capped at
64 KiB. Child reports have a fixed binary size; child output handles are not
inherited. Every failed launch/check returns nonzero. Compiling without
`-RunTests` is only a build, never a containment result.

All generated files are retained. There are no profile creation/deletion APIs,
registry mutations, filesystem deletion calls, global settings, privilege
adjustments, downloads, or ACL changes to ancestors/originals. Releasing owned
kernel handles and process attribute memory is not filesystem deletion. The only
registry operation requests an existing key handle and closes it without reading
values or modifying anything. The sentinel is prepared solely for this fixture;
its initial negative-control mutation is retained as evidence, not applied to
user data.

The final baseline also requires private non-input objects and a bounded job;
it omits AppContainer and child policy. Consequently, it now fails at the same
private-windowstation prerequisite on this machine. It does **not** fall back to
the default desktop to recover the earlier RED.

### Diagnostic-only mode — not a sandbox or a passing proof

The fixed host mode `--diagnose-objects-only` compares five unnamed,
system-generated-name windowstation attempts with the same `0x017F` request and
`CWF_CREATE_ONLY`. It never creates a desktop or child, selects a station, changes
existing objects, or creates a profile. Each case derives a fresh AppContainer
SID without registering it. The fixed cases are:

| Case | Descriptor | Observed creation |
|---|---|---|
| `existing_ga_low` | Existing current-user/AppContainer GA DACL plus low label | Error 183 |
| `default_null` | Null security-attributes pointer: OS default descriptor | Error 183 |
| `explicit_ga_no_sacl` | Same explicit GA DACL, no SACL | Error 183 |
| `explicit_ga_low_repeat` | Same explicit GA DACL and low label, independent SID nonce | Error 183 |
| `numeric_full_low` | Explicit `0x000F037F` allow ACEs instead of GA, same low label | Error 183 |

If an owned station is created, diagnostics check that it is non-input, query
its owner/group/DACL/mandatory label where available, close only acquired
references, and check disappearance. Here no station was created, so readback
and successful-object lifetime verification were unavailable. The default/null
case is **diagnostic-only**, not permission to use a default descriptor in the
strict proof.

Read-only host diagnostics observed a medium-integrity, non-elevated primary
token, no AppContainer, no restricted SIDs, no job membership, and no thread
impersonation token (`ERROR_NO_TOKEN`, 1008). The six queried mitigation flag
sets were zero. `TokenHasRestrictions` separately returned value 1 in one byte;
it is not synonymous with `IsTokenRestricted`, which returned false. These
observations do not establish why creation was denied or rule out unqueried
host/session access controls. Raw SIDs remain in local evidence, not this guide.

Completing this bounded comparison returns **20**, never strict-proof success
(0), and emits `containment_verdict=none`. The runner requires both diagnostic
completion and every independent strict check; diagnostic completion cannot
turn the overall failing command green. Historical named diagnostic failures
are in `tmp/native-japanese-vn/unicode-sandbox-20261014/REPORT-diagnostic-20261014.md`.
The current unnamed attempts, resolver RED/GREEN and regression evidence are in
`tmp/native-japanese-vn/unicode-sandbox-20261014/REPORT-unnamed-20261014.md`.

### Retained intermediate failures

Under `tmp/native-japanese-vn/unicode-sandbox-20261014/`:

- `run-20261014-232214-7656240`: actual uncontained fixture modified its prepared
  sentinel and wrote the allowed file; host containment assertion exited **23**.
  The initial RED-only implementation inherited its existing desktop and did not
  yet implement private objects. Both strict stub calls returned **1**, diagnostic
  error **50**. The compiler succeeded despite a `vcvarsall` internal `vswhere.exe`
  PATH warning; the runner now supplies the installed tool directory.
- `run-20261014-232501-9825842`: fixture built, but host `/WX` compilation failed
  with **C2065**, because `DESKTOP_ALL_ACCESS` is not an SDK constant. Replaced it
  with explicit required desktop access bits; no security fallback was added.
- `run-20261014-232530-6010644`: both binaries built; all four host invocations
  stopped at private windowstation creation with **error 5**. Source snapshots,
  metadata, file fixtures and logs remain available. No child was launched by
  these invocations.

Each attempt keeps `sandbox_probe.cpp`, `build-sandbox.ps1`, build command/logs,
`fixture_hash.h`, binaries, source/binary hashes, and individual test logs plus
`metadata.json`. Original user/logon SIDs and derived SID are recorded in local
metadata/logs; this guide does not publish them or personal filesystem paths.
The initial final attempt and UNI-1 regression result are recorded in
`tmp/native-japanese-vn/unicode-sandbox-20261014/REPORT.md`.

The controlled mask follow-up retained two fresh attempts:
`run-20261014-233252-7298425` logged `0x037F`, and
`run-20261014-233308-1345488` logged `0x017F`. All four invocations in each
attempt returned error 5 at creation. No object handle was returned, so actual
object-security readback and every later child stage were unavailable, not
passed. The supplied descriptor, create-only/non-input requirements and all
containment checks stayed unchanged. Follow-up evidence:
`tmp/native-japanese-vn/unicode-sandbox-20261014/REPORT-mask-20261014.md`.

Even a future passing result would establish only this owned Win32 fixture's
observed OS isolation. Standard AppContainer is not malware/exploit-proof, a
universal COM/broker isolation claim, portable Unicode support, or evidence of
YU-RIS compatibility. A derived SID alone establishes none of those properties.
The authoritative task remains
[`odd/tasks/yuris-unicode-runtime.md`](../../odd/tasks/yuris-unicode-runtime.md).

## Run the UNI-1 Unicode proof

From the repository root on Windows, with existing MSVC x86 tools and Windows SDK:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools/yuris-unicode-runtime/build-proof.ps1 -OutputDirectory tmp/native-japanese-vn/unicode-proof-20261014 -RunTests
```

The script discovers installed Visual Studio through `vswhere`, calls
`vcvarsall.bat x86`, and compiles with `/W4 /WX /std:c++17 /MT`. It checks both
outputs for i386/PE32. There are no downloads, installers, third-party detours,
vendored implementation code, Rust builds, or frontend builds in this command.
The existing Windows CI job runs this command; the Linux job is unchanged.

`-OutputDirectory` accepts repository-relative **descendants of
`tmp/native-japanese-vn/`**, so an independent verifier can select a fresh path
such as `tmp/native-japanese-vn/unicode-proof-verify-20261014`. The containing root
itself and similarly prefixed sibling directories are not output destinations.
The optional `.test-tmp` alternative is not enabled.

Validation runs before any directory creation. It rejects absolute/UNC/device
paths, traversal or empty components, wildcards, alternate data streams, reserved
Windows device names, trailing dots/spaces, and shell-expansion characters.
Components use ASCII letters/digits, underscores, hyphens, internal dots/spaces;
leading dots/spaces are disallowed. Every existing ancestor must be a real
directory, not a reparse point (including a dangling link). This is confinement
for an owned checkout, not a sandbox against concurrent hostile filesystem edits.

Every run creates a fresh `run-<UTC timestamp>` beneath the selected output path.
Compiler working files and `TMP`/`TEMP` stay there. `-RunTests` also runs bounded
PowerShell path checks, including negative script-entrypoint invocations that
reject outside/sibling destinations without creating them, and owned-junction
rejection tests. The junction and its empty target stay inside that run directory.
The script never deletes previous evidence, mutates dependencies, or configures
global DLL search paths. It uses absolute DLL loading with
`LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32`.

| Artifact | Meaning |
|---|---|
| `build.log`, `build.cmd` | Actual toolchain invocation and diagnostics |
| `tests.log` | Named assertions, font indices, metrics, pixel counts, restoration |
| `metadata.json` | Source line counts/SHA256, workflow SHA256, binary SHA256 before/after tests, PE architecture, separate path-policy/native-test counts and scope assertions |
| `proof.exe`, `locust_unicode_proof.dll` | Synthetic x86 host and explicitly initialized DLL |
| `valid.map`, `invalid-*.map`, `maximum.map` | Owned fixtures; each negative case is retained separately; only the immutability test rewrites `valid.map` |

A compiler/build/test failure, a missing summary, any skipped assertion, failed
pointer restoration, or absent required Spanish font coverage returns nonzero.
`-RunTests` is required for behavioral verification; without it only compilation
and PE checks run. No-GUI/no-original-write fields describe this bounded host's
behavior, not a filesystem audit of a real game.

## Review the boundary first

- `runtime.cpp`: map validation, strict decoder, bounded PE/import walker, and
  main-module-only IAT transaction.
- `proof.cpp`: independent expected Unicode/PE fixtures, import snapshots,
  guard pages, native W-API comparisons and nonblank DIB pixel comparisons.
- `runtime_map.h`: small shared ABI/constants and CNG hashing; the host assembles
  the authenticated map bytes independently of the reader.
- `build-proof.ps1`: output confinement, tool discovery, evidence and exit status.

The DLL has **no custom `DllMain`**. Loading it alone changes no imports. The host
explicitly resolves `UniInitialize`, `UniUninstall`, and the read-only
`UniProbeImage` test entrypoint. Initialization never takes an arbitrary target
module: it obtains only `GetModuleHandleW(nullptr)`. It requires these named,
module-qualified imports, rejecting a missing or duplicate match:

| Import module | Only intercepted API | Behavior |
|---|---|---|
| `kernel32.dll` | `MultiByteToWideChar` | Page **932 only** uses the exact decoder; all other pages, including symbolic ACP/thread ACP, call the original API unchanged |
| `gdi32.dll` | `TextOutA` | Strict decode, then `TextOutW` with the decoded explicit length |
| `gdi32.dll` | `GetTextExtentPoint32A` | Strict decode, then `GetTextExtentPoint32W` |

No `GetProcAddress` hook, delay-import hook, dynamic-module interception, font hook,
engine scan, code/entrypoint patch, remote-memory write, proxy autoload, process
injection, game launch, on-screen window, custom font installation, or GUI occurs.
The proof does not cover ExtTextOut, DrawText, Direct3D, font callbacks or shaping
outside these W APIs. DLL imports and executable memory sections are checked for
unchanged contents; both binary files are hashed before and after the tests.

### Lifecycle and rollback

This is an **owned-host, quiescent lifecycle contract**, not an arbitrary-process
hook library. The trusted host must keep callers quiescent during initialize and
uninstall, and uninstall from the initializing thread before `FreeLibrary`.
Wrappers use a shared lock; lifecycle operations hold it exclusively. Those locks
protect decoder state, but do not turn three IAT words into one CPU-atomic write
or make concurrent unload safe.

The walker checks readable committed ranges, DOS/NT/PE32/i386 headers, declared
image/import/IAT bounds, thunk termination, bounded names, aligned slots, and
required module/API identities. Actual installation also requires the original
OS API pointers and non-executable `MEM_IMAGE` data pages owned by the main module.
Only those IAT pages briefly become writable. Each slot exchange is atomic.
A transaction either completes or rolls back all changed slots and page
protections; the proof injects failure at each of the three installation and
restoration steps. Invariant loss during rollback/protection recovery fail-fasts
the owned process rather than returning a falsely safe unload state.

`UniInitialize(..., fault)` and `UniUninstall(fault)` accept `0` normally;
`1..3` are explicit **synthetic test-only fault positions**. `UniProbeImage` checks
owned malformed buffers without modifying them; it is not an alternate patch
entrypoint. Null/malformed ABI pointers supplied by an untrusted caller are not a
supported public IPC boundary. The host owns buffer lifetimes and valid output
storage, as with the native Win32 APIs.

## Original map format

This is **not** the upstream headerless `sjis_ext.bin` format. All integers are
little-endian; there is no implicit packing or raw C++ structure serialization.

| Offset | Bytes | Value |
|---|---:|---|
| 0 | 8 | `LOCUNI1` followed by NUL |
| 8 | 4 | Version = 1 |
| 12 | 4 | Entry count, 1..3422 |
| 16 | 16 | Opaque generation identifier |
| 32 | 32 | SHA256 of bytes `[0,32)` concatenated with `[64,end)` |
| 64 | 4 × count | Lead byte, trail byte, exact UTF-16LE BMP scalar |

The trusted host supplies the expected generation **and** expected digest.
The stored digest must equal the recomputed digest and the trusted expectation;
the generation must match too. This is content/generation binding, **not a
signature, deployment manifest, or script authentication protocol**. A future
bundle must supply trust independently, not accept a digest from the same
untrusted map. The proof uses predetermined host-owned fixture content.

The loader bounds file size before allocating, opens read-only without sharing
writers, rejects a final-component reparse file, and requires an exact file
length. It refuses unknown versions, trailing/truncated bytes, duplicate pairs
or codepoints, invalid/reserved pairs, NUL/control scalars, surrogates and Unicode
noncharacters. It publishes a fixed in-memory lookup only after validation;
there is no mutable allocation, lazy reload, lossy replacement or accent folding.
Only local absolute drive paths are accepted; this is not a sandbox against a
malicious trusted host or reparse ancestors.

### Experimental byte allocation is not engine-safe allocation

Prototype leads are `81..9F` and `E0..FB`; trails are `01..3F` excluding TAB, LF,
CR, space and comma. That gives **3,422** candidate pairs. NUL, `FC` leads and all
other pairs are ineligible map keys. These are invalid CP932 trail combinations
studied in upstream research, **not proof that any game/parser/control stream
can safely carry them**. The test exercises all 3,422 mappings but asserts no
engine safety. A production encoder, collision analysis and native script-path
proof remain out of scope.

Ordinary ASCII and Japanese use the native CP932 decoder with
`MB_ERR_INVALID_CHARS`, including ordinary vendor/user-defined CP932 pairs.
Unknown tunnel pairs, dangling leads and illegal trails fail rather than become
replacement characters. The proof compares every single byte and all legal
CP932 double-byte shapes against strict native conversion.

CP932 conversion accepts only flags `0` or `MB_ERR_INVALID_CHARS`. Explicit input
is limited to 65,536 bytes, retaining embedded NULs. Conversion count `-1` scans
only readable bytes up to that bound and includes the first NUL in the result;
zero/other negative conversion counts fail. Size queries use output capacity
zero; insufficient capacity fails without partial writes. GDI uses only explicit
nonnegative counts, with zero allowed; it does not acquire `-1` string semantics.
Unrelated code pages retain the original API's flags, bounds and error semantics.

## What the evidence does and does not prove

The first behavior RED used a successfully built pass-through DLL: the actual
host CP932 import returned **0**, not the required exact `ñé¡¿`.
`run-20261014-223600-4076165` retains that result and source/binary hashes.
Earlier toolchain/script failures are not behavioral RED. The first implemented
GREEN, `run-20261014-223926-4638430`, passed 97 assertions before further coverage
was added. Later run directories retain the final counts and source hashes.

Independent-verifier follow-up reported ten executions of the existing final
binary at 108/0/0, but its fresh build was blocked by the original output-path
policy's worker-specific root. The broadened confined policy above fixes that
source-build obstacle; existing-binary runs are not fresh-source evidence.
The verifier also reproduced 35 earlier diagnostic assertion failures, all with
`saved=0`, `save_error=32`, and unchanged IAT/protections: the observed failure
class is **fixture-write sharing violation**, not acceptance of malformed maps.
The identity of the conflicting handle remains unknown. Keep the unique negative
fixture filenames; this follow-up does not change runtime or native-test source.

The offscreen tests select installed **Segoe UI**, verify the selected face and
nonmissing glyph indices for `ñ`, `é`, `¡`, `¿`, then compare actual imported
A-API measurement/pixels to independent W-API reference calls. Whole-string and
individual-glyph images must be nonblank and pixel-identical. Missing required
Spanish coverage fails; it is never counted as a skip or simulated success.
The test also covers mixed ASCII/repeated Spanish and explicit embedded NULs.
Japanese/CJK **exact conversion and W-metric parity** are separate observations:
**CJK font glyph coverage is not claimed**.

No production encoding gate, database, injector, patch bundle, backend or UI is
changed. This synthetic result cannot complete UNI-2 transaction/deployment work
or UNI-3 engine/runtime/font/linguistic acceptance. See the authoritative task:
[`odd/tasks/yuris-unicode-runtime.md`](../../odd/tasks/yuris-unicode-runtime.md).

## Research provenance — no implementation copied

Research reference: [VNTranslationTools at
`36fbe909d02f2e9bb187443e03e2df595f56bfec`](https://github.com/arcusmaximus/VNTranslationTools/tree/36fbe909d02f2e9bb187443e03e2df595f56bfec).
Its root license at that revision is MIT, Copyright (c) 2021 arcusmaximus;
license SHA256:
`01c923f5099a4e3709d17ee9489e371747aaadfe23d4fdc839ff4cdba2b49caa`.
The prior read-only research report and per-file manifest are under
`tmp/native-japanese-vn/unicode-runtime-research-20261014/` (ignored local evidence).
The map schema, validation, IAT walker, wrappers and test host here are original;
no upstream implementation or Detours source/library is incorporated. The root
license observation is not third-party dependency/license clearance. Only the
installed Microsoft compiler/SDK, static C++ runtime and standard Win32/CNG APIs
are used; their ordinary platform license obligations still apply.

## Key learnings

1. Exact synthetic Unicode conversion does not establish engine-safe byte allocation.
2. An intercepted import, W-API metric comparison and nonblank pixel comparison test different parts of the rendering path.
3. The host must own trust inputs and quiescent hook lifecycle; SHA256 is not a deployment signature.
4. Spanish glyph coverage does not imply CJK glyph coverage or real-game compatibility.
5. Independent fresh builds need a confined output namespace, not a worker-specific directory.
6. Reported error 32 identifies a fixture-write sharing violation, not the conflicting handle's identity.
7. The UNI-S local failure occurred at private windowstation creation, before any profileless child-token observation.
8. SID derivation and a successful compiler build do not demonstrate preventive containment.
9. A nonzero prerequisite failure must remain distinct from a passed denial probe or verified cleanup.
10. Removing only the READSCREEN request did not resolve the observed private-windowstation creation denial.
11. All five diagnostic descriptor cases failed creation; no default descriptor was adopted for sandboxing.
12. The observed one-byte TokenHasRestrictions result required diagnostic-specific size reporting, not a change to strict child-token checks.
13. Unnamed create-only requests changed the observed failure from access denied to an existing-object collision, not to a containment pass.
14. A resolver can validate independent safe destinations without writing outside the worker's authorized test surface.
