/**
 * Lightweight asserts for stringFilterFacets
 * (run: npx --yes tsx src/lib/stringFilterFacets.test.ts).
 */
import {
  coerceExactFilterValue,
  filePathFilterPatch,
  filePathOptionLabel,
  tagFilterPatch,
  uniqueSortedFilePaths,
  uniqueSortedTags,
} from "./stringFilterFacets.ts";

const assert = {
  equal(actual: unknown, expected: unknown, message?: string) {
    if (actual !== expected) throw new Error(message ?? `${actual} !== ${expected}`);
  },
  deepEqual(actual: unknown, expected: unknown, message?: string) {
    if (JSON.stringify(actual) !== JSON.stringify(expected)) {
      throw new Error(
        message ?? `${JSON.stringify(actual)} !== ${JSON.stringify(expected)}`
      );
    }
  },
};

const entries = [
  { file_path: "scripts/zeta.json", tags: [" ui ", "quest", ""] },
  { file_path: "scripts/alpha.json", tags: ["quest", "  combat  "] },
  { file_path: "scripts/zeta.json", tags: ["ui", "   "] },
  { file_path: "", tags: [] },
];

assert.deepEqual(
  uniqueSortedFilePaths(entries),
  ["scripts/alpha.json", "scripts/zeta.json"],
  "file paths are unique, sorted, and non-empty"
);
assert.deepEqual(
  uniqueSortedTags(entries),
  ["combat", "quest", "ui"],
  "tags are flattened, trimmed, non-empty, unique, and sorted"
);

assert.equal(coerceExactFilterValue("  scripts/alpha.json  "), "scripts/alpha.json");
assert.equal(coerceExactFilterValue("quest"), "quest");
assert.equal(coerceExactFilterValue("   "), undefined);
assert.equal(coerceExactFilterValue(""), undefined);

assert.deepEqual(filePathFilterPatch("  scripts/alpha.json  "), {
  file_path: "scripts/alpha.json",
  offset: 0,
});
assert.deepEqual(filePathFilterPatch("   "), {
  file_path: undefined,
  offset: 0,
});
assert.deepEqual(tagFilterPatch("  quest  "), { tag: "quest", offset: 0 });
assert.deepEqual(tagFilterPatch(""), { tag: undefined, offset: 0 });

assert.equal(filePathOptionLabel("scripts/dialogue/main.json"), "main.json");
assert.equal(filePathOptionLabel("scripts\\dialogue\\main.json"), "main.json");
assert.equal(filePathOptionLabel("main.json"), "main.json");

console.log("stringFilterFacets.test.ts: ok");
