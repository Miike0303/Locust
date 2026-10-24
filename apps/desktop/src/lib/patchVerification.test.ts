import assert from "node:assert/strict";
import { test } from "node:test";
import type { PatchVerifyResult } from "./api";
import { translate } from "./i18n";
import { en } from "./i18n/en";
import { es } from "./i18n/es";
import { patchVerificationSummary } from "./patchVerification";

function report(overrides: Partial<PatchVerifyResult> = {}): PatchVerifyResult {
  return {
    outcome: "Clean", tier: "Strict", replaced: [], added: [], conflicts: [],
    backup_compromised: false, messages: [], manifest: null, ...overrides,
  };
}
function mismatch(path: string, expected: string, found: string): string {
  return `FileMismatch { path: ${JSON.stringify(path)}, expected: ${JSON.stringify(expected)}, found: ${JSON.stringify(found)} }`;
}

for (const outcome of ["Clean", "AlreadyApplied", "UpgradeAvailable"]) {
  test(`${outcome} without conflicts or backup damage is successful`, () => {
    const display = patchVerificationSummary(report({ outcome }));
    assert.equal(display.severity, "success");
    assert.deepEqual(display.conflicts, []);
    assert.notEqual(translate(en, "en", display.summaryKey, display.params), display.summaryKey);
  });
}

test("conflicts warn and count unique paths across the outcome and conflict list", () => {
  const path = "data/one.txt";
  const display = patchVerificationSummary(report({
    outcome: `Mismatch([${mismatch(path, "absent (added path)", "present")}])`,
    conflicts: [path, path, "data/two.txt"],
  }));
  assert.equal(display.severity, "warning");
  assert.equal(display.summaryKey, "patch.verify.conflicts");
  assert.deepEqual(display.params, { count: 2 });
  assert.deepEqual(display.conflicts, [
    { path, reasonKey: "patch.verify.reason.exists" },
    { path: "data/two.txt", reasonKey: "patch.verify.reason.exists" },
  ]);
});

for (const [locale, catalog, singular, plural] of [
  ["en", en, "Verification found 1 conflicting file.", "Verification found 2 conflicting files."],
  ["es", es, "La verificación encontró 1 archivo en conflicto.", "La verificación encontró 2 archivos en conflicto."],
] as const) {
  test(`${locale} conflict counts use the existing plural helper`, () => {
    for (const [conflicts, expected] of [[["a"], singular], [["a", "b", "a"], plural]] as const) {
      const display = patchVerificationSummary(report({ conflicts: [...conflicts] }));
      assert.equal(translate(catalog, locale, display.summaryKey, display.params), expected);
    }
  });
}

for (const outcome of ["Unknown", "NewBackendOutcome", "Mismatch([])", "Mismatch(malformed)"]) {
  test(`${outcome} is never a success`, () => {
    assert.equal(patchVerificationSummary(report({ outcome })).severity, "warning");
  });
}

test("downgrade is a warning and interrupted or compromised backups are errors", () => {
  for (const [overrides, severity, summaryKey] of [
    [{ outcome: 'DowngradeBlocked { installed: "2", incoming: "1" }' }, "warning", "patch.verify.downgrade"],
    [{ outcome: "Interrupted", conflicts: ["a"] }, "error", "patch.verify.interrupted"],
    [{ backup_compromised: true, conflicts: ["a"] }, "error", "patch.verify.backupCompromised"],
  ] as const) {
    const display = patchVerificationSummary(report({ ...overrides, conflicts: "conflicts" in overrides ? [...overrides.conflicts] : [] }));
    assert.equal(display.severity, severity);
    assert.equal(display.summaryKey, summaryKey);
  }
});

test("known per-file reason sentinels are localized; unfamiliar diagnostics use a generic label", () => {
  const outcome = `Mismatch([${[
    mismatch("exists", "absent (added path)", "present with unexpected content"),
    mismatch("missing", "present with original=abc", "absent"),
    mismatch("changed", "original=abc or patched=def", "new-hash"),
    mismatch("future", "new expectation code", "new reason code"),
  ].join(", ")}])`;
  const display = patchVerificationSummary(report({ outcome, messages: ["raw <diagnostic>", "another detail"] }));
  assert.deepEqual(display.conflicts, [
    { path: "exists", reasonKey: "patch.verify.reason.exists" },
    { path: "missing", reasonKey: "patch.verify.reason.missing" },
    { path: "changed", reasonKey: "patch.verify.reason.changed" },
    { path: "future", reasonKey: "patch.verify.reason.unknown" },
  ]);
  assert.deepEqual(display.technical, [outcome, "raw <diagnostic>", "another detail"]);
  for (const catalog of [en, es]) for (const { reasonKey } of display.conflicts) {
    assert.notEqual(translate(catalog, "en", reasonKey), reasonKey);
  }
});

test("Rust Debug escaped paths are decoded and invalid escapes cannot crash verification", () => {
  const outcome = String.raw`Mismatch([FileMismatch { path: "data/quote\"\\\u{1f600}\0.txt", expected: "present", found: "absent" }, FileMismatch { path: "\u{ffffffff}", expected: "present", found: "absent" }])`;
  const display = patchVerificationSummary(report({ outcome }));
  assert.deepEqual(display.conflicts, [{ path: 'data/quote"\\😀\0.txt', reasonKey: "patch.verify.reason.missing" }]);
  assert.deepEqual(display.technical, [outcome]);
});

test("literal backslashes in paths are not reinterpreted as Rust escape codes", () => {
  const path = String.raw`data/\u{1f600}\0.txt`;
  const outcome = `Mismatch([${mismatch(path, "present", "absent")}])`;
  assert.deepEqual(patchVerificationSummary(report({ outcome })).conflicts, [
    { path, reasonKey: "patch.verify.reason.missing" },
  ]);
});
