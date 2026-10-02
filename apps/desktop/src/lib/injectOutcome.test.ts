import assert from "node:assert/strict";
import type { InjectReportLike } from "./injectOutcome.ts";
import {
  classifyInjectReport,
  collectFilesWritten,
  collectInjectWarnings,
  injectionRecoveryPath,
  isRedundantBackupRemoved,
  collectSkipReasons,
  injectToastLevel,
  outcomeRecordingIssues,
  shouldOfferPackAfterInject,
  sumFilesModified,
  sumStringsSkipped,
  sumStringsWritten,
} from "./injectOutcome.ts";

// --- sumStringsWritten ---
assert.equal(sumStringsWritten({ strings_written: 12 }), 12);
assert.equal(sumStringsWritten({ strings_written: 0 }), 0);
assert.equal(
  sumStringsWritten({
    reports: { es: { strings_written: 3 }, fr: { strings_written: 2 } },
  }),
  5,
);
assert.equal(sumStringsWritten({ reports: { es: { strings_written: 0 } } }), 0);

// --- sumFilesModified / sumStringsSkipped: Add/Replace reports carry counts only per language ---
assert.equal(sumFilesModified({ files_modified: 4 }), 4);
assert.equal(
  sumFilesModified({ reports: { es: { files_modified: 3 }, fr: { files_modified: 1 } } }),
  4,
);
assert.equal(sumFilesModified({}), 0);
assert.equal(sumStringsSkipped({ strings_skipped: 2 }), 2);
assert.equal(
  sumStringsSkipped({ reports: { es: { strings_skipped: 5 }, fr: { strings_skipped: 1 } } }),
  6,
);

// --- skip reasons: exact counts, language scope, and legacy remainder ---
assert.deepEqual(collectSkipReasons({}), []);
assert.deepEqual(collectSkipReasons({ strings_skipped: 0 }), []);
assert.deepEqual(
  collectSkipReasons({
    strings_skipped: 5,
    skip_reasons: { unchanged: 2, too_long: 1 },
  }),
  [
    { lang: "", reason: "unchanged", count: 2 },
    { lang: "", reason: "too_long", count: 1 },
    { lang: "", reason: "unclassified", count: 2 },
  ],
);
// Historical skips are explicitly unclassified, never assumed too long.
assert.deepEqual(collectSkipReasons({ strings_skipped: 7 }), [
  { lang: "", reason: "unclassified", count: 7 },
]);
assert.deepEqual(
  collectSkipReasons({ strings_skipped: 2, reports: {} }),
  [{ lang: "", reason: "unclassified", count: 2 }],
);
// A direct response may repeat the same counts in its per-language report.
// Detailed reports take precedence so each skipped entry is counted once.
assert.deepEqual(
  collectSkipReasons({
    strings_skipped: 4,
    skip_reasons: { too_long: 4 },
    reports: {
      es: { strings_skipped: 4, skip_reasons: { too_long: 4 } },
      fr: { strings_skipped: 2, skip_reasons: { source_changed: 1 } },
      de: undefined,
    },
  }),
  [
    { lang: "es", reason: "too_long", count: 4 },
    { lang: "fr", reason: "source_changed", count: 1 },
    { lang: "fr", reason: "unclassified", count: 1 },
  ],
);
// Invalid counters cannot inflate the total or make negative rows. Fractional
// legacy values are truncated and reason counts cannot exceed strings_skipped.
assert.deepEqual(
  collectSkipReasons({
    strings_skipped: 3.9,
    skip_reasons: {
      negative: -2,
      nan: Number.NaN,
      infinite: Number.POSITIVE_INFINITY,
      zero: 0,
      unchanged: 1.8,
      too_long: 20,
      source_changed: 1,
    },
  }),
  [
    { lang: "", reason: "unchanged", count: 1 },
    { lang: "", reason: "too_long", count: 2 },
  ],
);
for (const invalidTotal of [-1, Number.NaN, Number.POSITIVE_INFINITY]) {
  assert.deepEqual(
    collectSkipReasons({ strings_skipped: invalidTotal, skip_reasons: { too_long: 2 } }),
    [],
  );
}

// Benign skips leave a successful write successful. Actionable skips, legacy
// unknown reasons, and future backend reasons must downgrade it to partial.
for (const reason of ["unchanged", "duplicate", "untranslated"]) {
  assert.equal(
    classifyInjectReport({
      strings_written: 3,
      strings_skipped: 1,
      skip_reasons: { [reason]: 1 },
    }),
    "success",
  );
}
for (const reason of ["too_long", "target_missing", "source_changed", "invalid_placeholders", "unclassified", "future_reason"]) {
  assert.equal(
    classifyInjectReport({
      strings_written: 3,
      strings_skipped: 1,
      skip_reasons: { [reason]: 1 },
    }),
    "partial",
  );
}
assert.equal(classifyInjectReport({ strings_written: 3, strings_skipped: 1 }), "partial");
assert.equal(
  classifyInjectReport({
    strings_written: 0,
    strings_skipped: 1,
    skip_reasons: { unchanged: 1 },
  }),
  "empty",
);

// --- classify: empty ---
assert.equal(
  classifyInjectReport({
    languages_processed: [],
    languages_failed: [["es", "boom"]],
    strings_written: 0,
  }),
  "empty",
);
assert.equal(
  classifyInjectReport({
    languages_processed: ["es"],
    languages_failed: [],
    strings_written: 0,
  }),
  "empty",
);
assert.equal(
  classifyInjectReport({
    languages_processed: ["es"],
    languages_failed: [],
    reports: { es: { strings_written: 0 } },
  }),
  "empty",
);

// --- classify: partial ---
assert.equal(
  classifyInjectReport({
    languages_processed: ["es"],
    languages_failed: [["fr", "nope"]],
    strings_written: 10,
  }),
  "partial",
);
assert.equal(
  classifyInjectReport({
    languages_processed: ["es"],
    languages_failed: [["fr", "nope"]],
    strings_written: 0,
  }),
  "partial",
);
assert.equal(
  classifyInjectReport({
    languages_processed: ["es"],
    languages_failed: [],
    strings_written: 4,
    outcomes: [["es", "NothingRecorded"]],
  }),
  "partial",
);
assert.equal(
  classifyInjectReport({
    languages_processed: ["es"],
    languages_failed: [],
    strings_written: 4,
    outcomes: [["es", { KeptPrevious: { recorded_at: "2026-01-01" } }]],
  }),
  "partial",
);

// --- classify: success ---
assert.equal(
  classifyInjectReport({
    languages_processed: ["es"],
    languages_failed: [],
    strings_written: 9,
    outcomes: [["es", { Recorded: { files: 2 } }]],
  }),
  "success",
);
assert.equal(
  classifyInjectReport({
    languages_processed: ["es", "fr"],
    languages_failed: [],
    reports: { es: { strings_written: 1 }, fr: { strings_written: 2 } },
  }),
  "success",
);

// --- toast level ---
assert.equal(injectToastLevel("success"), "success");
assert.equal(injectToastLevel("partial"), "warning");
assert.equal(injectToastLevel("empty"), "error");

// --- warnings / files ---
const retained = "injection 728ec773-0fcc-431a-aa1c-00c2b6e395db completed; verified originals retained at C:/game/.locust-injections/originals";
assert.deepEqual(collectInjectWarnings({
  warnings: [retained, retained],
  reports: { es: { warnings: [retained, "cannot re-encode", "cannot re-encode"] }, fr: { warnings: ["cannot re-encode"] } },
}), [retained, "es: cannot re-encode", "fr: cannot re-encode"]);
assert.equal(injectionRecoveryPath(retained), "C:/game/.locust-injections/originals");
assert.equal(injectionRecoveryPath(`zh-CN: ${retained}`), "C:/game/.locust-injections/originals");
assert.equal(injectionRecoveryPath("cannot re-encode"), null);
assert.equal(injectionRecoveryPath(retained.replace("completed;", "failed;")), null);
assert.deepEqual(
  collectInjectWarnings({
    warnings: ["top"],
    reports: { es: { warnings: ["cannot re-encode"] } },
  }),
  ["top", "es: cannot re-encode"],
);
assert.deepEqual(
  collectFilesWritten({
    files_written: ["a.pak", "a.pak"],
    reports: { es: { files_written: ["b.rpy"] } },
  }),
  ["a.pak", "b.rpy"],
);

// --- pack CTA ---
assert.equal(
  shouldOfferPackAfterInject({
    languages_processed: ["es"],
    languages_failed: [],
    strings_written: 0,
  }),
  false,
);
assert.equal(
  shouldOfferPackAfterInject({
    languages_processed: ["es"],
    languages_failed: [],
    strings_written: 3,
    outcomes: [["es", "NothingRecorded"]],
  }),
  false,
);
assert.equal(
  shouldOfferPackAfterInject({
    languages_processed: ["es"],
    languages_failed: [],
    strings_written: 3,
    outcomes: [["es", { Recorded: { files: 1 } }]],
  }),
  true,
);

assert.deepEqual(outcomeRecordingIssues({
  outcomes: [
    ["es", "NothingRecorded"],
    ["fr", { KeptPrevious: { recorded_at: "t" } }],
    ["de", { Recorded: { files: 1 } }],
  ],
}), [
  { lang: "es", kind: "nothing" },
  { lang: "fr", kind: "kept", detail: "t" },
]);

// Negative: treating zero-write as success must fail this suite.
assert.notEqual(
  classifyInjectReport({
    languages_processed: ["es"],
    languages_failed: [],
    strings_written: 0,
  }),
  "success",
);

console.log("injectOutcome.test.ts: ok");

assert.deepEqual(collectSkipReasons({ strings_skipped: 5, skip_reasons: { unclassified: 2 } }),
  [{ lang: "", reason: "unclassified", count: 5 }]);

// Only complete, explicit unchanged evidence may downgrade zero writes to info.
const unchangedReport: InjectReportLike = {
  languages_processed: ["es"], languages_failed: [],
  strings_written: 0, files_modified: 0, files_written: [], strings_skipped: 2,
  reports: { es: { strings_written: 0, files_modified: 0, strings_skipped: 2,
    skip_reasons: { unchanged: 2 }, files_written: [] } },
  outcomes: [["es", { KeptPrevious: { recorded_at: "2026-09-21" } }]],
  warnings: [retained, "unchanged injection: redundant new backup removed"],
};
assert.equal(classifyInjectReport(unchangedReport), "unchanged");
assert.equal(injectToastLevel("unchanged"), "info");
assert(isRedundantBackupRemoved("unchanged injection: redundant new backup removed"));
assert(isRedundantBackupRemoved("zh-CN: unchanged injection: redundant new backup removed"));
assert(!isRedundantBackupRemoved("unchanged injection: redundant new backup removed; ERROR"));
assert(!isRedundantBackupRemoved("unchanged injection completed, but duplicate backup cleanup was incomplete"));
assert.equal(classifyInjectReport({ ...unchangedReport,
  languages_processed: ["es", "fr"], strings_skipped: 4, outcomes: undefined,
  reports: { es: unchangedReport.reports!.es, fr: unchangedReport.reports!.es },
}), "unchanged");
assert.equal(classifyInjectReport({ ...unchangedReport, outcomes: [["es", "NothingRecorded"]] }), "unchanged");
assert.equal(shouldOfferPackAfterInject(unchangedReport), false);
assert.equal(shouldOfferPackAfterInject({ strings_written: 1,
  languages_processed: [], languages_failed: [["es", "failed"]] }), false);
for (const reason of ["untranslated", "duplicate", "too_long", "source_changed", "future_reason"]) {
  assert.notEqual(classifyInjectReport({ ...unchangedReport, reports: {
    es: { ...unchangedReport.reports!.es, skip_reasons: { unchanged: 1, [reason]: 1 } },
  } }), "unchanged", reason);
}
for (const override of [
  { languages_processed: [] },
  { languages_processed: ["es", "fr"] },
  { languages_failed: [["fr", "failure"]] },
  { strings_written: 1 }, { files_modified: 1 }, { files_written: ["story.html"] },
  { strings_skipped: 3 },
  { reports: { es: { ...unchangedReport.reports!.es, strings_skipped: 3 } } },
  { reports: { es: { ...unchangedReport.reports!.es, strings_skipped: 0 } } },
  { reports: { es: { ...unchangedReport.reports!.es, strings_written: 1 } } },
  { reports: { es: { ...unchangedReport.reports!.es, files_modified: 1 } } },
  { reports: { es: { ...unchangedReport.reports!.es, skip_reasons: { unchanged: 2, too_long: 1 } } } },
  { reports: { es: undefined } },
  { reports: { es: unchangedReport.reports!.es, fr: undefined } },
  { reports: { es: { ...unchangedReport.reports!.es, strings_skipped: 2.1 } } },
  { reports: { es: { ...unchangedReport.reports!.es, warnings: ["engine diagnostic"] } } },
  { warnings: ["unchanged injection completed, but duplicate backup cleanup was incomplete"] },
  { warnings: ["unrecognized backend warning"] },
  { outcomes: [["es", { Recorded: { files: 1 } }]] },
] as Partial<InjectReportLike>[]) {
  assert.notEqual(classifyInjectReport({ ...unchangedReport, ...override }), "unchanged", JSON.stringify(override));
}
