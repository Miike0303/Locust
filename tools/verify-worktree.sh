#!/bin/bash
# Verify a writer's changes in a clean worktree of HEAD (fmt, strict clippy, workspace tests).
# Usage: tools/verify-worktree.sh <tag> <repo-relative-file>...
# Output: $TEMP/locust-<tag>-verify.out ends with a "DONE" line. Remove the worktree afterwards with:
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
git worktree add -f --detach "$w" HEAD >> "$out" 2>&1
for f in "$@"; do mkdir -p "$w/$(dirname "$f")"; cp "$f" "$w/$f"; done
mkdir -p "$w/apps/desktop"; cp -r apps/desktop/dist "$w/apps/desktop/dist"
cd "$w" || exit 1
echo "== fmt" >> "$out";    cargo fmt --all --check >> "$out" 2>&1;                                  echo "fmt exit $?" >> "$out"
echo "== clippy" >> "$out"; cargo clippy --workspace --all-targets -- -D warnings >> "$out" 2>&1;    echo "clippy exit $?" >> "$out"
echo "== test" >> "$out";   cargo test --workspace >> "$out" 2>&1;                                   echo "test exit $?" >> "$out"
echo "== summary" >> "$out"
rg "^test result" "$out" | awk '{p+=$4; f+=$6; i+=$8} END {print "passed",p,"failed",f,"ignored",i}' >> "$out"
echo DONE >> "$out"
