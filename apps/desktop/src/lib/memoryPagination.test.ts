import assert from "node:assert/strict";
import { test } from "node:test";
import { memoryPageOffset, MEMORY_PAGE_SIZE } from "./memoryPagination.ts";

test("deleting the only row on a last page returns to the last populated page", () => {
  assert.equal(memoryPageOffset(50, 50), 0);
  assert.equal(memoryPageOffset(100, 100), 50);
});

test("empty memory returns to the first page", () => {
  assert.equal(memoryPageOffset(50, 0), 0);
  assert.equal(memoryPageOffset(0, 0), 0);
});

test("existing first and next page offsets are preserved while populated", () => {
  assert.equal(MEMORY_PAGE_SIZE, 50);
  assert.equal(memoryPageOffset(0, 51), 0);
  assert.equal(memoryPageOffset(50, 51), 50);
  assert.equal(memoryPageOffset(100, 101), 100);
});
