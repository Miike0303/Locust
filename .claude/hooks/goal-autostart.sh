#!/usr/bin/env bash
# SessionStart (startup) hook: orient the session. It no longer auto-resumes a loop.
# Remove this hook from .claude/settings.local.json to silence it.
cat <<'EOF'
{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"Locust session note (owner 2026-11-09): the Codex subscription was cancelled, so never dispatch Codex; Claude writes, researches and verifies directly. Do not auto-start a /loop or /goal cycle; do what the user asks. Current state lives in odd/TASKS.md and the newest ledger under odd/tasks/. Pushing verified units to origin/main is authorized (gates and CI first, never force)."}}
EOF
