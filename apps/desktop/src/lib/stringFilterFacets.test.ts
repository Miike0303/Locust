/**
 * Lightweight asserts for stringFilterFacets
 * (run: npx --yes tsx src/lib/stringFilterFacets.test.ts).
 */
import {
  coerceExactFilterValue,
  facetOptions,
  filePathFilterPatch,
  filePathOptionLabel,
  tagFilterPatch,
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

assert.deepEqual(facetOptions(undefined), []);
assert.deepEqual(facetOptions([]), []);
assert.deepEqual(
  facetOptions(["  ui ", "", "quest", "   "]),
  ["ui", "quest"],
  "blank facet values are dropped"
);

console.log("stringFilterFacets.test.ts: ok");
