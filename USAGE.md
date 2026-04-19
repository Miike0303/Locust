# Project Locust — User Guide

A short walkthrough for translating your first game. No coding needed.

---

## 1. Download and install

1. Go to the [Releases page](https://github.com/Miike0303/Locust/releases).
2. Download the installer for your OS:
   - **Windows**: `Project.Locust_<ver>_x64_en-US.msi`
   - **macOS**: `Project.Locust_<ver>_<arch>.dmg`
   - **Linux**: `project-locust_<ver>_amd64.AppImage`
3. Run the installer. Launch *Project Locust* from your start menu / applications.

---

## 2. Open a game

1. On the welcome screen, click **Open Game Folder**.
2. Pick the folder that contains `Game.exe` (or `index.html`, `.rpy` scripts, etc.).
3. Locust auto-detects the engine:
   - If you see a badge next to your engine name → supported.
   - If not detected, choose the format manually from the dropdown.
4. Click **Open Project**. Extraction runs automatically (usually 1–10 seconds).

**Supported right now**: RPG Maker MV/MZ/XP/VX Ace, Ren'Py.
Unity, Unreal, HTML, QSP and others are marked *Coming soon* — you'll see them in the list but can't open them yet.

---

## 3. Translate

1. In the editor, click the green **Translate** button in the toolbar.
2. Pick a **Provider**:
   - **Google** *(free, no API key)* — good quality, no cost, rate-limited.
   - **OpenAI / Claude / DeepL** — paid, higher quality. Set API keys in Settings first.
   - **Ollama** — free, runs offline. Needs [Ollama](https://ollama.com) installed locally.
3. Pick **Source** (or leave on *Auto-detect*) and **Target** language.
   Default: source = Auto, target = Español.
4. Optional: add **Game context** (e.g. "dark fantasy RPG, medieval") — helps the AI pick the right tone.
5. Click **Start Translation**.

You can close the modal. Translations continue in the background. Progress shows in the bottom-right corner.

### Translation tips

- **Start with a small batch** (set *Batch size* = 10) to check the output quality before committing to a full run.
- **Set a cost limit** if using paid providers — Locust stops before exceeding it.
- **Use glossary** to keep character names and proper nouns consistent.
- **Use memory** to reuse translations from previous projects (saves cost).

---

## 4. Review and edit

Your translations go into a database file next to your project. **You can safely close the app and reopen the project later — progress is saved.**

In the Editor:

- Click any row to edit a translation by hand.
- Use **Status** filters (Pending, Translated, Approved) to focus on what needs work.
- Mark rows as **Approved** once you're happy with them.
- Protected patterns like `[player_name]`, `{i}...{/i}`, `\n` are kept as-is automatically — you don't need to worry about breaking them.

---

## 5. Inject

When you're ready to play the translated game:

1. Click the **Inject** button.
2. Pick one or more target languages (checkboxes).
3. Pick a mode:

### Mode: Add (recommended)

- Writes translation files into `<game>/game/tl/<lang>/`.
- Adds a floating **🌐 Language** button to the game's main menu.
- **Original game files are untouched** — you can toggle between languages in-game.
- Best for casual testing or when you want multiple languages.

### Mode: Replace

- Copies the game to a new folder, e.g. `<game>-es/`, with translations applied.
- Modifies dialogue `.rpy` / `.json` files in the copy.
- Use when you want a standalone translated build.

Injection is fast (seconds even for large games). Then launch the game — translations should appear.

---

## 6. Keep improving translations

Incremental workflow:

1. Play the game briefly, spot mistranslations or awkward phrasing.
2. Open the project in Locust again (it's in your Recent Projects list).
3. Edit or re-translate the bad rows in the editor.
4. Click Inject again — changes propagate.

Repeat until happy. The project file keeps all your work.

---

## 7. Updates

Locust checks for updates each time you launch it. When a new version is available, you'll see a green *Update available* banner in the bottom-right. Click **Download & install** and the app restarts on the new version automatically.

---

## FAQ

**Q: Translations don't appear in the game.**
A: Usually one of:
- The game caches compiled `.rpyc` — Locust deletes these automatically on inject. Try re-injecting.
- Wrong target language — the game may not be configured to show your language by default. The Add-mode language picker button fixes this.
- The game was built from an archive (`.rpa`) — Locust extracts these transparently but some edge cases may fail. Report the game name in an issue.

**Q: The game crashes after injection.**
A: Rare. Usually caused by a translation provider returning malformed output (unbalanced quotes, broken tags). Check the Review tab — look for rows flagged with a warning icon — and fix them manually. Use the Inject again.

**Q: Can I translate paid / commercial games?**
A: Locust doesn't care — it's a tool. Legality depends on the game's EULA and your jurisdiction. Personal use is generally safe; redistributing modified commercial games is usually not.

**Q: Does Locust support my game engine X?**
A: Check the welcome screen format list. If X is marked "Coming soon", it's on the roadmap. File an issue with your game info to help prioritize.

**Q: Where's the project file saved?**
A: By default, `<game-folder>/../<game-name>.locust.db`. Shown under Recent Projects on the welcome screen.

---

## Getting help

- [GitHub Issues](https://github.com/Miike0303/Locust/issues) — bug reports, feature requests
- [Discussions](https://github.com/Miike0303/Locust/discussions) — usage questions, tips
