#!/bin/bash
# Verify a writer's changes in a clean worktree of HEAD (fmt, strict clippy, workspace tests).
# Usage: tools/verify-worktree.sh <tag> <repo-relative-file>...
# Output: $TEMP/locust-<tag>-verify.out has RESULT PASS or RESULT FAIL
# (fmt=<n> clippy=<n> test=<n>) before the final DONE line; exit 0 on pass, 1 on failure.
# Worktree creation failure reports RESULT FAIL (worktree add failed), then DONE and exit 1.
# Remove the worktree afterwards with:
#   git worktree remove --force "$TEMP/locust-<tag>-verify"; rm -f "$TEMP/locust-<tag>-verify.out"
# One build at a time. Uses the shared cargo target, no debuginfo, no incremental.
tag="$1"; shift
if [ -z "$tag" ] || [ "$#" -eq 0 ]; then echo "usage: $0 <tag> <file>..."; exit 2; fi
export PATH="$PATH:/c/msys64/mingw64/bin:/c/Users/Mike/.cargo/bin"
export CARGO_TARGET_DIR="${LOCUST_SHARED_TARGET:-$LOCALAPPDATA/locust-shared-target}"
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0
repo="$(git rev-parse --show-toplevel)"
tmp="${TEMP:-/tmp}"
w="$tmp/locust-$tag-verify"
out="$tmp/locust-$tag-verify.out"
: > "$out"
cd "$repo" || exit 1
if ! git worktree add -f --detach "$w" HEAD >> "$out" 2>&1; then
  echo "RESULT FAIL (worktree add failed)" >> "$out"
  echo DONE >> "$out"
  exit 1
fi
for f in "$@"; do mkdir -p "$w/$(dirname "$f")"; cp "$f" "$w/$f"; done
mkdir -p "$w/apps/desktop"; cp -r apps/desktop/dist "$w/apps/desktop/dist"
cd "$w" || exit 1
echo "== fmt" >> "$out";    cargo fmt --all --check >> "$out" 2>&1;                               fmt_exit=$?; echo "fmt exit $fmt_exit" >> "$out"
echo "== clippy" >> "$out"; cargo clippy --workspace --all-targets -- -D warnings >> "$out" 2>&1; clippy_exit=$?; echo "clippy exit $clippy_exit" >> "$out"
echo "== test" >> "$out";   cargo test --workspace >> "$out" 2>&1;                                test_exit=$?; echo "test exit $test_exit" >> "$out"
echo "== summary" >> "$out"
rg "^test result" "$out" | awk '{p+=$4; f+=$6; i+=$8} END {print "passed",p,"failed",f,"ignored",i}' >> "$out"
result=0
if [ "$fmt_exit" -ne 0 ] || [ "$clippy_exit" -ne 0 ] || [ "$test_exit" -ne 0 ]; then
  echo "RESULT FAIL (fmt=$fmt_exit clippy=$clippy_exit test=$test_exit)" >> "$out"
  result=1
else
  echo "RESULT PASS" >> "$out"
fi
echo DONE >> "$out"
exit "$result"
