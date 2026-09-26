import assert from "node:assert/strict";
import { acknowledgeDraft, draftEntryKey, draftProjectKey, editDraft, saveDraft, useDraftStore } from "../stores/draftStore";

const project = { path: "C:/game", format_id: "html-game", name: "Game", database_path: "C:/es.db" };
const scope = draftProjectKey(project);
const key = draftEntryKey(scope, "row");
const other = draftEntryKey(draftProjectKey({ ...project, database_path: "C:/fr.db" }), "row");
const rowB = draftEntryKey(scope, "row B");
const get = () => useDraftStore.getState().drafts[key];
const reset = () => useDraftStore.setState({ drafts: {} });

reset();
editDraft(key, "borrador español");
editDraft(other, "brouillon français");
editDraft(rowB, "otra entrada");
assert.equal(get().text, "borrador español");
assert.equal(useDraftStore.getState().drafts[other].text, "brouillon français");
acknowledgeDraft(key, "old cached translation");
assert.equal(get().text, "borrador español", "refetch must not erase dirty text");

let release!: () => void;
let calls = 0;
const write = async (text: string) => {
  calls++;
  assert.equal(text, "borrador español");
  await new Promise<void>((resolve) => { release = resolve; });
};
const first = saveDraft(key, "old", write);
assert.equal(saveDraft(key, "old", write), first, "reopened panel joins the existing save");
await Promise.resolve();
assert.equal(calls, 1);
acknowledgeDraft(key, "borrador español");
assert.equal(get().saving, true, "refetch cannot remove a pending write");
release();
assert.equal(await first, true);
assert.equal(get().text, "borrador español", "HTTP success alone must not expose stale cache");
acknowledgeDraft(key, "borrador español");
assert.equal(get(), undefined);

editDraft(key, "failed draft");
assert.equal(await saveDraft(key, "old", () => { throw new Error("conflict"); }), false);
assert.equal(get().text, "failed draft");
assert.equal(get().error, "conflict");
assert.equal(get().saving, false);
assert.equal(await saveDraft(key, "old", async () => { throw new Error("network rejected"); }), false);
assert.equal(get().error, "network rejected");
assert.equal(get().text, "failed draft");
assert.equal(await saveDraft(key, "old", async () => {}), true, "failed operation must release its promise");
assert.equal(get().error, null);

editDraft(key, "v1");
const saving = saveDraft(key, "old", async () => new Promise<void>((resolve) => { release = resolve; }));
await Promise.resolve();
editDraft(key, "v2");
release();
await saving;
acknowledgeDraft(key, "v1");
assert.equal(get().text, "v2", "late acknowledgment must preserve newer draft");

editDraft(key, "");
assert.equal(get().text, "", "empty drafts are real edits");
let saved = "not called";
await saveDraft(key, "old", async (text) => { saved = text; });
assert.equal(saved, "");
acknowledgeDraft(key, "");
assert.equal(get(), undefined);
assert.equal(useDraftStore.getState().drafts[other].text, "brouillon français");
reset();
console.log("editorDrafts.test.ts: ok");
