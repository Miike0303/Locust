import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

// Cover conditional branches (including the format picker) without a browser.
const source = readFileSync(new URL("../pages/Welcome.tsx", import.meta.url), "utf8");

test("Welcome has no loose gray palette utilities", () => {
  const literals = source.match(/[\w:/-]*gray-[\w/.-]+/g) ?? [];
  assert.equal(literals.length, 0, `Welcome still uses loose grays: ${literals.slice(0, 5).join(", ")}`);
});

test("Welcome uses scalable type utilities instead of arbitrary pixel text sizes", () => {
  const literals = source.match(/text-\[\d+(?:\.\d+)?px\]/g) ?? [];
  assert.equal(literals.length, 0, `Welcome still uses pixel text sizes: ${literals.join(", ")}`);
});
