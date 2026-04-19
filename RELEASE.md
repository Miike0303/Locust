# Releasing Project Locust

This document describes how to cut a new release that users will receive via
auto-update.

## One-time setup

### 1. Generate signing keys

Tauri updates must be cryptographically signed. Generate a keypair:

```bash
cd apps/desktop
npm install -g @tauri-apps/cli
tauri signer generate -w ~/.tauri/locust.key
```

This outputs:
- **Private key**: `~/.tauri/locust.key` (KEEP SECRET)
- **Public key**: printed to stdout (copy it)

### 2. Configure the public key in the app

Edit `apps/desktop/src-tauri/tauri.conf.json` and paste the pubkey:

```json
"updater": {
  "pubkey": "YOUR_PUBLIC_KEY_HERE",
  ...
}
```

Commit this.

### 3. Store private key as GitHub secret

In the GitHub repo settings → Secrets and variables → Actions, add:

- `TAURI_SIGNING_PRIVATE_KEY` — contents of `~/.tauri/locust.key`
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` — the password you set when generating the key (or empty)

## Cutting a release

1. Bump the version in:
   - `apps/desktop/src-tauri/tauri.conf.json` (`"version"`)
   - `Cargo.toml` (workspace `[workspace.package]` → `version`, if used)
   - `apps/desktop/package.json`

2. Commit and push:
   ```bash
   git add -A
   git commit -m "Bump version to 0.2.0"
   git push
   ```

3. Tag the release:
   ```bash
   git tag v0.2.0
   git push origin v0.2.0
   ```

4. GitHub Actions (`.github/workflows/release.yml`) will:
   - Build the app for Windows, macOS (both arches), Linux
   - Sign each bundle with your private key
   - Upload bundles to a new GitHub release
   - Generate `latest.json` with signatures and upload it to the release

5. Users with the app will automatically see the update notification next time
   they start the app.

## Manual testing

To test the updater locally without publishing:

```bash
cd apps/desktop
npm run tauri dev
```

In-app, the UpdateChecker component runs a silent check on mount. The floating
update notification appears in the bottom-right when a newer version is found
on the configured endpoint.

To force a check: use `triggerUpdateCheck()` from `components/UpdateChecker.tsx`.

## Endpoint

The updater checks:

```
https://github.com/Miike0303/Locust/releases/latest/download/latest.json
```

This URL always points to the latest released `latest.json`, which contains
the signed URLs of the platform-specific bundles.

## Rollback

To roll back a bad release:

1. Delete or mark as prerelease the bad GitHub release.
2. The "latest" tag on GitHub will point to the previous stable release.
3. Users who already updated must manually download the previous version.
