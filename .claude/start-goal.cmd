@echo off
rem Opens Claude Code in Locust and starts the /goal loop immediately.
cd /d C:\Projects\Locust
claude "/loop /goal Locust on main. Writers: Codex first, gpt-6-astra xhigh; researchers gpt-6-astra high; never gpt-6-sol. If Codex is unavailable, fall back to Cursor grok-4.7-xhigh writers / grok-4.7-high researchers. NO fast tier. Ticks every 15-20 min. One Codex session at a time while other fleets run. Continue until Codex quota and resets exhausted or user stops."
