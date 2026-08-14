/**
 * Lightweight asserts for translationJob (run: npx --yes tsx src/lib/translationJob.test.ts).
 */
import assert from "node:assert/strict";
import {
  shouldReattachTranslationJob,
  shouldSubscribeToJob,
  snapshotIsTerminal,
  translationModalStep,
} from "./translationJob.ts";

assert.equal(shouldReattachTranslationJob(null, false), false);
assert.equal(shouldReattachTranslationJob("job-1", false), false);
assert.equal(shouldReattachTranslationJob(null, true), false);
assert.equal(shouldReattachTranslationJob("job-1", true), true);

assert.equal(shouldSubscribeToJob("job-1", null), true);
assert.equal(shouldSubscribeToJob("job-1", "job-1"), false);
assert.equal(shouldSubscribeToJob("job-2", "job-1"), true);

assert.equal(snapshotIsTerminal(null), false);
assert.equal(
  snapshotIsTerminal({
    done: false,
    cancelled: false,
    error: null,
  }),
  false,
);
assert.equal(
  snapshotIsTerminal({ done: true, cancelled: false, error: null }),
  true,
);
assert.equal(
  snapshotIsTerminal({ done: false, cancelled: true, error: null }),
  true,
);
assert.equal(
  snapshotIsTerminal({
    done: false,
    cancelled: false,
    error: "lost",
  }),
  true,
);

assert.equal(
  translationModalStep({ isTranslating: false, snapshot: null }),
  "configure",
);
assert.equal(
  translationModalStep({
    isTranslating: true,
    snapshot: { done: false, cancelled: false, error: null },
  }),
  "progress",
);
assert.equal(
  translationModalStep({
    isTranslating: false,
    snapshot: { done: true, cancelled: false, error: null },
  }),
  "progress",
);
assert.equal(
  translationModalStep({
    isTranslating: false,
    snapshot: { done: false, cancelled: false, error: "boom" },
  }),
  "progress",
);

console.log("translationJob.test.ts: ok");
