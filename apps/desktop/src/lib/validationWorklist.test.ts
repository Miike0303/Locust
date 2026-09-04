/**
 * Run: npx --yes tsx src/lib/validationWorklist.test.ts
 */
import assert from "node:assert/strict";
import {
	canStartValidationWorklist,
	stepWorklistIndex,
	uniqueIssueEntryIds,
} from "./validationWorklist.ts";

assert.deepEqual(uniqueIssueEntryIds([]), []);
assert.deepEqual(
	uniqueIssueEntryIds([
		{ entry_id: "a" },
		{ entry_id: "b" },
		{ entry_id: "a" },
		{ entry_id: "c" },
	]),
	["a", "b", "c"],
);
// Negative: empty id must not invent a slot or collapse the list.
assert.deepEqual(
	uniqueIssueEntryIds([{ entry_id: "" }, { entry_id: "x" }, { entry_id: "" }]),
	["x"],
);

assert.equal(stepWorklistIndex(0, 3, 1), 1);
assert.equal(stepWorklistIndex(2, 3, 1), 2);
assert.equal(stepWorklistIndex(1, 3, -1), 0);
assert.equal(stepWorklistIndex(0, 3, -1), 0);
assert.equal(stepWorklistIndex(5, 0, 1), 0);

assert.equal(canStartValidationWorklist(0), false);
assert.equal(canStartValidationWorklist(1), true);
// Negative: forcing a zero-issue start would strand the Editor banner.
assert.equal(canStartValidationWorklist(0), false);

console.log("validationWorklist.test.ts: ok");
