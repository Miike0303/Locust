/**
 * Lightweight asserts for openProjectFlow
 * (run: npx --yes tsx src/lib/openProjectFlow.test.ts).
 */
import {
  formatPickerPathFromState,
  isDetectionFailure,
  openDbCanConfirm,
  projectFromOpenResponse,
  shouldOpenProjectDb,
} from "./openProjectFlow.ts";

const assert = {
  equal(actual: unknown, expected: unknown, message?: string) {
    if (actual !== expected) throw new Error(message ?? `${actual} !== ${expected}`);
  },
  ok(cond: unknown, message?: string) {
    if (!cond) throw new Error(message ?? "expected truthy");
  },
};

assert.ok(isDetectionFailure("Could not detect game format"));
assert.ok(isDetectionFailure("format not detected"));
assert.equal(isDetectionFailure("path not found"), false);
assert.equal(isDetectionFailure("detect without the other word"), false);

assert.equal(formatPickerPathFromState(null), null);
assert.equal(formatPickerPathFromState({}), null);
assert.equal(formatPickerPathFromState({ formatPickerPath: "  " }), null);
assert.equal(
  formatPickerPathFromState({ formatPickerPath: "C:\\Games\\Title" }),
  "C:\\Games\\Title",
);

const info = projectFromOpenResponse({
  format_id: "renpy",
  format_name: "Ren'Py",
  total_strings: 3,
  project_path: "/games/title",
  project_name: "title",
  supported_modes: ["replace"],
  database_path: "/games/title.locust.db",
  added: 3,
  updated: 0,
  stale_source_reset: 0,
  removed: 0,
  preserved_translations: 0,
});
assert.equal(info.path, "/games/title");
assert.equal(info.format_id, "renpy");
assert.equal(info.name, "title");
assert.equal(info.database_path, "/games/title.locust.db");

assert.equal(shouldOpenProjectDb(undefined), false);
assert.equal(shouldOpenProjectDb(""), false);
assert.equal(shouldOpenProjectDb("/games/title"), false);
assert.equal(shouldOpenProjectDb("/games/title-pivot.locust.db"), true);
assert.equal(shouldOpenProjectDb("C:\\x\\a.locust.db"), true);
// Negative: treating a bare game path as open-db would re-extract pivots.
assert.equal(shouldOpenProjectDb("/games/title"), false);

assert.ok(openDbCanConfirm("/x/a.locust.db", "/games/title", "renpy"));
assert.equal(openDbCanConfirm("/x/a.locust.db", "/games/title", "auto"), false);
assert.equal(openDbCanConfirm("/x/a.locust.db", "  ", "renpy"), false);
assert.equal(openDbCanConfirm("/games/title", "/games/title", "renpy"), false);

console.log("openProjectFlow.test.ts: ok");
