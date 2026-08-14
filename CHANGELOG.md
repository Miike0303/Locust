# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Desktop interface language: English and Spanish, selectable under Settings → Appearance → Interface language (`locust.ui.language`). Separate from the translation target language.
- CLI binaries on GitHub Releases (`locust-x86_64-windows.exe`, `locust-aarch64-macos`, `locust-x86_64-macos`, `locust-x86_64-linux`) so players can run `locust apply` without installing the desktop app.
- CI workflow (`.github/workflows/ci.yml`): frontend build + unit tests, `cargo fmt`, clippy `-D warnings`, `cargo test --workspace`.

### Changed

- Each game now has its own SQLite file at `<game-parent>/<game-name>.locust.db` (fallback under the app data `projects/` directory if the parent is not writable). Desktop and HTTP open no longer share one global `project.db`.
- Opening a project **merges** instead of wiping: existing translations, statuses, and provider attribution survive. If the extracted source text for an id changed, the translation is kept but status is reset to pending. Ids that disappeared from the game are removed. Open response reports `added` / `updated` / `stale_source_reset` / `removed` / `preserved_translations`.
- Provider retry and per-provider-id rate limiting are wired at the single call site in `locust-core::translation` (previously unused). Transient failures only (429/5xx/timeout/network); auth errors are not retried; exponential backoff with jitter; overall deadline; cancellation wins during backoff.

### Fixed

- Opening a project no longer `DELETE FROM strings` on a shared database, which previously destroyed translations and approvals for every other game.
