import assert from "node:assert/strict";
import { test } from "node:test";
import {
  availableInjectModes,
  coerceInjectMode,
  defaultInjectMode,
  type OutputMode,
} from "./injectModes.ts";

test("Replace-only defaults and illegal Add selections use a translated copy", () => {
  assert.deepEqual(availableInjectModes(["replace"]), ["replace", "direct"]);
  assert.equal(defaultInjectMode(["replace"]), "replace");
  assert.equal(coerceInjectMode("add", ["replace"]), "replace");
});

test("Replace-only preserves explicit Direct and Replace choices", () => {
  assert.equal(coerceInjectMode("direct", ["replace"]), "direct");
  assert.equal(coerceInjectMode("replace", ["replace"]), "replace");
});

test("Replace + Add keeps the historical Add default and valid choices", () => {
  assert.deepEqual(availableInjectModes(["replace", "add"]), ["replace", "direct", "add"]);
  assert.equal(defaultInjectMode(["replace", "add"]), "add");
  for (const mode of ["add", "replace", "direct"] as const) {
    assert.equal(coerceInjectMode(mode, ["replace", "add"]), mode);
  }
});

test("Add-only remains Add and coerces unsupported choices", () => {
  assert.deepEqual(availableInjectModes(["add"]), ["add"]);
  assert.equal(defaultInjectMode(["add"]), "add");
  assert.equal(coerceInjectMode("direct", ["add"]), "add");
  assert.equal(coerceInjectMode("replace", ["add"]), "add");
});

test("unknown, empty and unrecognized modes preserve the legacy non-Direct default", () => {
  for (const supported of [undefined, null, [], ["future"] as unknown as OutputMode[]]) {
    assert.deepEqual(availableInjectModes(supported), ["replace", "add", "direct"]);
    assert.equal(defaultInjectMode(supported), "add");
    assert.equal(coerceInjectMode("direct", supported), "direct");
  }
});
