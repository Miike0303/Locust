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

Only need to **apply** someone else's patch zip, not translate? Skip the desktop app and grab the CLI binary from the same release (`locust-x86_64-windows.exe`, `locust-aarch64-macos`, `locust-x86_64-macos`, or `locust-x86_64-linux`). See [Applying a patch](#7-applying-a-patch).

The desktop menus themselves can be **English or Spanish**. That is the app's interface language, not the language you translate the game into. Change it in **Settings → Appearance → Interface language**.

---

## 2. Open a game

1. On the welcome screen, click **Open Game Folder**.
2. Pick the folder that contains `Game.exe` (or `index.html`, `.rpy` scripts, etc.).
3. Locust auto-detects the engine:
   - If you see a badge next to your engine name → supported.
   - If not detected, choose the format manually from the dropdown.
4. Click **Open Project**. Extraction runs automatically (usually 1–10 seconds).

**Stable**: RPG Maker MV/MZ, RPG Maker XP/VX Ace, Ren'Py.

**Experimental** (working extract → inject → patch; still evolving): Unity, Unreal, Wolf RPG, HTML / Twine (SugarCube), QSP, TyranoBuilder, KiriKiri, YU-RIS, NScripter, VNTextPatch JSON.

Nothing on that list is “coming soon.” Experimental means it works and you can open it; treat the result as something to playtest, not a finished localization pipeline.

Each game gets its **own** database file. Reopening the same folder **keeps** your translations and approvals — Locust merges new strings in; it does not wipe the project.

---

## 3. Translate

1. In the editor, click the green **Translate** button in the toolbar.
2. Pick a **Provider**:
   - **Google** *(free, no API key)* — good quality, no cost, rate-limited.
   - **OpenAI / Claude / DeepL** — paid, higher quality. Set API keys in Settings first.
   - **Ollama** — free, runs offline. Needs [Ollama](https://ollama.com) installed locally.
3. Pick **Source** (or leave on *Auto-detect*) and **Target** language.
   Default: source = Auto, target = Español.
   This target is the **game's** language, not the Locust interface language (that lives under Settings → Appearance).
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

Your work is saved in `<game-folder>/../<game-name>.locust.db` (the folder *next to* the game, not inside it). If that location is not writable, Locust falls back to its app data folder. **Close the app and reopen the project later — progress is saved.** Shown under Recent Projects on the welcome screen.

If you open the game again after an update:

- Existing translations, statuses, and which provider wrote them are kept.
- Lines whose *original* text changed are kept but marked **Pending**, so a stale translation cannot ship as approved.
- Strings that no longer exist in the game are removed.

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
3. Pick a mode — only modes that engine supports are shown:

### Mode: Add

Available for **Ren'Py** and **RPG Maker MV/MZ**. Adds a language pack beside the original files. Original game files stay playable.

- **Ren'Py**: writes `game/tl/<lang>/` and adds an in-game language picker on the main menu (and preferences). You can toggle languages inside the game.
- **RPG Maker MV/MZ**: writes extra language packs. That is **not** a floating language button. To list the language in the game's own menu (Iavra / VisuMZ / boot-map choices), check **After inject, also register language(s) in game UI** — or use **Register … in game UI** without injecting. Same as the CLI `locust register-lang`. Optional menu label (for example Español) when one language is selected.

### Mode: Replace

- Copies the game to a new folder, e.g. `<game>-es/`, with translations applied.
- Use when you want a standalone translated build. Keep a pristine original.

### Mode: Direct

- Writes into the game folder itself. A backup is created first.
- Default for engines that do not support Add (Unity, Unreal, KiriKiri, and others). Work on a **copy** of the game when you can.
- Direct inject is what Patch → Pack records from when you later build a shareable zip.

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

## 7. Applying a patch

Translators can pack a patch zip from the desktop (**Patch**, Ctrl+Shift+P) after a Direct inject.

Players who only want to apply that zip do not need the desktop app:

1. Download the CLI from the same [Releases page](https://github.com/Miike0303/Locust/releases) (`locust-x86_64-windows.exe` on Windows, `locust-aarch64-macos` / `locust-x86_64-macos` on macOS, `locust-x86_64-linux` on Linux).
2. Apply onto a **copy** of the game (never the only original):

```bash
locust apply "<game-copy>" game-es-patch.zip
```

Or download and apply in one step (http/https only; do not pass a local zip at the same time):

```bash
locust apply "<game-copy>" --url "https://example.com/game-es-patch.zip"
```

Check or undo:

```bash
locust patch-status "<game-copy>"
locust patch-rollback "<game-copy>"
```

Rollback restores the backup Locust stored under `<game>/.locust/backup/`.

---

## 8. Updates

Locust checks for updates each time you launch it. When a new version is available, you'll see a green *Update available* banner in the bottom-right. Click **Download & install** and the app restarts on the new version automatically.

---

## FAQ

**Q: Translations don't appear in the game.**
A: Usually one of:
- The game caches compiled `.rpyc` — Locust deletes these automatically on inject. Try re-injecting.
- Wrong target language — the game may not be configured to show your language by default. On **Ren'Py**, Add mode installs an in-game language picker. On **RPG Maker MV/MZ**, use **Register … in game UI** (`register-lang`) so the language appears in the game's own menu.
- The game was built from an archive (`.rpa`) — Locust extracts these transparently but some edge cases may fail. Report the game name in an issue.

**Q: The game crashes after injection.**
A: Rare. Usually caused by a translation provider returning malformed output (unbalanced quotes, broken tags). Check the Review tab — look for rows flagged with a warning icon — and fix them manually. Use Inject again.

**Q: Can I translate paid / commercial games?**
A: Locust doesn't care — it's a tool. Legality depends on the game's EULA and your jurisdiction. Personal use is generally safe; redistributing modified commercial games is usually not.

**Q: Does Locust support my game engine X?**
A: Check the welcome screen format list. Stable and Experimental engines can be opened. If X is not listed, file an issue with your game info to help prioritize.

**Q: Where's the project file saved?**
A: By default, `<game-folder>/../<game-name>.locust.db` — next to the game folder, not inside it. Shown under Recent Projects on the welcome screen. Reopening merges; it does not wipe your translations.

---

## Getting help

- [GitHub Issues](https://github.com/Miike0303/Locust/issues) — bug reports, feature requests
- [Discussions](https://github.com/Miike0303/Locust/discussions) — usage questions, tips
