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

1. **Desktop job** — builds Windows / macOS (arm64 + x64) / Linux bundles with
   `tauri-apps/tauri-action`, signs them with `TAURI_SIGNING_PRIVATE_KEY`, and
   uploads the installers to the GitHub release.
2. **CLI job** — builds and uploads `locust` binaries (not part of the updater
   manifest). See [USAGE.md](../USAGE.md#7-applying-a-patch).
3. **`update-manifest` job** — runs `scripts/generate-updater-manifest.sh <tag>`.
   The script uses `gh` + `curl` + `jq` to read that release's assets, fetch each
   matching `.sig`, and write `latest.json`. It then `gh release upload … latest.json --clobber`.

The script maps assets as follows (first match wins):

| Updater platform key | Asset match |
| -------------------- | ----------- |
| `windows-x86_64`     | `*_x64_en-US.msi` + `.msi.sig` |
| `darwin-aarch64`     | `aarch64*.app.tar.gz` + `.sig` |
| `darwin-x86_64`      | `x64*.app.tar.gz` + `.sig` |
| `linux-x86_64`       | `*_amd64.AppImage` + `.AppImage.sig` |

Platforms that failed to upload are omitted; the job is `if: always()` so a
single OS failure does not block a manifest for the rest.

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
      "signature": "<minisign from the .msi.sig asset>",
      "url": "https://github.com/Miike0303/Locust/releases/download/v0.2.0/Project.Locust_0.2.0_x64_en-US.msi"
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
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`). Generate a keypair with
`cargo tauri signer generate` / `tauri signer generate` as described in
[RELEASE.md](../RELEASE.md).

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
