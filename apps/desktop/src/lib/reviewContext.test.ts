import assert from "node:assert/strict";
import { test } from "node:test";
import { performance } from "node:perf_hooks";
import * as contextLibrary from "./reviewContext.ts";
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

test("indexed context matches every legacy window across mixed numeric script locators", () => {
  const rows = [
    ...["chapter1.rpyc", "chapter2.rpyc", "0#cmd_1#chapter3.rpyc"].flatMap(member =>
      Array.from({ length: 14 }, (_, n) => entry(`scripts.rpa#${member}#s${n}`))),
    ...[0, 1, 2, 10].flatMap(event => [0, 1, 10].flatMap(page =>
      [0, 1, 2, 9, 10, 11].map(command =>
        entry(`Map001.json#0#event_${event}#page_${page}#cmd_${command}#msg`)))),
    ...["Troops.json", "CommonEvents.json"].flatMap(file =>
      [1, 2, 10].flatMap(index => [0, 2, 10].map(command =>
        entry(`${file}#${index}#page_0#cmd_${command}`)))),
    ...[1, 2, 9, 10, 11].map(n => entry(`script.rpy#${n}`)),
    ...[1, 2, 9, 10].map(n => entry(`r${n}`)),
    entry("scripts.rpa#chapter2.rpyc#s2", "other.rpa"),
  ].sort((a, b) => a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
  const before = [...rows];
  const index = contextLibrary.buildReviewContextIndex(rows);
  for (const current of rows) {
    assert.deepEqual(index.lookup(current), reviewContext(current, rows), current.id);
  }
  assert.deepEqual(rows, before, "building the index must not reorder the input");
  const current = entry("scripts.rpa#chapter2.rpyc#s2");
  assert.deepEqual(ids(index.lookup(current)),
    [0, 1, 2, 3, 4, 5].map(n => `scripts.rpa#chapter2.rpyc#s${n}`));
  const window = index.lookup(current);
  window.pop();
  assert.deepEqual(index.lookup(current), reviewContext(current, rows), "windows are independent copies");
});

test("indexed context preserves absent-row fallbacks and the first duplicate id", () => {
  const rows = [entry("script.rpy#s2"), entry("script.rpy#s10"), entry("script.rpy#s2"),
    entry("script.rpy#s1"), entry("script.rpy#s3"), entry("script.rpy#s4")];
  const index = contextLibrary.buildReviewContextIndex(rows);
  for (const current of [entry("missing.rpy#s1"), entry("script.rpy#s99"),
    entry("script.rpy#s2", "other.rpy"), ...rows]) {
    assert.deepEqual(index.lookup(current), reviewContext(current, rows));
  }
  const current = entry("empty.rpy#s0");
  assert.deepEqual(contextLibrary.buildReviewContextIndex<Entry>([]).lookup(current), [current]);
});

test("100k-row context builds once for 100 lookups and reports legacy/indexed timings", () => {
  const rows = Array.from({ length: 100_000 }, (_, n) => entry(`scripts.rpa#chapter.rpyc#s${n}`))
    .sort((a, b) => a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
  const selected = Array.from({ length: 100 }, (_, n) => rows[n * 997]);
  let builds = 0;
  const build = () => { builds++; return contextLibrary.buildReviewContextIndex(rows); };
  const indexedStart = performance.now();
  const index = build();
  const built = performance.now();
  const actual = selected.map(current => index.lookup(current));
  const indexedEnd = performance.now();
  // The legacy path re-sorts all rows per lookup; compare a few samples so the
  // unit suite stays fast, while every indexed lookup is still exercised above.
  const samples = [0, 50, 99];
  const expected = samples.map(i => reviewContext(selected[i], rows));
  const legacyEnd = performance.now();
  assert.deepEqual(samples.map(i => actual[i]), expected);
  assert.equal(builds, 1, "selection must reuse the same built index");
  console.log(`100k rows / 100 lookups: legacy(${samples.length} lookups)=${(legacyEnd - indexedEnd).toFixed(2)}ms; build=${(built - indexedStart).toFixed(2)}ms; indexed lookups=${(indexedEnd - built).toFixed(2)}ms; indexed total=${(indexedEnd - indexedStart).toFixed(2)}ms; builds=${builds}`);
});
