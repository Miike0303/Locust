import assert from "node:assert/strict";
import type { StringEntry } from "./api";
import { binaryBudgetHint, encodedByteLen } from "./binaryBudget";

const entry = (metadata: Record<string, unknown>, source = "こんにちは") => ({ source, metadata } as StringEntry);
assert.equal(encodedByteLen("utf8", "😀"), 4);
assert.equal(encodedByteLen("utf16le", "😀"), 4);
assert.equal(encodedByteLen("sjis", "東京"), null);
assert.equal(binaryBudgetHint(entry({ binary_slot: "utf8" })).capacity, 15);
assert.equal(binaryBudgetHint(entry({ binary_slot: "utf8", locust_injection_source: "日本語", locust_injection_capacity: { encoding: "utf8", bytes: 9 } }, "A much longer English pivot")).capacity, 9);
assert.equal(binaryBudgetHint(entry({ binary_slot: "utf8", locust_injection_capacity: { encoding: "utf8", bytes: 999 } })).capacity, null);
const capable = { binary_slot: "utf8", extraction_method: "textasset", textasset_rewrite: "serialized-v1", unity_serialized_version: 22 };
assert.equal(binaryBudgetHint(entry(capable)).capacity, 1_048_576);
assert.equal(binaryBudgetHint(entry({ ...capable, unity_serialized_version: "22" })).capacity, 15);
assert.equal(binaryBudgetHint(entry({ ...capable, unity_serialized_version: 16 })).expandable, false);
assert.equal(binaryBudgetHint(entry({ ...capable, extraction_method: "monobehaviour" })).capacity, 15);
for (const extraction_method of ["textasset_loc_line", "textasset_csv_cell"]) {
  const hint = binaryBudgetHint(entry({ ...capable, extraction_method }));
  assert.equal(hint.capacity, null);
  assert.equal(hint.grouped, true);
  assert.equal(hint.expandable, true);
}
console.log("binaryBudget.test.ts: ok");
