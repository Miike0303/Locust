import assert from "node:assert/strict";
import { test } from "node:test";
import { canRestoreRevision, historyStatusLabel } from "./stringHistory";
import { setLocale, t } from "./i18n";

test("restore eligibility distinguishes null, empty text and the current value", () => {
  assert.equal(canRestoreRevision(null, "current"), false);
  assert.equal(canRestoreRevision("current", "current"), false);
  assert.equal(canRestoreRevision("old", "current"), true);
  assert.equal(canRestoreRevision("", "current"), true);
  assert.equal(canRestoreRevision("", null), true);
  assert.equal(canRestoreRevision(null, null), false);
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
