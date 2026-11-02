# Desktop auto-update (GitHub Releases)

The desktop app does **not** talk to `releases.projectlocust.com`. That host is not
referenced anywhere in the code. The live updater endpoint is a GitHub Release asset.

## Endpoint (what actually runs)

Configured in `apps/desktop/src-tauri/tauri.conf.json` → `plugins.updater.endpoints`:

```
https://github.com/Miike0303/Locust/releases/latest/download/latest.json
```

`/releases/latest/download/…` follows the published release GitHub designates latest.
The app checks this URL on launch (`UpdateChecker`). Tauri compares `latest.json`'s
`version` to the running app and, if newer, offers Download & install.

## How `latest.json` is produced

On each `v*` tag (and `workflow_dispatch`), `.github/workflows/release.yml`:

1. **`validate` job** — requires a stable `refs/tags/vX.Y.Z` ref, equality with
   the Cargo workspace, desktop package and Tauri versions, and exactly one
   nonempty `## [X.Y.Z]` CHANGELOG section. Manual dispatch from a branch is
   refused. It creates a draft with those notes and the exact candidate SHA, or
   reuses a matching draft. Public releases, prereleases and a different target
   SHA are refused; same-ref runs are serialized.
2. **Desktop matrix** — builds Windows (x64 + ARM64), macOS (arm64 + x64) and Linux
   x64 bundles with `tauri-apps/tauri-action`, uploading to the existing **draft**
   by release ID with `releaseDraft: true`. `bundle.createUpdaterArtifacts: true`
   requests signed updater artifacts using the existing signing secrets. Action
   `args: ... -- --locked` passes `--locked` through Tauri to Cargo.
3. **CLI matrix** — builds and uploads all four `locust` binaries to the draft
   (not part of the updater manifest), with locked Cargo resolution.
   See [USAGE.md](../USAGE.md#7-applying-a-patch).
4. **`publish` job** — runs only after all five desktop and all four CLI builds
   succeed. It rechecks the tag/source SHA and draft, then runs
   `bash scripts/generate-updater-manifest.sh <tag>` in strict mode. The shell
   entrypoint delegates to Python stdlib, using `gh` for release metadata and
   authenticated draft signature downloads (`gh release download`). Public
   release recovery uses `curl --fail --silent --show-error --location`.
5. After uploading `latest.json`, the finalizer downloads it and verifies all
   five desktop pairs, four CLI binaries and the manifest are present/nonempty.
   It checks all platform keys, exact URLs/signatures, version, CHANGELOG notes
   and draft/tag/candidate SHA. Only then does it publish the draft as latest.
   A failed, skipped or cancelled build or a failed verification leaves the
   draft unpublished and does not replace the current public latest.

The release gate requires Python 3.11+ (`tomllib`); workflow jobs provision 3.12.
The required CLI assets are `locust-x86_64-windows.exe`, `locust-aarch64-macos`,
`locust-x86_64-macos` and `locust-x86_64-linux`.

Each artifact must have its **own exact filename plus `.sig`**. Asset ordering
never selects a signature, and duplicate names or multiple artifacts for a target
are refused rather than guessed:

| Updater platform key | Asset match |
| -------------------- | ----------- |
| `windows-x86_64`     | `*_x64_en-US.msi` + exact `.sig` |
| `windows-aarch64`    | `*_arm64_en-US.msi` (or `*_aarch64_en-US.msi`) + exact `.sig` |
| `darwin-aarch64`     | `*_aarch64.app.tar.gz` + exact `.sig` |
| `darwin-x86_64`      | `*_x64.app.tar.gz` + exact `.sig` |
| `linux-x86_64`       | `*_amd64.AppImage` + exact `.sig` |

The default is strict: **all five complete pairs are required** before any
signature download or manifest replacement. For explicit recovery only:

```bash
bash scripts/generate-updater-manifest.sh --allow-partial <tag>
```

Recovery permits a nonempty subset, omitting platforms with no complete pair.
It still refuses zero pairs, ambiguity and invalid signatures. The command only
writes a local manifest; it does not upload or publish a release. The workflow
never passes this flag, and its final inventory gate always requires all five
platforms and all four CLI binaries.

HTTP/download failures, empty/HTML/invalid signatures and ambiguous metadata fail
closed without retries. Signatures must be base64-encoded minisign documents
with the expected four-line structure and packet lengths. This is a shape check,
not cryptographic verification against the app's public key. All downloads and
validation finish before atomic replacement of local `latest.json`; a refusal
preserves the old file, leaves no scratch file and emits no manifest/raw network
response. The repository is `GITHUB_REPOSITORY` (default `Miike0303/Locust`); no
alternate endpoint or output destination is introduced.

`tauri-action` uses `includeUpdaterJson: false`; the finalizer's strict generator
is the sole source of `latest.json`. Its platform URLs are the eventual public
release URLs, even while signature files are downloaded from the private draft.

## `latest.json` shape

Tauri updater JSON (example — URLs and signatures come from the release assets):

```json
{
  "version": "0.2.0",
  "notes": "## [0.2.0] - 2026-10-17\n\n<release CHANGELOG section>",
  "pub_date": "2026-08-14T00:00:00Z",
  "platforms": {
    "windows-x86_64": {
      "signature": "<base64-encoded minisign document from this exact .msi.sig>",
      "url": "https://github.com/Miike0303/Locust/releases/download/v0.2.0/Project.Locust_0.2.0_x64_en-US.msi"
    },
    "windows-aarch64": {
      "signature": "…",
      "url": "https://github.com/Miike0303/Locust/releases/download/v0.2.0/Project.Locust_0.2.0_arm64_en-US.msi"
    },
    "darwin-aarch64": {
      "signature": "…",
      "url": "https://github.com/Miike0303/Locust/releases/download/v0.2.0/<aarch64>.app.tar.gz"
    },
    "darwin-x86_64": {
      "signature": "…",
      "url": "https://github.com/Miike0303/Locust/releases/download/v0.2.0/<x64>.app.tar.gz"
    },
    "linux-x86_64": {
      "signature": "…",
      "url": "https://github.com/Miike0303/Locust/releases/download/v0.2.0/project-locust_0.2.0_amd64.AppImage"
    }
  }
}
```

`notes` is the GitHub release body, extracted from the tagged version's
`## [X.Y.Z]` CHANGELOG section, including its heading and stopping before the
next level-two heading. A missing, empty or duplicate section fails validation.
`pub_date` is UTC now at manifest generation.

There is no “204 No Content” path: the file is always a JSON document. If the
running version is already current, the Tauri plugin simply does not prompt.

## Signing

**Tauri updater signatures** (minisign): configured. The public key lives in
`tauri.conf.json` → `plugins.updater.pubkey`. The private key is the GitHub
secret `TAURI_SIGNING_PRIVATE_KEY` (optional password:
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`). **Owner prerequisite:** restore/validate
a usable existing private key and matching password corresponding to the
configured public key before delivery. All five v0.2.0 signing jobs failed to
decode the configured credentials; the owner dependency is recorded in
[`odd/tasks/release-v0.2.0.md`, L15–L17](../odd/tasks/release-v0.2.0.md).
Configuration and signature shape/equality checks alone do not prove key
correspondence. Verify signed assets against that public key and test an older
installation before claiming updater acceptance. Ordinary release preparation
must not read/print secrets, rotate keys or disable signing. Follow the
[release checklist](../RELEASE.md).

**OS code-signing is not configured.** In `tauri.conf.json`:

- macOS `bundle.macOS.signingIdentity` is `null`
- Windows `bundle.windows.certificateThumbprint` is `null`

Installers are therefore not Apple-notarized / Authenticode-signed. Windows and
macOS may show unknown-publisher warnings. That is independent of the Tauri
updater signature check.

## Possible future: custom update server

Not implemented. A future alternative would be a small HTTP service (or a
static host) that serves the same Tauri JSON, for example:

```
GET https://example.com/{target}/{arch}/{current_version}
```

returning 200 + JSON when an update exists. The desktop would only use that if
`plugins.updater.endpoints` in `tauri.conf.json` were changed. Until then,
GitHub Releases + `latest.json` is the whole pipeline.
