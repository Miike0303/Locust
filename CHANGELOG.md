# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-10-17

### Added

- Desktop interface language: English and Spanish, selectable under Settings → Appearance → Interface language (`locust.ui.language`). Separate from the translation target language.
- CLI binaries on GitHub Releases (`locust-x86_64-windows.exe`, `locust-aarch64-macos`, `locust-x86_64-macos`, `locust-x86_64-linux`) so players can run `locust apply` without installing the desktop app.
- Two-platform CI for frontend build/unit tests, Rust format/lint/workspace tests, Python pipeline/replay/RGSS oracles and offline updater-manifest checks.
- Native Godot/Dialogic support, explicitly **Experimental**: selected standalone Godot 4.0–4.3 PCK v2 archives, bounded timeline/character text extraction, translated archive copies, Direct revisions and strict patch/rollback workflows. Unsupported constructs report extraction warnings; this is not general Godot resource or encryption support.
- Safe JA→EN→ES wrapper profiles for Ren’Py, RPG Maker MV/MZ, KiriKiri and, now, generic HTML as the fourth profile. HTML verifies ordered visible slots and full artifact bytes before packaging. Optional per-phase glossaries and conservative English seed filtering are bound to resume/configuration proofs.
- Independent RPG Maker VX Ace QA for a **six-row synthetic fixture**, checking EN/ES full injected artifacts and Spanish install/rollback/reapply. This does not admit XP/VX/Ace into the generic safe wrapper.
- Pivot projects with immutable physical-source provenance, complete-pivot validation, binary-capacity preflight and explicit opt-in for lossy binary fitting.
- Patch packaging, URL verification/apply, strict baseline binding, rollback preview, backup listing/restoration and optional distribution-entry preparation across CLI and desktop workflows.
- Additional experimental format profiles and bounded Unity SerializedFile/TextAsset, Unreal LocRes/PAK/IoStore, and visual-novel resource handling; capabilities and limits remain format-specific.
- Desktop project resume, in-memory editor drafts retained across navigation (not app reload/exit), review/validation worklists, project-scoped glossary and translation memory, accessible dialogs, semantic appearance tokens and clearer English/Spanish recovery guidance.

### Changed

- Each game now has its own SQLite file at `<game-parent>/<game-name>.locust.db` (fallback under the app data `projects/` directory if the parent is not writable). Desktop and HTTP open no longer share one global `project.db`.
- Opening a project **merges** instead of wiping: existing translations, statuses, and provider attribution survive. Changed source text keeps its translation but resets status to pending. Complete extractions remove disappeared IDs; incomplete extractions preserve absent saved rows and report warnings. Failed opens keep the live project. Verified Direct projects can resume saved rows without re-extracting translated output.
- Provider retry and per-provider-id rate limiting are wired at the single call site in `locust-core::translation` (previously unused). Transient failures only (429/5xx/timeout/network); auth errors are not retried; exponential backoff with jitter; overall deadline; cancellation wins during backoff. Provider-specific batch defaults, glossary hints and length-aware retries respect explicit settings and physical byte limits.
- Backup generations can share verified identical payload files while retaining independent restore inventories; external corruption of shared payloads can affect multiple backups.
- Release metadata is synchronized at 0.2.0 without dependency upgrades. Signed Tauri updater artifacts are explicitly enabled; manifest generation pairs each installer with its exact signature, including Windows ARM64, and refuses HTTP errors, malformed signatures, ambiguous assets or zero complete pairs without replacing an existing manifest. Partial-platform manifests remain supported.

### Fixed

- Opening a project no longer `DELETE FROM strings` on a shared database, which previously destroyed translations and approvals for every other game.
- Injection and patch operations reject unsafe provenance, drift, interrupted/ambiguous state and incompatible representations before mutation. YU-RIS Spanish guidance explains Shift-JIS refusal while preserving raw diagnostics; it does not add accented-Spanish runtime support.
- CRLF-saved PO/XLIFF catalogs preserve source identity; stale sources still refuse. RPG Maker message wrapping/list boundaries, format-aware placeholders and runaway repetition handling preserve supported engine controls.
- Concurrent saves detect stale edits, review approval saves current text, incomplete translation jobs no longer report unqualified success, and reconnectable job streams retain cancellation and terminal results.

### Verification limits

- Artifact/fixture checks are not certification of native Tauri windows, gameplay, fonts, layout or human linguistic quality. Live Spanish YU-RIS compatibility, broader structural Add confirmation, generic RGSS pipelines and arbitrary encrypted resources remain unproved or unsupported. See [the current task index](odd/TASKS.md) and format-specific guides for bounded evidence.
- Release publication and signing-key correspondence require exact-candidate hosted CI and inspection of uploaded assets; this changelog is not proof that all release platforms succeeded.
