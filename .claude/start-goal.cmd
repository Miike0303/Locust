@echo off
rem Opens Claude Code in Locust and starts the /goal loop immediately.
cd /d C:\Projects\Locust
claude "/loop /goal Locust on main. Writers: Codex only (no Cursor), gpt-6-astra xhigh; researchers gpt-6-sol high. NO fast tier. Ticks every 15-20 min. One Codex session at a time while other fleets run. Continue until Codex quota and resets exhausted or user stops."
