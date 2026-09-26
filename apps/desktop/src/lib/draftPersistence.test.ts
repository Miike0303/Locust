import assert from "node:assert/strict";
import { createDraftPersistence, DRAFT_PREFIX, MAX_DRAFT_TEXT, MAX_DRAFT_RECORDS, type DraftStorage } from "./draftPersistence";
class MemoryStorage implements DraftStorage {
  data = new Map<string, string>();
  failWrite = false;
  failDelete = false;
  get length() { return this.data.size; }
  key(i: number) { return [...this.data.keys()][i] ?? null; }
  getItem(k: string) { return this.data.get(k) ?? null; }
  setItem(k: string, v: string) { if (this.failWrite) throw new Error("QuotaExceededError"); this.data.set(k, v); }
  removeItem(k: string) { if (this.failDelete) throw new Error("blocked deletion"); this.data.delete(k); }
}
const storage = new MemoryStorage();
const repository = createDraftPersistence(() => storage);
const first = repository.write("es:row", "日本語 → Español 🦗\n", "offline", null).record!;
assert.equal(repository.read().records[0].text, "日本語 → Español 🦗\n");
assert.equal(repository.read().records[0].error, "offline");
const second = repository.write("es:row", "", null, first.revision).record!;
assert.equal(repository.read().records[0].text, "");
assert.equal(storage.getItem(DRAFT_PREFIX + first.revision), null);
const french = repository.write("fr:row", "français", null, null).record!;
assert.equal(repository.write("es:row", "x".repeat(MAX_DRAFT_TEXT + 1), null, second.revision).issue, "tooLarge");
assert.equal(storage.getItem(DRAFT_PREFIX + second.revision), JSON.stringify(second));
storage.failWrite = true;
assert.ok(repository.write("es:row", "unsaved after quota", null, second.revision).issue);
assert.equal(storage.getItem(DRAFT_PREFIX + second.revision), JSON.stringify(second));
storage.failWrite = false;
// Independent writers sharing a predecessor preserve both resulting branches.
const a = repository.write("es:row", "window A", null, second.revision).record!;
const b = repository.write("es:row", "window B", null, second.revision).record!;
assert.deepEqual(repository.read().records.filter(r => r.entryKey === "es:row").map(r => r.text).sort(), ["window A", "window B"]);
repository.remove(a);
assert.equal(storage.getItem(DRAFT_PREFIX + b.revision), JSON.stringify(b));
assert.equal(storage.getItem(DRAFT_PREFIX + french.revision), JSON.stringify(french));
// Death or delete failure between the new write and retiring its predecessor.
storage.failDelete = true;
const child = repository.write("es:row", "latest version", null, b.revision).record!;
assert.ok(child);
assert.equal(repository.remove(child), "unavailable");
assert.equal(storage.getItem(DRAFT_PREFIX + child.revision), JSON.stringify(child));
storage.failDelete = false;
assert.equal(repository.remove(child), null);
assert.equal(storage.getItem(DRAFT_PREFIX + b.revision), null, "obsolete ancestor must not resurrect after acknowledgement");
assert.equal(storage.getItem(DRAFT_PREFIX + child.revision), null);
// Corrupt/unsupported records remain byte-for-byte, and valid projects survive.
storage.setItem(DRAFT_PREFIX + "corrupt", "{oops");
storage.setItem(DRAFT_PREFIX + "future", JSON.stringify({ version: 8 }));
assert.equal(repository.read().issue, "invalid");
assert.equal(repository.read().records[0].revision, french.revision);
const healthy = repository.write("es:healthy", "saved beside corruption", null, null).record!;
assert.ok(healthy);
assert.equal(storage.getItem(DRAFT_PREFIX + "corrupt"), "{oops");
assert.equal(storage.getItem(DRAFT_PREFIX + "future"), JSON.stringify({ version: 8 }));
// Untrusted predecessor may not delete a different database's entry.
repository.write("es:evil", "hello", null, french.revision);
assert.equal(storage.getItem(DRAFT_PREFIX + french.revision), JSON.stringify(french));
const unavailable = createDraftPersistence(() => { throw new Error("SecurityError"); });
assert.equal(unavailable.read().issue, "unavailable");
assert.equal(unavailable.write("key", "text", null, null).issue, "unavailable");
const pretty = repository.write("pretty:entry", "preserve JSON semantics", null, null).record!;
storage.data.set(DRAFT_PREFIX + pretty.revision, JSON.stringify(pretty, null, 2));
const restoredPretty = repository.read().records.find(r => r.revision === pretty.revision)!;
assert.equal(repository.remove(restoredPretty), null);
assert.equal(storage.getItem(DRAFT_PREFIX + pretty.revision), null, "formatting must not prevent retiring a valid restored revision");
const changed = repository.write("changed:entry", "before external mutation", null, null).record!;
storage.data.set(DRAFT_PREFIX + changed.revision, "corrupted after read");
assert.equal(repository.remove(changed), "invalid");
assert.equal(storage.getItem(DRAFT_PREFIX + changed.revision), "corrupted after read", "compare-before-delete must retain a record changed since reading");
const crowded = new MemoryStorage();
crowded.setItem(DRAFT_PREFIX + "corrupt", "invalid");
for (let i = 0; i < MAX_DRAFT_RECORDS; i++) {
  const record = { ...french, revision: `revision-${i}`, parent: null };
  crowded.setItem(DRAFT_PREFIX + record.revision, JSON.stringify(record));
}
const bounded = createDraftPersistence(() => crowded);
assert.equal(bounded.read().issue, "full", "corruption cannot bypass the record count limit");
assert.equal(bounded.write("new project", "new text", null, null).record, null);
assert.equal(crowded.length, MAX_DRAFT_RECORDS + 1, "hitting the cap never evicts other drafts");
console.log("draftPersistence.test.ts: ok");
