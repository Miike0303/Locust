# Releasing Project Locust

Release only the exact candidate that passed Windows and Ubuntu CI. A successful
manifest job is **not** proof that every desktop or CLI platform shipped.

## Prerequisites

- Use the existing Node/Rust/Python toolchains and release workflow. Manifest
  generation needs Python 3, `gh` and `curl`; no `jq` dependency is required.
- Tauri `bundle.createUpdaterArtifacts` must be `true`. Updater signing uses the
  existing `TAURI_SIGNING_PRIVATE_KEY` and optional
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` GitHub Actions secrets, with the public key
  configured in `tauri.conf.json`. Do not print/read secrets or rotate keys as
  part of an ordinary release. Missing signing configuration is a release issue,
  not permission to publish unsigned updater bundles.
- OS signing is separate: Apple notarization and Windows Authenticode signing
  are not configured. Unknown-publisher warnings may still appear.

## Candidate checklist

- [ ] Synchronize the version in all release metadata:
  - `Cargo.toml` → `[workspace.package].version`.
  - `Cargo.lock` → the six inherited `locust-*` workspace packages only.
  - `apps/desktop/package.json` → root version.
  - `apps/desktop/package-lock.json` → root and `packages[""].version` only.
  - `apps/desktop/src-tauri/tauri.conf.json` → root version.
- [ ] Leave dependency requirements, resolved versions/checksums and the
  independent plugin-example version unchanged. No install/update is needed for
  a version-only lockfile edit.
- [ ] Keep `CHANGELOG.md`'s Unreleased heading; add the dated release with honest
  user-facing changes and unresolved support/verification limits. Exclude private
  game contents, local game paths, credentials and internal raw logs.
- [ ] Review the complete diff and stage **only explicitly reviewed paths**.
  Keep unrelated work and private evidence out of the release commit.
- [ ] Record the candidate commit SHA, publish the work branch non-forcibly and
  manually run `.github/workflows/ci.yml` on that exact SHA/ref. Confirm both
  `check` and `check-windows` succeeded and correspond to that SHA; cancellation,
  old green runs and successful local tests are not substitutes.

## Delivery checklist

The release owner performs these steps after candidate verification:

1. Confirm remote `main` is an ancestor of the tested candidate. Advance `main`
   by fast-forward only and push normally, without force. Stop on divergence.
2. Confirm the intended version tag does not already exist remotely. Create and
   push `v0.2.0` at the **same tested SHA**, never by moving an existing tag.
3. Observe `.github/workflows/release.yml` for that tag/SHA:
   - Desktop matrix: Windows x64/ARM64, macOS ARM64/x64, Linux x64.
   - CLI matrix: Windows x64, macOS ARM64/x64, Linux x64.
   - Updater manifest: runs after desktop jobs even if some failed; uploads only
     when at least one complete, valid artifact/signature pair exists.
4. Inspect the actual release body and assets, not just job summaries. Check
   versions, expected installers/CLI binaries, exact `.sig` companions and
   `latest.json` URLs/platform keys against the uploaded files. Ensure notes
   describe actual support limits and any missing platforms honestly.
5. Verify signatures against the configured public key using supported updater
   tooling before claiming signing-key correspondence. Offline shape tests do
   not prove cryptographic validity or that installed apps can update.
6. Record source SHA, CI runs, release runs and observed assets separately. A
   partial manifest may be useful, but never call it a complete five-platform
   desktop/four-platform CLI release when required builds/assets are missing.

## Updater contract

The endpoint remains:

```
https://github.com/Miike0303/Locust/releases/latest/download/latest.json
```

`scripts/generate-updater-manifest.sh <tag>` writes `latest.json` in the current
directory and prints that JSON on success. It uses `GITHUB_REPOSITORY`, defaulting
to `Miike0303/Locust`. A refusal exits nonzero, prints a sanitized reason to
stderr, prints no manifest to stdout and preserves an existing file. There are
no automatic network retries. See [updater details](docs/update-server.md) for
pairing rules and the intentionally partial-platform policy.

## Manual updater acceptance

On an isolated supported installation of an older version, verify the launch
check, download, signature verification and installation with the published
assets. Record the platform and actual outcome; neither a headless frontend test
nor a generated manifest establishes native updater acceptance. The current
version should not prompt for an update to itself. Users already on a bad newer
version may need to install an older known-good release manually; hiding a bad
release does not downgrade installed applications.
