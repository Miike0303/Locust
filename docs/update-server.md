# Desktop auto-update (GitHub Releases)

The desktop app does **not** talk to `releases.projectlocust.com`. That host is not
referenced anywhere in the code. The live updater endpoint is a GitHub Release asset.

## Endpoint (what actually runs)

Configured in `apps/desktop/src-tauri/tauri.conf.json` → `plugins.updater.endpoints`:

```
https://github.com/Miike0303/Locust/releases/latest/download/latest.json
```

`/releases/latest/download/…` always follows the newest non-prerelease GitHub release.
The app checks this URL on launch (`UpdateChecker`). Tauri compares `latest.json`'s
`version` to the running app and, if newer, offers Download & install.

## How `latest.json` is produced

On each `v*` tag (and `workflow_dispatch`), `.github/workflows/release.yml`:

1. **Desktop job** — builds Windows (x64 + ARM64), macOS (arm64 + x64) and Linux
   x64 bundles with `tauri-apps/tauri-action`. `bundle.createUpdaterArtifacts: true`
   explicitly requests updater artifacts signed through the existing
   `TAURI_SIGNING_PRIVATE_KEY` configuration, alongside the release installers.
2. **CLI job** — builds and uploads `locust` binaries (not part of the updater
   manifest). See [USAGE.md](../USAGE.md#7-applying-a-patch).
3. **`update-manifest` job** — runs `scripts/generate-updater-manifest.sh <tag>`.
   The shell entrypoint delegates to Python 3 stdlib, using `gh` to read release
   assets and `curl --fail --silent --show-error --location` to fetch signatures.
   Python is explicitly provisioned in this job. After success the workflow runs
   `gh release upload … latest.json --clobber`.

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

Platforms with no complete pair are omitted; the job is `if: always()` so a
single OS failure does not block a manifest for the rest. **Zero complete pairs
is a failure**, not an empty successful manifest. A manifest with fewer than five
platforms is intentionally supported and is not proof of a complete release.

HTTP/download failures, empty/HTML/invalid signatures and ambiguous metadata fail
closed without retries. Signatures must be base64-encoded minisign documents
with the expected four-line structure and packet lengths. This is a shape check,
not cryptographic verification against the app's public key. All downloads and
validation finish before atomic replacement of local `latest.json`; a refusal
preserves the old file, leaves no scratch file and emits no manifest/raw network
response. The repository is `GITHUB_REPOSITORY` (default `Miike0303/Locust`); no
alternate endpoint or output destination is introduced.

`tauri-action` is also invoked with `includeUpdaterJson: true`; the dedicated
script is the source of truth because it overwrites `latest.json` on the release.

## `latest.json` shape

Tauri updater JSON (example — URLs and signatures come from the release assets):

```json
{
  "version": "0.2.0",
  "notes": "See CHANGELOG for details. The app will auto-update from this release.",
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

`notes` is the GitHub release body (`release.yml` currently sets that body to
“See CHANGELOG for details…”). `pub_date` is UTC now at manifest generation.

There is no “204 No Content” path: the file is always a JSON document. If the
running version is already current, the Tauri plugin simply does not prompt.

## Signing

**Tauri updater signatures** (minisign): configured. The public key lives in
`tauri.conf.json` → `plugins.updater.pubkey`. The private key is the GitHub
secret `TAURI_SIGNING_PRIVATE_KEY` (optional password:
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`). Configuration alone does not prove that
secrets are available or correspond to the configured public key. Inspect actual
signed assets and verify key correspondence before claiming updater acceptance;
ordinary release preparation must not read secrets or rotate keys. Follow the
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
