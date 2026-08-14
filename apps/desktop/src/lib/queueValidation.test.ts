/**
 * Lightweight asserts for queueValidation
 * (run: npx --yes tsx src/lib/queueValidation.test.ts).
 */
import assert from "node:assert/strict";
import {
  queueItemPatchAfterValidation,
  validationIssueCount,
} from "./queueValidation.ts";

assert.equal(validationIssueCount(null), 0);
assert.equal(validationIssueCount(undefined), 0);
assert.equal(validationIssueCount({}), 0);
assert.equal(validationIssueCount({ validation: null }), 0);
assert.equal(validationIssueCount({ validation: {} }), 0);
assert.equal(validationIssueCount({ validation: { issues_found: 0 } }), 0);
assert.equal(validationIssueCount({ validation: { issues_found: 4 } }), 4);
assert.equal(validationIssueCount({ validation: { issues_found: -1 } }), 0);

assert.deepEqual(queueItemPatchAfterValidation({ ok: true, issuesFound: 0 }), {
  status: "done",
  validationIssues: 0,
  validationError: null,
});
assert.deepEqual(queueItemPatchAfterValidation({ ok: true, issuesFound: 3 }), {
  status: "done",
  validationIssues: 3,
  validationError: null,
});
assert.deepEqual(
  queueItemPatchAfterValidation({ ok: false, error: "timeout" }),
  {
    status: "done",
    validationIssues: null,
    validationError: "timeout",
  },
);

console.log("queueValidation.test.ts: ok");
