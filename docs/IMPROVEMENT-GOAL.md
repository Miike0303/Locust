# Improvement goal

State for the `/goal` cycle. Read it before proposing anything; update it after every cycle.

## Goal

Make Locust's two products — the translation app and the patch tool — trustworthy for someone who is not a developer. The engine is far ahead of the surfaces that expose it: prefer work that removes a way for a user to lose work, be misled, or get stuck, over work that adds capability.

## Constraints

Standing bans. Each exists for a reason; do not re-litigate them.

- **Never write to a user's game files unattended.** Batch inject/pack in a queue is a footgun, not a feature. Read-only steps (validate) are fine.
- **No new dependency for what a few lines of stdlib or platform API can do.** The UI i18n layer is ~60 lines over `Intl.PluralRules` and needs no library.
- **Do not build the receiving half of a channel that has no sender.** Deep links, protocol handlers, and update servers wait until something exists to link from.
- **Never associate `.zip`.** Hijacking every archive on a user's machine for a game-translation tool is not acceptable scope. A `locust://` scheme is the correct granularity, when it is time.
- **Newlines in extracted text are not always width wrapping.** Unity Naninovel `@cmd` blocks and Unreal `.locres` paragraphs are semantic; re-wrapping them corrupts scripts. See `CLAUDE.md`.
- **Secrets never gain a derived `Debug`.** `TokenStore` has a hand-written redacting impl for this reason.

## Backlog

- No web presence of any kind: no landing, no docs site, no deploy. `crates/server` cannot serve static files (`tower-http` is compiled with `cors, trace` only). Deferred by the user, not rejected.

## In flight

Nothing.

## Done

- `pending` — Review pages instead of loading the project: it fetched up to 50k translated + 50k reviewed rows and concatenated them for a screen that shows one entry at a time. The queue now validates each item after translating, so a batch user finds breakage before injecting rather than one game at a time. Copy stopped naming internals at the user (`register-lang failed`, `locust server`, `plugins=` field dumps).

- `602cd92` — project-wide filter facets, pivot workflow in the app, `open-db` without extraction, Astro stub pointed at `locust apply`, dead chrome removed. Filters had been built from one page of ≤100 rows.
- `b6a8dc2` — patch apply streams progress over a job websocket, apply-only entry without a project, in-app grok-sub login, translation jobs survive closing their modal.
- `4354ce1` — **stopped destroying translations on project open.** `open_project` ran `DELETE FROM strings` every time, against one global database; opening game B destroyed game A.
- `a2f7952` — first workspace-wide `cargo fmt`, isolated so it could not bury the change beside it.

## Rejected

- **`.zip` file association / `locust://` protocol handler** — 2026-08-14. The handler exists so a user can click a link on a patch distribution site. No such site exists; the Astro stub points at third-party rule95. Building the receiver now means OS-level registration, per-platform, untestable against anything real. Revisit when the web decision is made.
- **Aligning the translation default languages** — 2026-08-14. The chain is `lastUsed ?? config ?? "auto"/"es"` and config always exists, so the hardcoded fallback is unreachable. Changing it changes nothing for anyone.
- **Moving ~700 lines of `#[cfg(test)]` fixtures to satisfy `items_after_test_module`** — 2026-08-14. Pure churn and real risk for a layout lint. Allowed with a written reason at `crates/formats/src/unity_serialized.rs`.
- **Batch inject/pack in the queue** — 2026-08-14. See Constraints.

## Failures

None yet.
