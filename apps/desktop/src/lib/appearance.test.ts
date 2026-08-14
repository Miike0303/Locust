/**
 * Lightweight asserts for appearance helpers
 * (run: npx --yes tsx src/lib/appearance.test.ts).
 */
import {
  clampTableRowHeight,
  showSourceColumnEnabled,
  TABLE_ROW_HEIGHT_DEFAULT,
  TABLE_ROW_HEIGHT_MAX,
  TABLE_ROW_HEIGHT_MIN,
} from "./appearance.ts";

const assert = {
  equal(actual: unknown, expected: unknown, message?: string) {
    if (actual !== expected) throw new Error(message ?? `${actual} !== ${expected}`);
  },
};

assert.equal(clampTableRowHeight(undefined), TABLE_ROW_HEIGHT_DEFAULT);
assert.equal(clampTableRowHeight("36"), TABLE_ROW_HEIGHT_DEFAULT);
assert.equal(clampTableRowHeight(36), 36);
assert.equal(clampTableRowHeight(10), TABLE_ROW_HEIGHT_MIN);
assert.equal(clampTableRowHeight(200), TABLE_ROW_HEIGHT_MAX);
assert.equal(clampTableRowHeight(33.6), 34);

assert.equal(showSourceColumnEnabled(undefined), true);
assert.equal(showSourceColumnEnabled(true), true);
assert.equal(showSourceColumnEnabled(false), false);

console.log("appearance.test.ts: ok");
