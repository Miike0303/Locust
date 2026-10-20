# Project Locust

**LOC**alization **U**niversal **S**cripting **T**ool — a free, open-source desktop app that translates video games across engines, providers, and languages. Runs on Windows, macOS, and Linux.

![License](https://img.shields.io/badge/license-MIT-green)
![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-blue)
![Status](https://img.shields.io/badge/status-alpha-orange)

---

## What it does

Locust extracts translatable text from game files, runs it through a translation provider (free or paid), and injects the translations back — all while **preserving variables, formatting tags, and code** so the game keeps working.

```
  extract → SQLite DB → translate → inject → patch zip → apply (verify/backup/receipt)
                ↑ edit/review in desktop or export PO/XLIFF anytime
```

You can stop at any stage, review/edit translations in the built-in editor, then continue.
Progress is saved in the project database (one file per game; reopening merges, it does not wipe).
Prefer **patch → apply** on a *copy* of the game (never the only original without backup).

The desktop interface itself is available in **English and Spanish** (Settings → Appearance →
Interface language). That setting is separate from the language you translate the game into.

---

## Supported game engines

Stability matches `locust formats` / the desktop Welcome screen
(`stable` | `experimental`). The API still has a `coming soon` label; no registered plugin uses it.

### Stable

| Engine                    | Extensions            | Notes |
| ------------------------- | --------------------- | ----- |
| **RPG Maker MV / MZ**     | `.json`, `.jsono`      | Maps, events, system, plugins; shared `rpgmaker-mv` plugin |
| **RPG Maker XP / VX / VX Ace** | `.rxdata`, `.rvdata`, `.rvdata2` | Ruby Marshal data |
| **Ren'Py**                | `.rpy`, `.rpyc`, `.rpa` | Loose scripts + RPA; orphan compiled scripts use a runtime filter in Replace mode; Add-mode `game/tl/<lang>/` language packs |

Compiled Ren'Py scripts are read with a bounded, non-executing pickle reader,
including supported protocol-4/5 opcodes. Python globals and constructors are
never executed; protocol-5 out-of-band buffers are not supported. Unsupported or
malformed compiled scripts produce partial-extraction warnings rather than
claiming complete extraction. Rejected RPA member paths, out-of-bounds or
oversized scripts, and unreadable script members also produce partial-extraction
warnings; valid siblings are still extracted, and reopening a project preserves
saved rows absent from that incomplete extraction.

### Experimental (Phase-2 extract → inject → patch → apply proven on fixtures or sample games)

| Engine                    | Extensions       | Notes |
| ------------------------- | ---------------- | ----- |
| **SugarCube / Twine**     | `.html`, `.htm`  | Interactive fiction |
| **HTML (generic)**        | `.html`, `.htm`  | Non-SugarCube HTML adventures |
| **Unity**                 | `.assets`, `level*`, `data.unity3d` | Validated TextAsset resizing in SerializedFile v17–22 / UnityFS v6–8; other heuristic slots remain fixed-size |
| **Unreal Engine**         | `.pak`, `.locres`, `.utoc` / `.ucas` | Structural LocRes from supported classic/modern PAK and indexed IoStore ExternalFile chunks; writes `*_LOCUST_P.pak` overlays |
| **Wolf RPG Editor**       | `.wolf`          | Shift-JIS binary; length ≤ source |
| **VNTextPatch JSON**      | `.json`          | VN intermediate (KiriKiri/YU-RIS/… via VNTextPatch); Phase-2 patch apply proven on fixture |
| **QSP**                   | `.qsp`, `.gam`   | QuestSoft Player; synthetic fixture only |
| **TyranoBuilder**         | `.ks`, `.asar`, `.nw`, `.exe` | `data/scenario` loose + `app.asar` + NW.js `package.nw` / `data.exe`; UTF-8 |
| **KiriKiri / KAG**        | `.ks`, `.xp3`    | Loose + XP3; FE FE 0/1/2 (zlib); patch.xp3 write; no cxdec |
| **YU-RIS**                | `.ybn`, `.ypf`   | Loose YSTB + YPF unpack/repack (common versions); synthetic + real-game YSTB |
| **NScripter / ONScripter**| `0.txt`, `nscr_sec.dat`, `nscript.dat` | Shift-JIS; rot5 / XOR 0x84; synthetic; no NSA / nscript.___ |
| **Godot / Dialogic**      | `.pck`           | Selected standalone Godot 4.0–4.4 PCK v2 with supported stock Dialogic timelines; archive-only Replace. Godot 4.4 support is proven on synthetic fixtures only; no real 4.4 game has been verified. See [support limits](docs/GODOT-DIALOGIC.md). |

Not seeing your engine? [Open an issue](https://github.com/Miike0303/Locust/issues).

---

## Supported translation providers

Always registered (`locust providers`):

- **Google Translate** (free web endpoint) — no API key
- **Argos Translate** — free offline
- **Ollama** / **LM Studio** — free local models
- **Mock** — length-safe for binary-engine inject tests

When API keys are configured: **DeepL**, **OpenAI**, **Anthropic Claude**, **DeepSeek**, **Grok** (xAI), **Google Gemini**.
**Grok SuperGrok** (`grok-sub`) appears after `locust auth grok`.
**Antigravity** (`antigravity`) appears when an existing `agy` executable is found on PATH or through `providers.antigravity.extra.agy_path`. Authentication is managed by the Antigravity CLI.

### Providers and credentials

Configure provider credentials in Settings or in `config.json` under the [config directory](#where-locust-keeps-data). Keys are stored as JSON in `providers.<id>.api_key`; the settings API masks stored keys in its responses.

| Provider IDs | Credentials |
| ------------ | ----------- |
| `deepl`, `openai`, `claude`, `deepseek`, `grok`, `gemini` | API key required |
| `google`, `argos`, `lmstudio`, `ollama`, `mock` | No user API key required |
| `grok-sub` | OAuth subscription login below |
| `antigravity` | Existing Antigravity CLI login; no API key |

For **Grok subscription** access, run `locust auth grok`, open the printed URL in your browser, enter the displayed code and approve access. Locust polls for approval and saves access/refresh tokens in `xai-oauth.json` in the config directory. Tokens refresh automatically; if the session expires, run the command again. xAI may refuse access for some subscription tiers.

For **Antigravity**, log in through its CLI and make an existing `agy` executable available on PATH, or set `providers.antigravity.extra.agy_path` to its path. Finding the executable makes the provider available; Antigravity manages authentication.

Local providers connect to a separately running service. Their defaults are:

| Provider | Default base URL | Translation endpoint / default model |
| -------- | ---------------- | ------------------------------------ |
| Argos (`argos`) | `http://localhost:5000` | `/translate`; source/target language pair |
| LM Studio (`lmstudio`) | `http://localhost:1234` | `/v1/chat/completions`; `qwen/qwen3-14b` |
| Ollama (`ollama`) | `http://localhost:11434` | `/api/chat`; `llama3.2` |

Set `providers.<id>.base_url` to change the service address; LM Studio and Ollama also accept `providers.<id>.model`. Start the service and make the requested model/language pair available before translating.

### Privacy and cost

Translation sends source text to the selected provider, including remote services such as free Google Translate. LLM requests can also include game context and glossary hints. Argos, LM Studio and Ollama keep translation requests on your machine **when their services run locally**; a remote `base_url` sends text to that server instead. Antigravity uses an external CLI and cloud service, not a local translation model.

`cost_limit_usd` is an optional per-run USD limit (`locust translate ... --cost-limit <USD>` in the CLI). With a limit, provider calls run sequentially: Locust checks accumulated observed cost plus the next batch's estimate before sending it and refuses a batch that would exceed the limit. Additional translation/correction calls share the run's budget. It also stops further calls if reported cost exceeds the limit or becomes unknown. These checks depend on provider estimates and reported usage; they cannot guarantee the provider's final bill.

A paid provider without a cost estimate is refused when a limit is set; unknown cost is not treated as zero. Without a limit, translation can proceed with incomplete cost reporting. To retain a limit, choose a provider with usable estimates (or a free provider). Removing the limit allows unknown-cost calls and gives up that guard.

#### Model policy configuration

`config.json` accepts an optional top-level `model_overrides` map keyed by provider ID, then **exact effective model name** (case-sensitive, no aliases or default-model inference):

```json
{
  "model_overrides": {
    "example-provider": {
      "example-model": { "context_window": 8192 }
    }
  }
}
```

Each policy may contain `context_window` (integer from 1 through 18446744073709551615) and `input_usd_per_million` / `output_usd_per_million` (both finite nonnegative numbers, or both absent). Zero prices are allowed. Prices are user-supplied USD per million tokens; Locust does not seed a current-market price catalog.

Partial settings updates preserve omitted fields and other model/provider entries. Set `context_window` to JSON `null` to reset it; reset prices by setting **both** price fields to `null` in the same update. Clearing just one price while the other remains configured is rejected. An absent or empty policy means legacy behavior. Invalid policies are rejected on load, partial update and save before replacing saved configuration; unrelated provider fields and masked API-key preservation keep their existing behavior.

At provider registration, policies match the adapter's constructor-resolved model (including its default), never a guessed alias. Paired prices override the existing approximate character-based estimate (1.3 input and 1.3 output tokens per character) and costs calculated from reported input/output usage for OpenAI, Claude, Grok subscription and paid OpenAI-compatible endpoints. Missing usage remains unknown, even with zero configured prices; batch usage/cost stays on the first result only. Prices are not applied to free or non-token adapters (including LM Studio, Ollama, Google, DeepL and Antigravity). Context-only policies preserve existing rates; absent, empty or unmatched policies preserve legacy behavior.

Effective-model and matched-policy metadata are available internally for model-based adapters, including Ollama and Antigravity. Context windows do **not yet** affect batching, and no estimate endpoint or Settings editor is added here. A configured context window is not proof of tokenizer or prompt fit; configured prices are user assumptions, not verified service charges.

---

## Install

### Pre-built binaries (recommended)

Download the latest release for your OS from the [Releases page](https://github.com/Miike0303/Locust/releases).

- **Windows**: `Project.Locust_<ver>_x64_en-US.msi` — run the installer
- **macOS**: `Project.Locust_<ver>_<arch>.dmg` — open and drag to Applications
- **Linux**: `project-locust_<ver>_amd64.AppImage` — `chmod +x` then run

The app checks for updates on launch and offers to install them one-click.

### Installing unsigned builds

Windows Authenticode signing and Apple signing/notarization are not configured, so OS unknown-publisher warnings may appear. See [release requirements](RELEASE.md#prerequisites). Tauri **updater signatures are separate**: updater artifacts use a signing key and the public key configured in the app. An updater signature does not make an installer OS code-signed or notarized.

### CLI (apply a patch without the desktop app)

The same release also ships `locust` binaries: `locust-x86_64-windows.exe`,
`locust-aarch64-macos`, `locust-x86_64-macos`, `locust-x86_64-linux`.

Players who only need to apply a translation patch:

```bash
locust apply "<game-copy>" game-es-patch.zip
# or:
locust apply "<game-copy>" --url "https://example.com/game-es-patch.zip"
```

### Build from source

Use current stable Rust (validated with Rust 1.94) and Node.js 20+.
Patch coordination uses the standard library's file-lock API, so older toolchains
without that API cannot build this version.

```bash
git clone https://github.com/Miike0303/Locust.git
cd Locust
npm --prefix apps/desktop ci
npm --prefix apps/desktop run build
cargo build --release -p locust-desktop
# Launch: target/release/locust-desktop(.exe)
```

---

## Quick start

1. **Launch the app** and click *Open Game Folder* (or *Open Game File*).
2. Pick the game's directory (the one containing `Game.exe`, `index.html`, etc.).
3. Locust auto-detects the format and extracts all translatable strings.
4. Click **Translate** in the editor toolbar, choose a provider, and confirm both **source** and **target** languages before starting. Defaults depend on project text, saved preferences/settings and, when the target matches the detected source, the UI language; Spanish is not a universal default.
5. Watch the progress — translations are saved to the database as they come in.
6. Edit in the desktop editor as needed:
   - **Validate** (Ctrl+Shift+V) — placeholders + binary inject-slot length (Unity/Unreal/Wolf).
   - **Search & replace** (Ctrl+Shift+F) — bulk edit translations only (one DB batch).
   - **Export / Import** (Ctrl+E) — PO or XLIFF for external CAT tools (ids survive round-trip).
7. Click **Inject**, select the single target language translated in this database, and choose:
   - **Add mode** *(where supported, e.g. Ren'Py)* — language packs beside the original tree.
   - **Replace mode** — writes into a work copy (always keep a pristine original).
8. Package and apply a patch (recommended distribution path):
   - Desktop: Editor → **Patch** (Ctrl+Shift+P), or CLI below.
9. Play your translated game.

### Speaker workspace

Open **Speakers** in the sidebar to see character names and row counts. Select a character to save or clear a project-specific note, or open their rows in the existing editor worklist. **Unassigned rows** also open in the editor; compiled dialogue without supported speaker evidence stays unassigned rather than guessing a character.

Saved notes apply to the **next translation snapshot**, not an active job, and may be sent to the selected provider as character context. Providers may ignore context. Notes belong to the current project database, game and format; they do not rename characters or alter extracted speaker metadata.

### CLI sketch

```bash
locust extract "<game>" -o project.locust.db
locust translate project.locust.db -p mock -s en -t es   # or grok-sub / google / …
locust inject "<work_copy>" -P project.locust.db -l es --direct
locust patch "<work_copy>" -P project.locust.db -l es -o game-es-patch.zip --pristine "<pristine_copy>"
locust apply "<clean_copy>" game-es-patch.zip
locust patch-status "<clean_copy>"
locust patch-rollback "<clean_copy>"   # restores .locust/backup
```

After a new Direct injection, `patch` can omit `--pristine`: it retrieves the exact recorded backup and verifies its original hashes, including after restart or switching between CLI and desktop profiles. Repeating an injection that writes nothing keeps the previous association. A missing or modified recorded backup stops packaging; supply an intact `--pristine` copy to recover. Older records without backup provenance retain the legacy fallback behavior. The backup belongs to that injection generation, which may start from an already translated working copy.

### Machine-readable injection reports

```bash
locust inject "<work_copy>" -P project.locust.db --direct -l es --json > inject.json
locust inject "<work_copy>" -P project.locust.db -m add -l es --json > inject.json
# Replace also supports --json (requires -l es and -o <output_dir>).
```

`inject --json` writes exactly one existing core report object to stdout, without
an envelope: `mode: "direct"` selects `DirectInjectReport`; `mode: "add"` or
`"replace"` selects `MultiLangReport`. Each language's `reports` entry includes
counts, `files_written`, `warnings`, and `skip_reasons` (including `unclassified`
when the plugin supplies no precise reason). Direct exposes `outcomes` as
`[language, outcome]` pairs and retained `backup_id`, `backup_path`, and
`pristine_backup_id`; Add/Replace expose `recording_outcomes`, `injected_roots`,
and `backup_id`. Outcomes retain core serialization: `{"Recorded":{"files":N}}`,
`{"KeptPrevious":{"recorded_at":"..."}}`, or `"NothingRecorded"`.

Counts retain engine semantics: a pristine-replay no-op can count staged strings
while writing zero files. Inspect paths, skips, warnings, and recording outcomes,
not just `strings_written` or exit status. JSON adds no fail-on-warning policy.
Diagnostics/logs go to stderr; unflagged human output is unchanged. Failed
languages still exit nonzero after emitting the report when one is available;
earlier fatal errors emit stderr diagnostics with no report.

### Complete Japanese → English → Spanish pivot

After translating a Japanese project into English, gate the next stage on every
extracted row rather than silently pivoting only the finished subset:

```bash
locust pivot game-en.locust.db -o game-es.locust.db --require-complete
locust translate game-es.locust.db -p google -s en -t es
```

`--require-complete` refuses an empty project or any row with a missing/blank
translation (including Unicode whitespace), a status other than translated,
reviewed or approved, a stale source, or damaged protected controls. Refusal
creates no output database or sidecars and leaves source rows and existing output
files unchanged. A successful pivot carries every row into the new project's
source text while retaining the original injection baseline. This gate checks
recorded completeness and controls, not linguistic quality or game runtime behavior.
Without the flag, `pivot` retains its partial-editing workflow and skips missing or
blank translations. Existing output files are never overwritten in either mode.

Got suspicious strings? Open the Editor, filter by tag (`dialogue`, `ui_label`, `menu`, etc.), and edit or approve translations manually before injecting.

If a Direct/Add insertion is interrupted, run `locust inject-status "<work_copy>"` and `locust inject-recover "<work_copy>"` before modifying that game again. Recovery uses verified originals and preserves conflicting edits when explicitly forced. See [insertion recovery](docs/INJECTION-RECOVERY.md) for commands, backup behavior, disk costs, and limits.

---

## What gets translated (and what doesn't)

Locust is conservative by design — **variables, image paths, scripts, and internal identifiers are never touched**. This matters because translating the wrong thing can crash the game or break saves.

**Translated**
- Dialogue (`character "text"`, `centered "text"`, narrator lines)
- Menu choices, UI buttons, tooltips, notifications
- Character names, item names, stat labels, skill descriptions
- Map/location display names
- Credits, help screens, about text

**Not translated** (automatically filtered)
- `script`, `python`, `init python:` blocks
- Image/sound paths (`gui/foo.png`, `audio/bgm.ogg`)
- Variable references (`[player_name]`, `{0}`, `%s`) — protected with placeholders
- Style properties, screen element IDs, config flags
- Plugin tags in RPG Maker note fields (`<Augment: X>`, `<Cooldown: 2>`)
- Commented-out code

---

## Saving progress

Every project is saved as a `<game-name>.locust.db` file next to your game folder (or in the app data directory). It contains:

- All extracted strings with their source/target text
- Translation status (pending, translated, reviewed, approved)
- Translation memory (for reusing translations across projects)
- Glossary (for consistent terminology)

You can close Locust and reopen the same project later — your progress persists. Reopening a game merges newly extracted strings; existing translations stay. If the original text for a line changed, Locust keeps the translation but marks it pending so a stale line cannot ship as approved. Use the **Review** tab to audit translations before finalizing.

### Speaker notes API and translation context

`GET /api/speakers` accepts `database_path`, `project_path` and `format_id` query parameters matching the current project's exact identity (`project_path` is `path` in `/api/project/current`). It returns `{ groups: [{ id, name, count, entry_ids, note }], unassigned_entry_ids }` from current rows. IDs are opaque, case-sensitive JSON tuple strings; clients must echo them unchanged.

`PUT /api/speakers/note` accepts those same ownership fields plus `speaker_id` and `note`, and returns `{ speaker_id, note }`. An empty note clears it. No open project returns 400; stale identity or a busy project returns 409; an unknown current speaker returns 404. Notes persist in the project database under `locust.speakers.notes.v1`; unrelated metadata is preserved. Notes for removed speakers remain saved but do not create groups.

Recognized identities are explicit `speaker` / `speaker_name` / `character` string metadata (conflicts remain unassigned), Ren'Py `.rpy` dialogue context with Ren'Py format evidence, and version-1 Godot/Dialogic structured text locators. Names/layout tags and arbitrary context are not inferred as speakers. Translation runs prefer explicit row `format_id`; without it, only native `.rpy` dialogue line locators or version-1 Godot/Dialogic text locators qualify. No game files are read to resolve notes.

Translation jobs snapshot notes once (including across fallback providers). A matching nonblank note is appended as record-local translation data without replacing scene/game context, glossary, placeholder or fit hints. Identical sources with different effective contexts remain separate provider records; stored row context is unchanged. Noted rows bypass source-only translation memory reads and writes to avoid cross-character reuse; exact glossary matches still take precedence. OpenAI-compatible, Claude and Antigravity prompts already scope metadata per record. For Ollama, a job with matching nonblank notes uses single-record calls so each note stays with its own source, including during retries and fallback. Ollama's prompt and ordered-source-array wire format are unchanged; jobs without matching notes keep their previous batching even if ordinary context contains note-like text. Providers that do not consume context cannot apply character notes. The speaker workspace UI is a separate follow-up.

### Where Locust keeps data

Saved translations live in the project database described [above](#saving-progress). The desktop/HTTP project opener uses the game folder's parent when writable, with a config-directory `projects/` fallback. CLI `extract` without `-o` instead writes `<game-folder-name>.locust.db` in the current working directory; pass `-o` to choose the database explicitly.

The config directory contains `config.json` (including API keys), `xai-oauth.json` after Grok login, and the default `backups/` store:

| OS | Default config directory |
| -- | ------------------------ |
| Windows | `%LOCALAPPDATA%\project-locust` |
| macOS | `~/Library/Application Support/project-locust` |
| Linux | `$XDG_CONFIG_HOME/project-locust`, or `~/.config/project-locust` |

A nonempty `LOCUST_DATA_DIR` overrides this directory. If the OS directory cannot be resolved, Locust falls back to `.project-locust` relative to the working directory. The CLI also accepts `LOCUST_BACKUP_ROOT` to override its backup store.

Injection backups preserve the selected game's files **before injection**, not just the translations. New backups store metadata in `<backup-id>/manifest.json` and original content in `<backup-id>/payload/`. Plan disk space for a full backup, an independent working copy and transaction originals/results; forced recovery may need conflict copies too. Verified identical backup payloads can share disk storage, but this is not a fixed-size guarantee. See [insertion recovery and disk costs](docs/INJECTION-RECOVERY.md).

Desktop drafts are **unsaved edits**, separate from database translations. They use browser `localStorage` keys beginning with `locust.editor.draft.v1.` (one key per revision). The batch queue uses `locust.queue.v1` for items, translation settings and recent history. Interrupted queue work requires an explicit retry after restart. Clearing this browser storage removes those drafts and queue snapshots; it does not save drafts into the project database.

---

## After injection: QA checklist

1. Run the translated **game copy** and play through representative scenes.
2. Check fonts and missing glyphs. For a font-coverage audit against a UTF-8 text file containing the actual target text, run `locust font-check "<game-copy>" --text-file "<target-text.txt>"`. Coverage alone does not prove the game selects that font or renders it correctly; see [font checks and patches](docs/FONT-PATCH.md).
3. Look for clipped/overflowing text, broken line wrapping, menus and choices that no longer fit or work.
4. Test saving and loading, then continue playing from the loaded save.
5. Keep the original copy and backup until satisfied. Artifact validation does not certify gameplay, fonts or layout.

For a ZIP installed with `apply`, use `locust patch-rollback "<game-copy>"` to restore its `.locust/backup`. For an injection backup, list IDs with `locust backups`, preview with `locust restore-backup <id> --dry-run`, then restore with `locust restore-backup <id>` to its recorded game location. These are separate workflows; handle interrupted Direct/Add operations with the [insertion recovery commands](docs/INJECTION-RECOVERY.md) first.

---

## Troubleshooting

Messages below are literal diagnostics (the desktop may show localized guidance).

| Message | What to do |
| ------- | ---------- |
| `no project open` | Open a game or saved project database before editing/translating. |
| `translation changed since it was loaded` | Your save used an older version. Keep your draft, load the latest translation and compare; choose **Overwrite** only if you intend to replace it. |
| `cannot enforce a cost limit: this provider has no cost estimate` | Choose a provider with estimates or a free provider; remove the limit only if you accept unknown cost. |
| `cannot enforce a cost limit: observed calls have unknown cost` | The run stopped because usage is incomplete. Check provider usage before retrying; choose a provider with known cost to retain enforcement. |
| `Interrupted when the app closed. Retry to continue.` (batch queue) | Retry the interrupted queue item explicitly; restored work does not auto-start. |

---

## License

MIT. See [LICENSE](LICENSE).

---

## Contributing

For app users: [Safe native visual-novel workflow](docs/NATIVE-VN-WORKFLOW.md) — Japanese → English → Spanish, complete pivots, translated copies and encoding limits; no Python required.

Pull requests welcome. Start with the [architecture map](docs/ARCHITECTURE.md) and the [improvement ledger](docs/IMPROVEMENT-GOAL.md) (latest **Done** entry and backlog). See [CLAUDE.md](CLAUDE.md) for agent instructions and [RELEASE.md](RELEASE.md) for the release process.

Priority areas:
- NScripter NSA / nscript.___; KiriKiri cxdec/Hxv4; YU-RIS exotic YPF schemes
- Length-aware real-ES for binary engines (Unity / Unreal / Wolf)
- Deeper Unity parsers beyond heuristic scans; Unreal full pak index
- Real commercial Wolf RPG end-to-end proof

---

## Credits

Inspired by [Paloslios](https://f95zone.to/threads/free-renpy-translator-multicore.70107/) (Ren'Py), [Translator++](http://dreamsavior.net/) (commercial) and the rich ecosystem of game-translation tools built by the community.
