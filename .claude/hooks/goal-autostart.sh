#!/usr/bin/env bash
# SessionStart (startup) hook: tell Claude to resume the Locust /goal loop.
# Remove this hook from .claude/settings.local.json to stop the auto-resume.
cat <<'EOF'
{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"AUTO-RESUME (user request, 2026-09-27): as your FIRST action in this session, whatever the user's first message is, invoke the Skill tool with skill \"loop\" and args \"/goal Locust on main. Writers: Codex only (no Cursor), gpt-6-astra xhigh; researchers gpt-6-sol high. NO fast tier. Ticks every 15-20 min. One Codex session at a time while other fleets run. Continue until Codex quota and resets exhausted or user stops.\". Before the first cycle read docs/HANDOFF-2026-09-27.md and .claude/improvement.md. Skip this only if the user's first message explicitly says not to resume the goal loop."},"systemMessage":"Locust: auto-resuming /loop /goal (see docs/HANDOFF-2026-09-27.md)"}
EOF
