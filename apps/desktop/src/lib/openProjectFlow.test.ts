/**
 * Lightweight asserts for openProjectFlow
 * (run: npx --yes tsx src/lib/openProjectFlow.test.ts).
 */
import {
  formatPickerPathFromState,
  isDetectionFailure,
  projectFromOpenResponse,
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
});
assert.equal(info.path, "/games/title");
assert.equal(info.format_id, "renpy");
assert.equal(info.name, "title");

console.log("openProjectFlow.test.ts: ok");
