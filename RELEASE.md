# Releasing Project Locust

Release only the exact candidate that passed Windows and Ubuntu CI. A successful
manifest job is **not** proof that every desktop or CLI platform shipped.

## Prerequisites

- Use the existing Node/Rust/Python toolchains and release workflow. Manifest
  generation needs Python 3, `gh` and `curl`; the release gate needs Python 3.11+.
  No `jq` dependency is required.
- Tauri `bundle.createUpdaterArtifacts` must be `true`. Updater signing uses the
  existing `TAURI_SIGNING_PRIVATE_KEY` and optional
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` GitHub Actions secrets, with the public key
  configured in `tauri.conf.json`. **Owner prerequisite:** the private key and
  password must be usable together and match that configured public key. The
  v0.2.0 attempt failed all five desktop signing steps because the configured
  credentials could not be decoded; see
  [`odd/tasks/release-v0.2.0.md`, L15–L17](odd/tasks/release-v0.2.0.md).
  The owner must repair the existing secrets before another delivery attempt.
  Do not print/read secrets, rotate the identity or disable signing as part of
  an ordinary release. Missing signing configuration is a release issue,
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
   push the intended `vX.Y.Z` at the **same tested SHA**, never by moving an
   existing tag.
3. Observe `.github/workflows/release.yml` for that tag/SHA. Manual dispatch
   must also select the actual stable `vX.Y.Z` tag, not a branch:
   - `validate` first checks the tag against `Cargo.toml`,
     `apps/desktop/package.json` and `tauri.conf.json`, and requires one nonempty
     `## [X.Y.Z]` CHANGELOG section for the release body.
   - It creates a **draft** bound to the candidate SHA, or reuses a matching
     draft. A public release, prerelease or different target SHA is refused.
     Runs for the same ref are serialized.
   - Desktop matrix: Windows x64/ARM64, macOS ARM64/x64, Linux x64. All upload to
     that draft. Tauri action's `args: ... -- --locked` forwards `--locked` through
     `tauri build` to Cargo; CLI builds also use `--locked`.
   - CLI matrix: Windows x64, macOS ARM64/x64, Linux x64; uploads to the draft.
   - Only `publish`, after **all nine builds succeed**, generates strict
     `latest.json`, uploads it and downloads it again for verification.
4. The finalizer requires all five desktop artifact families with exact `.sig`
   companions, all four named CLI binaries and `latest.json`, each nonempty.
   It checks draft/tag/source SHA, all five manifest platform keys, asset URLs,
   signature shape/equality and changelog-derived notes. It then alone publishes
   the draft and marks it latest. Any build, download or verification failure
   leaves the draft unpublished; the existing public latest is unchanged.
   Inspect the resulting release body/assets and record the finalizer outcome.
5. Verify signatures against the configured public key using supported updater
   tooling before claiming signing-key correspondence. Offline shape/equality
   tests do not prove cryptographic validity or that installed apps can update.
6. Record source SHA, CI runs, release runs and observed assets separately.
   Recovery from a failed run may reuse only the matching draft at the same
   candidate SHA. Never move an existing tag or bypass the final inventory gate.

## Updater contract

The endpoint remains:

```
https://github.com/Miike0303/Locust/releases/latest/download/latest.json
```

`bash scripts/generate-updater-manifest.sh <tag>` writes `latest.json` in the current
directory and prints that JSON on success. It uses `GITHUB_REPOSITORY`, defaulting
to `Miike0303/Locust`. A refusal exits nonzero, prints a sanitized reason to
stderr, prints no manifest to stdout and preserves an existing file. There are
no automatic network retries. By default it requires **all five** platform pairs.
An explicit recovery command,
`bash scripts/generate-updater-manifest.sh --allow-partial <tag>`, permits a
nonempty subset of valid pairs. This only generates a local manifest; it does
not upload, publish or bypass the release gate, and the workflow never uses it.
Draft signature downloads use authenticated `gh`; public release recovery uses
`curl`. See [updater details](docs/update-server.md) for pairing rules.

## Manual updater acceptance

On an isolated supported installation of an older version, verify the launch
check, download, signature verification and installation with the published
assets. Record the platform and actual outcome; neither a headless frontend test
nor a generated manifest establishes native updater acceptance. The current
version should not prompt for an update to itself. Users already on a bad newer
version may need to install an older known-good release manually; hiding a bad
release does not downgrade installed applications.
