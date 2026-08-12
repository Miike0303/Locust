/**
 * Lightweight asserts for injectModes (run: npx --yes tsx src/lib/injectModes.test.ts).
 */
import assert from "node:assert/strict";
import {
  availableInjectModes,
  coerceInjectMode,
  defaultInjectMode,
} from "./injectModes.ts";

// Unknown / empty → all modes, historical default add
assert.deepEqual(availableInjectModes(undefined), ["replace", "add", "direct"]);
assert.deepEqual(availableInjectModes([]), ["replace", "add", "direct"]);
assert.equal(defaultInjectMode(undefined), "add");
assert.equal(defaultInjectMode([]), "add");

// Replace-only (Unity / KiriKiri / YU-RIS)
assert.deepEqual(availableInjectModes(["replace"]), ["replace", "direct"]);
assert.equal(defaultInjectMode(["replace"]), "direct");
assert.equal(coerceInjectMode("add", ["replace"]), "direct");
assert.equal(coerceInjectMode("direct", ["replace"]), "direct");
assert.equal(coerceInjectMode("replace", ["replace"]), "replace");

// Replace + Add (Ren'Py / RPG Maker)
assert.deepEqual(availableInjectModes(["replace", "add"]), [
  "replace",
  "direct",
  "add",
]);
assert.equal(defaultInjectMode(["replace", "add"]), "add");
assert.equal(coerceInjectMode("add", ["replace", "add"]), "add");

// Add-only (defensive)
assert.deepEqual(availableInjectModes(["add"]), ["add"]);
assert.equal(defaultInjectMode(["add"]), "add");

console.log("injectModes.test.ts: ok");
