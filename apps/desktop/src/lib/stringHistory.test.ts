import assert from "node:assert/strict";
import { test } from "node:test";
import { canRestoreRevision, historyStatusLabel } from "./stringHistory";
import * as stringHistory from "./stringHistory";
import { setLocale, t } from "./i18n";

test("restore eligibility distinguishes null, empty text and the current value", () => {
  assert.equal(canRestoreRevision(null, "current"), false);
  assert.equal(canRestoreRevision("current", "current"), false);
  assert.equal(canRestoreRevision("old", "current"), true);
  assert.equal(canRestoreRevision("", "current"), true);
  assert.equal(canRestoreRevision("", null), true);
  assert.equal(canRestoreRevision(null, null), false);
});

test("before eligibility restores V1 when the only retained revision is V1 → V2 and V2 is current", () => {
  assert.equal(stringHistory.canRestoreBefore("V1", "V2"), true);
});

test("before eligibility preserves empty text as a restorable value, including over null", () => {
  assert.equal(stringHistory.canRestoreBefore("", "V2"), true);
  assert.equal(stringHistory.canRestoreBefore("", null), true);
});

test("before eligibility excludes null because there is no prior translation to restore", () => {
  assert.equal(stringHistory.canRestoreBefore(null, "V2"), false);
  assert.equal(stringHistory.canRestoreBefore(null, null), false);
});

test("before eligibility hides old text equal to the current value, including empty text", () => {
  assert.equal(stringHistory.canRestoreBefore("V1", "V1"), false);
  assert.equal(stringHistory.canRestoreBefore("", ""), false);
});

for (const [locale, labels] of [
  ["en", ["Pending", "Translated", "Reviewed", "Approved", "Error"]],
  ["es", ["Pendiente", "Traducida", "Revisada", "Aprobada", "Error"]],
] as const) {
  test(`${locale}: history uses the existing status labels and preserves unknown values`, () => {
    setLocale(locale);
    assert.deepEqual(["pending", "translated", "reviewed", "approved", "error"].map(status => historyStatusLabel(status, t)), labels);
    assert.equal(historyStatusLabel("future-status", t), "future-status");
    assert.equal(historyStatusLabel(null, t), "—");
  });
}
