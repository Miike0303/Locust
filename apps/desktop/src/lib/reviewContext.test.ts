import assert from "node:assert/strict";
import { test } from "node:test";
import { reviewContext } from "./reviewContext.ts";

type Entry = { id: string; file_path: string };
const entry = (id: string, file_path = "scripts.rpa"): Entry => ({ id, file_path });
const ids = (entries: Entry[]) => entries.map(row => row.id);

test("archive context excludes preceding and following members sharing the same file path", () => {
  const rows = ["chapter1.rpyc", "chapter2.rpyc", "chapter3.rpyc"].flatMap(member =>
    Array.from({ length: 5 }, (_, n) => entry(`scripts.rpa#${member}#s${n}`)));
  assert.deepEqual(ids(reviewContext(rows[5], rows)),
    [0, 1, 2, 3].map(n => `scripts.rpa#chapter2.rpyc#s${n}`));
  assert.deepEqual(ids(reviewContext(rows[9], rows)),
    [1, 2, 3, 4].map(n => `scripts.rpa#chapter2.rpyc#s${n}`));
});

for (const locator of [
  (n: number) => `script.rpy#${n}`,
  (n: number) => `scripts.rpa#chapter2.rpyc#s${n}`,
  (n: number) => `Map001.json#0#event_1#page_0#cmd_${n}#msg`,
  (n: number) => `Map001.json#0#event_${n}#page_0#cmd_0#msg`,
  (n: number) => `Map001.json#0#event_1#page_${n}#cmd_0#msg`,
]) {
  test(`context keeps numeric 9/10 ordering within a logical script: ${locator(8)}`, () => {
    const rows = Array.from({ length: 120 }, (_, n) => entry(locator(n + 1)))
      .sort((a, b) => a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
    const before = [...rows];
    assert.deepEqual(ids(reviewContext(entry(locator(8)), rows)), [5, 6, 7, 8, 9, 10, 11].map(locator));
    assert.deepEqual(rows, before, "context selection must not reorder the cached candidates");
  });
}

test("loose-file context uses the script locator and keeps file edges", () => {
  const rows = [entry("other.rpy#1", "script.rpy"),
    ...[1, 2, 3, 4, 5].map(n => entry(`script.rpy#${n}`, "script.rpy"))];
  assert.deepEqual(ids(reviewContext(rows[1], rows)), [1, 2, 3, 4].map(n => `script.rpy#${n}`));
  assert.deepEqual(ids(reviewContext(rows[5], rows)), [2, 3, 4, 5].map(n => `script.rpy#${n}`));
});

test("context falls back to the current entry when its locator is absent", () => {
  const current = entry("scripts.rpa#chapter2.rpyc#s0");
  assert.deepEqual(reviewContext(current, [entry("scripts.rpa#chapter1.rpyc#s0")]), [current]);
});

test("legacy ids without position separators retain same-file numeric context", () => {
  const rows = [2, 10, 9].map(n => entry(`r${n}`, "dialogue.json"));
  assert.deepEqual(ids(reviewContext(rows[2], [...rows, entry("r8", "other.json")])), ["r2", "r9", "r10"]);
});

test("troop battle commands keep their numeric neighbors", () => {
  const locator = (n: number) => `Troops.json#1#page_0#cmd_${n}#msg`;
  const rows = Array.from({ length: 20 }, (_, n) => entry(locator(n + 1), "Troops.json"));
  assert.deepEqual(ids(reviewContext(entry(locator(8), "Troops.json"), rows)), [5, 6, 7, 8, 9, 10, 11].map(locator));
});

test("archive member names that contain command-like text stay separate", () => {
  const rows = ["0#cmd_1#chapter1.rpyc", "0#cmd_2#chapter2.rpyc"].flatMap(member =>
    Array.from({ length: 4 }, (_, n) => entry(`scripts.rpa#${member}#s${n}`)));
  assert.deepEqual(ids(reviewContext(rows[4], rows)),
    [0, 1, 2, 3].map(n => `scripts.rpa#0#cmd_2#chapter2.rpyc#s${n}`));
});
