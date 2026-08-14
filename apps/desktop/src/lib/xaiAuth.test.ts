/**
 * Lightweight asserts for xaiAuth (run: npx --yes tsx src/lib/xaiAuth.test.ts).
 */
import assert from "node:assert/strict";
import {
  grokSubIsReady,
  nextXaiPollAction,
  XAI_POLL_INTERVAL_MS,
} from "./xaiAuth.ts";

assert.equal(XAI_POLL_INTERVAL_MS, 5_000);

assert.deepEqual(nextXaiPollAction("pending"), { action: "poll" });
assert.deepEqual(nextXaiPollAction("complete"), {
  action: "stop",
  outcome: "complete",
});
assert.deepEqual(nextXaiPollAction("denied"), {
  action: "stop",
  outcome: "denied",
});
assert.deepEqual(nextXaiPollAction("expired"), {
  action: "stop",
  outcome: "expired",
});

assert.deepEqual(
  nextXaiPollAction("pending", {
    startedAtMs: 1_000,
    expiresInSecs: 30,
    nowMs: 1_000 + 29_000,
  }),
  { action: "poll" },
);
assert.deepEqual(
  nextXaiPollAction("pending", {
    startedAtMs: 1_000,
    expiresInSecs: 30,
    nowMs: 1_000 + 30_000,
  }),
  { action: "stop", outcome: "expired" },
);
assert.deepEqual(
  nextXaiPollAction("complete", {
    startedAtMs: 1_000,
    expiresInSecs: 1,
    nowMs: 1_000 + 60_000,
  }),
  { action: "stop", outcome: "complete" },
);

assert.equal(grokSubIsReady(undefined), false);
assert.equal(grokSubIsReady([]), false);
assert.equal(grokSubIsReady([{ id: "mock", configured: true }]), false);
assert.equal(
  grokSubIsReady([{ id: "grok-sub", configured: false }]),
  false,
);
assert.equal(grokSubIsReady([{ id: "grok-sub" }]), true);
assert.equal(
  grokSubIsReady([{ id: "grok-sub", configured: true }]),
  true,
);

console.log("xaiAuth.test.ts: ok");
