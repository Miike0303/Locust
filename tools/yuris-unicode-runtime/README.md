# Isolated x86 Unicode runtime proof — not a game runtime

This original C++17 DLL and owned console host demonstrate exact Spanish conversion,
measurement, and offscreen rendering through **three actual host imports**. They do
not enable Unicode in Locust's production backend, install a proxy, or establish
YU-RIS compatibility. Keep the existing Shift-JIS refusal in place.

## Run the proof

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
