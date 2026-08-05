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
Progress is saved in the project database. Prefer **patch → apply** on a *copy* of the game
(never the only original without backup).

---

## Supported game engines

Stability matches `locust formats` / the desktop Welcome screen
(`stable` | `experimental` | `coming soon`).

### Stable

| Engine                    | Extensions            | Notes |
| ------------------------- | --------------------- | ----- |
| **RPG Maker MV / MZ**     | `.json`               | Maps, events, system, plugins; shared `rpgmaker-mv` plugin |
| **RPG Maker XP / VX Ace** | `.rvdata2`, `.rxdata` | Ruby Marshal data |
| **Ren'Py**                | `.rpy`, `.rpa`        | Loose scripts + RPA; Add-mode `game/tl/<lang>/` language packs |

### Experimental (Phase-2 extract → inject → patch → apply proven on fixtures or sample games)

| Engine                    | Extensions       | Notes |
| ------------------------- | ---------------- | ----- |
| **SugarCube / Twine**     | `.html`, `.htm`  | Interactive fiction |
| **HTML (generic)**        | `.html`, `.htm`  | Non-SugarCube HTML adventures |
| **Unity**                 | `.assets`        | Binary length-limited inject (UTF-8 ≤ source) |
| **Unreal Engine**         | `.pak`           | UTF-16LE heuristic; length ≤ source |
| **Wolf RPG Editor**       | `.wolf`          | Shift-JIS binary; length ≤ source |
| **VNTextPatch JSON**      | `.json`          | VN intermediate (KiriKiri/YU-RIS/… via VNTextPatch); Phase-2 patch apply proven on fixture |
| **QSP**                   | `.qsp`, `.gam`   | QuestSoft Player; synthetic fixture only |
| **KiriKiri / KAG**        | `.ks`            | Loose scripts (UTF-16/UTF-8/SJIS; FE FE 0/1); XP3 not yet |
| **YU-RIS**                | `.ybn`           | Loose YSTB (XOR + Shift-JIS); YPF not yet; synthetic fixtures |

### Coming soon

- **Japanese light novel engines** — NScripter, TyranoBuilder

Not seeing your engine? [Open an issue](https://github.com/Miike0303/Locust/issues).

---

## Supported translation providers

Always available in the CLI (`locust providers`):

- **Google Translate** (free web endpoint) — no API key
- **Argos Translate** — free offline
- **Ollama** / **LM Studio** — free local models
- **Grok SuperGrok** (`grok-sub`) — subscription OAuth (`locust auth`)
- **Mock** — length-safe for binary-engine inject tests

When API keys are configured: **DeepL**, **OpenAI-compatible**, **Anthropic Claude**, etc.

---

## Install

### Pre-built binaries (recommended)

Download the latest release for your OS from the [Releases page](https://github.com/Miike0303/Locust/releases).

- **Windows**: `Project.Locust_<ver>_x64_en-US.msi` — run the installer
- **macOS**: `Project.Locust_<ver>_<arch>.dmg` — open and drag to Applications
- **Linux**: `project-locust_<ver>_amd64.AppImage` — `chmod +x` then run

The app checks for updates on launch and offers to install them one-click.

### Build from source

Requires Rust 1.75+ and Node.js 20+.

```bash
git clone https://github.com/Miike0303/Locust.git
cd Locust
npm --prefix apps/desktop ci
cargo build --release -p locust-desktop
# Launch: target/release/locust-desktop(.exe)
```

---

## Quick start

1. **Launch the app** and click *Open Game Folder* (or *Open Game File*).
2. Pick the game's directory (the one containing `Game.exe`, `index.html`, etc.).
3. Locust auto-detects the format and extracts all translatable strings.
4. Click **Translate** in the editor toolbar, choose a provider and target language (Spanish by default).
5. Watch the progress — translations are saved to the database as they come in.
6. Edit in the desktop editor as needed:
   - **Validate** (Ctrl+Shift+V) — placeholders + binary inject-slot length (Unity/Unreal/Wolf).
   - **Search & replace** (Ctrl+Shift+F) — bulk edit translations only (one DB batch).
   - **Export / Import** (Ctrl+E) — PO or XLIFF for external CAT tools (ids survive round-trip).
7. Click **Inject**, pick language(s), and choose:
   - **Add mode** *(where supported, e.g. Ren'Py)* — language packs beside the original tree.
   - **Replace mode** — writes into a work copy (always keep a pristine original).
8. Package and apply a patch (recommended distribution path):
   - Desktop: Editor → **Patch** (Ctrl+Shift+P), or CLI below.
9. Play your translated game.

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

Got suspicious strings? Open the Editor, filter by tag (`dialogue`, `ui_label`, `menu`, etc.), and edit or approve translations manually before injecting.

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
- Backups (before each inject, so you can roll back)

You can close Locust and reopen the same project later — your progress persists. Use the **Review** tab to audit translations before finalizing.

---

## License

MIT. See [LICENSE](LICENSE).

---

## Contributing

Pull requests welcome. See [CLAUDE.md](CLAUDE.md) for the project architecture overview and [RELEASE.md](RELEASE.md) for the release process.

Priority areas:
- Japanese light-novel engines (NScripter / TyranoBuilder); KiriKiri XP3 + mode-2; YU-RIS YPF
- Length-aware real-ES for binary engines (Unity / Unreal / Wolf)
- Deeper Unreal/Unity parsers beyond heuristic scans
- Real commercial Wolf RPG end-to-end proof

---

## Credits

Inspired by [Paloslios](https://f95zone.to/threads/free-renpy-translator-multicore.70107/) (Ren'Py), [Translator++](http://dreamsavior.net/) (commercial) and the rich ecosystem of game-translation tools built by the community.
