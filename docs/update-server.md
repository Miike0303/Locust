# Tauri Auto-Update Server Specification

## Endpoint Format

```
GET https://releases.projectlocust.com/{target}/{arch}/{current_version}
```

Where:
- `{target}` — OS target: `darwin`, `linux`, `windows`
- `{arch}` — Architecture: `x86_64`, `aarch64`
- `{current_version}` — Current app version, e.g. `0.1.0`

## Response Format

If an update is available, return HTTP 200 with JSON:

```json
{
  "version": "0.2.0",
  "notes": "Bug fixes and performance improvements",
  "pub_date": "2025-01-15T00:00:00Z",
  "platforms": {
    "darwin-x86_64": {
      "signature": "...",
      "url": "https://github.com/Miike0303/Locust/releases/download/v0.2.0/Project.Locust.app.tar.gz"
    },
    "darwin-aarch64": {
      "signature": "...",
      "url": "https://github.com/Miike0303/Locust/releases/download/v0.2.0/Project.Locust.app.tar.gz"
    },
    "linux-x86_64": {
      "signature": "...",
      "url": "https://github.com/Miike0303/Locust/releases/download/v0.2.0/project-locust_0.2.0_amd64.AppImage.tar.gz"
    },
    "windows-x86_64": {
      "signature": "...",
      "url": "https://github.com/Miike0303/Locust/releases/download/v0.2.0/Project.Locust_0.2.0_x64-setup.nsis.zip"
    }
  }
}
```

If no update is available, return HTTP 204 No Content.

## Implementation Options

### Option 1: GitHub Releases (Recommended)
Use GitHub Releases as the source of truth. Deploy a simple Cloudflare Worker or Vercel Edge Function that:
1. Fetches latest release from GitHub API
2. Compares with `current_version`
3. Returns the update JSON or 204

### Option 2: Static JSON
Host a static `latest.json` file that gets updated on each release via CI.

## Signing
All updates must be signed with the Tauri updater key pair.
Generate keys with: `cargo tauri signer generate -w ~/.tauri/locust.key`
Store the private key securely (GitHub Secrets).
The public key goes in `tauri.conf.json > plugins > updater > pubkey`.
