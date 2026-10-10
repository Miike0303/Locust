import assert from "node:assert/strict";
import { test } from "node:test";
import { ApiError } from "../lib/apiError";
import { createDraftPersistence, type DraftStorage } from "../lib/draftPersistence";

class MemoryStorage implements DraftStorage {
  data = new Map<string, string>();
  get length() { return this.data.size; }
  key(i: number) { return [...this.data.keys()][i] ?? null; }
  getItem(k: string) { return this.data.get(k) ?? null; }
  setItem(k: string, v: string) { this.data.set(k, v); }
  removeItem(k: string) { this.data.delete(k); }
}
const storage = new MemoryStorage();
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage: storage, addEventListener() {} } });
const drafts = await import("./draftStore");
const repository = createDraftPersistence(() => storage);
const get = (key: string) => drafts.useDraftStore.getState().drafts[key];
const conflict = () => { throw new ApiError("409: translation changed since it was loaded"); };

test("the first baseline survives edits, persistence, hydration and branch selection", () => {
  drafts.editDraft("baseline", "first edit", "V0");
  drafts.editDraft("baseline", "second edit", "V1");
  assert.equal(get("baseline").baseline, "V0");
  assert.equal(repository.read().records.find(r => r.entryKey === "baseline")?.baseline, "V0");
  drafts.useDraftStore.setState({ drafts: {} });
  drafts.refreshDurableDrafts();
  assert.equal(get("baseline").baseline, "V0");
  const other = repository.write("baseline", "other window", null, null, "V2").record!;
  drafts.refreshDurableDrafts();
  drafts.selectDraftAlternative("baseline", other.revision);
  assert.equal(get("baseline").baseline, "V2");
});

test("expected null survives storage and is sent to the writer; old records stay readable", async () => {
  drafts.editDraft("empty", "filled", null);
  drafts.useDraftStore.setState({ drafts: {} });
  drafts.refreshDurableDrafts();
  let expected: string | null | undefined;
  await drafts.saveDraft("empty", "", async (_text, baseline) => { expected = baseline; });
  assert.equal(expected, null);
  repository.write("legacy", "old draft", null, null);
  drafts.refreshDurableDrafts();
  await drafts.saveDraft("legacy", "server", async (_text, baseline) => { expected = baseline; });
  assert.equal(expected, undefined);
});

test("conflict remains recoverable after reload and matching cache cannot discard it", async () => {
  drafts.editDraft("conflict", "mine", "V0");
  assert.equal(await drafts.saveDraft("conflict", "V0", conflict), false);
  drafts.useDraftStore.setState({ drafts: {} });
  drafts.refreshDurableDrafts();
  assert.equal(get("conflict").conflict, true);
  drafts.acknowledgeDraft("conflict", "mine");
  assert.equal(get("conflict").text, "mine");
  let writes = 0;
  assert.equal(await drafts.saveDraft("conflict", "V1", async () => { writes++; }), false);
  assert.equal(writes, 0, "automatic blur cannot resolve a conflict");
});

test("failed Load latest keeps text and recovery actions; success accepts null", async () => {
  drafts.editDraft("load", "mine", "V0");
  await drafts.saveDraft("load", "V0", conflict);
  assert.equal(await drafts.loadLatestDraft("load", async () => { throw new Error("offline"); }), false);
  assert.equal(get("load").text, "mine");
  assert.equal(get("load").conflict, true);
  assert.equal(await drafts.loadLatestDraft("load", async () => null), true);
  assert.equal(get("load").text, "");
  assert.equal(get("load").baseline, null);
  assert.equal(get("load").conflict, false);
});

test("typing during a pending save keeps text and advances the baseline only on success", async () => {
  drafts.editDraft("pending", "V1", "V0");
  let release!: () => void;
  const first = drafts.saveDraft("pending", "V0", async () => new Promise<void>(resolve => { release = resolve; }));
  assert.equal(drafts.saveDraft("pending", "V0", async () => assert.fail("duplicate write")), first);
  await Promise.resolve();
  drafts.editDraft("pending", "V2", "V0");
  release();
  await first;
  assert.equal(get("pending").text, "V2");
  assert.equal(get("pending").baseline, "V1");
});
