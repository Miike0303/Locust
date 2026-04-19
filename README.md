# Project Locust

**LOC**alization **U**niversal **S**cripting **T**ool — a free, open-source desktop app that translates video games across engines, providers, and languages. Runs on Windows, macOS, and Linux.

![License](https://img.shields.io/badge/license-MIT-green)
![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-blue)
![Status](https://img.shields.io/badge/status-alpha-orange)

---

## What it does

Locust extracts translatable text from game files, runs it through a translation provider (free or paid), and injects the translations back — all while **preserving variables, formatting tags, and code** so the game keeps working.

```
  [ Game files ]  →  extract  →  [ SQLite DB ]  →  translate  →  [ translations ]  →  inject  →  [ Translated game ]
```

You can stop at any stage, review/edit translations in the built-in editor, then continue.
Your progress is saved automatically in the project database so you can revisit it later.

---

## Supported game engines

### ✅ Available now

| Engine                    | Extensions       | Notes                                           |
| ------------------------- | ---------------- | ----------------------------------------------- |
| **RPG Maker MV / MZ**     | `.json`          | Maps, Events, Items, Actors, Troops, System, plugins |
| **RPG Maker XP / VX Ace** | `.rvdata2`, `.rxdata` | Ruby-marshal data files                    |
| **Ren'Py**                | `.rpy`, `.rpa`   | Proper `translate <lang> <label>_<hash>:` blocks, in-game language picker |

### 🚧 Coming soon

- **Unity** — `.assets` / Il2Cpp VN script extraction
- **Unreal Engine** — `.pak` asset extraction
- **HTML / Twine / SugarCube** — interactive fiction
- **QSP** — Russian-style text adventures
- **Japanese light novel engines** — KiriKiri, NScripter, Yuris, TyranoBuilder

Not seeing your engine? [Open an issue](https://github.com/Miike0303/Locust/issues).

---

## Supported translation providers

- **Google Translate** (free web endpoint) — no API key
- **DeepL** — paid API
- **OpenAI (GPT-4 / GPT-3.5)** — paid API
- **Anthropic Claude** — paid API
- **Ollama** — free local models
- **Argos Translate** — free offline
- **Mock** — for testing

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
6. When done, click **Inject**, pick language(s), and choose:
   - **Add mode** *(recommended)* — writes standard Ren'Py translation files to `game/tl/<lang>/`, leaves the original game untouched. Includes an in-game language picker.
   - **Replace mode** — copies the game to a new folder and modifies it in place.
7. Play your translated game.

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
- Unity .assets extractor (VN-style games, not full engine)
- HTML/Twine handling
- More providers (Anthropic, local Llama via Ollama)
- Better handling of string interpolation across languages

---

## Credits

Inspired by [Paloslios](https://f95zone.to/threads/free-renpy-translator-multicore.70107/) (Ren'Py), [Translator++](http://dreamsavior.net/) (commercial) and the rich ecosystem of game-translation tools built by the community.
