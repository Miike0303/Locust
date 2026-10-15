@echo off
rem Opens Claude Code in Locust and starts the /goal loop immediately.
cd /d C:\Projects\Locust
claude "/loop /goal Locust on main. Writers: Codex first, gpt-6.1-sol xhigh; researchers gpt-6.1-sol high (no gpt-6-astra, no gpt-6-sol 6.0). Codex is the only external writer/researcher (owner 2026-10-09, no Cursor): always dispatch it even if quota reads 0 percent or exhausted; if Codex errors or stalls, retry it, and after 3 failed Codex rounds Claude implements directly. NO fast tier. Ticks every 20 min. One Codex session at a time while other fleets run. Claude double-verifies every writer result and fixes it directly if wrong. Continue until the user stops."
