import assert from "node:assert/strict";
import { createDraftPersistence, DRAFT_PREFIX, type DraftStorage } from "./draftPersistence";
class FixtureStorage implements DraftStorage {
  data = new Map<string, string>();
  failEntry: string | null = null;
  failRead = false;
  get length() { if (this.failRead) throw new Error("SecurityError"); return this.data.size; }
  key(i: number) { return [...this.data.keys()][i] ?? null; }
  getItem(key: string) { if (this.failRead) throw new Error("SecurityError"); return this.data.get(key) ?? null; }
  setItem(key: string, value: string) { if (JSON.parse(value).entryKey === this.failEntry) throw new Error("quota"); this.data.set(key, value); }
  removeItem(key: string) { this.data.delete(key); }
}
const storage = new FixtureStorage();
const repository = createDraftPersistence(() => storage);
const initial = repository.write("es:entry", "persisted draft", "offline error", null).record!;
storage.data.set(DRAFT_PREFIX + initial.revision, JSON.stringify({ ...initial, saving: true, pending: {} }));
repository.write("fr:entry", "brouillon", null, null);
Object.defineProperty(globalThis, "window", { value: { localStorage: storage, addEventListener() {} }, configurable: true });
const drafts = await import("../stores/draftStore");
const get = () => drafts.useDraftStore.getState();
assert.equal(get().drafts["es:entry"].text, "persisted draft");
assert.equal(get().drafts["es:entry"].error, "offline error");
assert.equal(get().drafts["es:entry"].saving, false, "runtime save state must never be hydrated");
assert.equal(await drafts.saveDraft("es:entry", "server", async () => {}), true, "hydration must not create a pending promise");
storage.failEntry = "es:entry";
drafts.editDraft("es:entry", "new text after quota");
assert.equal(get().drafts["es:entry"].text, "new text after quota");
assert.ok(get().persistenceIssues["es:entry"]);
assert.equal(drafts.draftAlternatives("es:entry", get().records, get().drafts["es:entry"]).length, 0, "old persisted revision is not a separate conflict");
drafts.editDraft("fr:entry", "nouveau brouillon");
assert.ok(get().persistenceIssues["es:entry"], "saving a different DB must not hide the failed persistence");
assert.equal(repository.read().records.find(r => r.entryKey === "es:entry")?.text, "persisted draft");
assert.equal(repository.read().records.find(r => r.entryKey === "fr:entry")?.text, "nouveau brouillon");
storage.failEntry = null;
drafts.editDraft("es:entry", "retry persists successfully");
assert.equal(get().persistenceIssues["es:entry"], undefined);
assert.equal(repository.read().records.find(r => r.entryKey === "es:entry")?.text, "retry persists successfully");
storage.failRead = true;
drafts.editDraft("es:entry", "read blocked but memory kept");
assert.equal(get().drafts["es:entry"].text, "read blocked but memory kept");
assert.ok(get().persistenceIssues["es:entry"]);
storage.failRead = false;
assert.equal(repository.read().records.find(r => r.entryKey === "es:entry")?.text, "retry persists successfully");
drafts.editDraft("es:entry", "read recovered");
const predecessor = get().drafts["es:entry"].revision!;
const external = repository.write("es:entry", "other window", null, predecessor).record!;
drafts.refreshDurableDrafts();
assert.equal(get().drafts["es:entry"].text, "read recovered", "external events may not overwrite active text");
drafts.selectDraftAlternative("es:entry", external.revision);
assert.equal(get().drafts["es:entry"].text, "other window");
assert.ok(repository.read().records.some(r => r.text === "read recovered"), "selection must preserve the previously active text even after another window retired its predecessor");
drafts.acknowledgeDraft("es:entry", "other window");
assert.equal(get().drafts["es:entry"].text, "read recovered", "acknowledging one branch must preserve the other");
assert.equal(repository.read().records.find(r => r.entryKey === "fr:entry")?.text, "nouveau brouillon");
drafts.editDraft("es:entry", "old durable version");
storage.failEntry = "es:entry";
drafts.editDraft("es:entry", "new version that only reached the DB");
assert.equal(await drafts.saveDraft("es:entry", "server", async () => {}), true);
drafts.acknowledgeDraft("es:entry", "new version that only reached the DB");
assert.equal(get().drafts["es:entry"], undefined, "successful DB save after quota must retire the old durable version instead of resurrecting it");
assert.equal(repository.read().records.some(r => r.entryKey === "es:entry"), false);
console.log("editorDraftPersistence.test.ts: ok");
