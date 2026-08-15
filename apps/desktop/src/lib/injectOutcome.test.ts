import assert from "node:assert/strict";
import {
  classifyInjectReport,
  collectFilesWritten,
  collectInjectWarnings,
  injectToastLevel,
  outcomeRecordingIssues,
  shouldOfferPackAfterInject,
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
