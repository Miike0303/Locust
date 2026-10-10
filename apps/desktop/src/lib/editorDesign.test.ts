import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

// Inspect every authored branch, including inline editing and conditional detail states.
const files = [
  "../pages/Editor.tsx",
  "../pages/Review.tsx",
  "../pages/TranslationMemory.tsx",
  "../components/StringTable.tsx",
  "../components/DetailPanel.tsx",
  "../components/FilterBar.tsx",
] as const;

for (const file of files) {
  const source = readFileSync(new URL(file, import.meta.url), "utf8");

  test(`${file} has no loose gray palette utilities`, () => {
    const literals = source.match(/[\w:/-]*gray-[\w/.-]+/g) ?? [];
    assert.equal(literals.length, 0, `${file} still uses loose grays: ${literals.join(", ")}`);
  });

  test(`${file} has no arbitrary pixel text sizes`, () => {
    const literals = source.match(/text-\[\d+(?:\.\d+)?px\]/g) ?? [];
    assert.equal(literals.length, 0, `${file} still uses pixel text sizes: ${literals.join(", ")}`);
  });

  test(`${file} has no emerald-500 CSS variable`, () => {
    assert.doesNotMatch(source, /var\(--color-emerald-500\)/);
  });
}

test("selected table row retains its inset indicator using the accent token", () => {
  const source = readFileSync(new URL("../components/StringTable.tsx", import.meta.url), "utf8");
  assert.match(source, /shadow-\[inset_2px_0_0_0_var\(--color-accent\)\]/);
});
