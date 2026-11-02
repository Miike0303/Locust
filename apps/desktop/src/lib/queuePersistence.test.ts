import assert from "node:assert/strict";
import { test } from "node:test";
import type { QueueItem } from "../stores/queueStore";
import type { TranslationStartParams } from "./api";
import { QUEUE_STORAGE_KEY, readQueue, writeQueue, restoreQueue, serializeQueue, type QueueHistoryEntry } from "./queuePersistence";

const item: QueueItem = {
  id: "one", projectName: "Game", projectPath: "/game", formatId: "renpy", status: "pending",
  progress: { completed: 4, total: 10, costSoFar: 0.5, costIsComplete: false, startedAt: 123 },
  error: null, validationIssues: null, validationError: null,
};
const translationParams: TranslationStartParams = {
  provider_id: "mock", fallback_provider_ids: ["other"], options: {
    source_lang: "ja", target_lang: "es", batch_size: 2, max_concurrent: 1,
    max_batch_tokens: null, cost_limit_usd: 2, game_context: "Game", use_glossary: true,
    use_memory: true, skip_approved: true,
  },
};
const history: QueueHistoryEntry[] = [{
  id: "attempt", projectName: "Game", projectPath: "/game", status: "done",
  finishedAt: "2026-10-19T12:00:00.000Z", completed: 10, total: 10,
  costSoFar: 0.5, costIsComplete: true, validationIssues: 2,
}];
const snapshot = { items: [item], translationParams, history };
const empty = { items: [], translationParams: null, history: [] };

test("queue persistence round trips items, translation settings and history", () => {
  assert.deepEqual(restoreQueue(serializeQueue(snapshot)), snapshot);
});

test("queue serialization excludes all runtime state", () => {
  const raw = serializeQueue({ ...snapshot, isRunning: true, isPanelOpen: true,
    globalProgress: item.progress, cancelRequested: true, activeJobId: "job", activeOpening: {} });
  assert.deepEqual(Object.keys(JSON.parse(raw!)).sort(), ["history", "items", "translationParams", "version"]);
});

test("queue restore converts only in-flight items to interrupted errors and preserves progress", () => {
  const statuses = ["pending", "extracting", "translating", "validating", "done", "error", "cancelled"] as const;
  const items = statuses.map(status => ({ ...item, id: status, status, error: status === "error" ? "Existing failure" : null }));
  const restored = restoreQueue(serializeQueue({ ...snapshot, items }));
  for (let i = 0; i < items.length; i++) {
    assert.deepEqual(restored.items[i], i >= 1 && i <= 3
      ? { ...items[i], status: "error", error: "queue.interrupted" } : items[i]);
  }
});

test("queue restore ignores malformed and unsupported snapshots", () => {
  for (const raw of [null, "{", "null", "[]", JSON.stringify({ ...snapshot, version: 2 })]) {
    assert.deepEqual(restoreQueue(raw), empty);
  }
});

test("missing, blocked and quota-limited storage and serialization errors never throw", () => {
  const blocked = { getItem() { throw new Error("blocked"); }, setItem() { throw new Error("quota"); }, removeItem() {} };
  assert.deepEqual(readQueue(null), empty);
  assert.deepEqual(readQueue(blocked), empty);
  assert.doesNotThrow(() => writeQueue(snapshot, blocked));
  assert.doesNotThrow(() => writeQueue(snapshot, null));
  const cyclic = { ...snapshot, items: [] as any[] }; cyclic.items.push(cyclic);
  assert.doesNotThrow(() => writeQueue(cyclic, blocked));
});

test("queue storage uses the versioned key and caps newest-first history at 20", () => {
  const many = Array.from({ length: 25 }, (_, i) => ({ ...history[0], id: String(i) }));
  let raw = "";
  const storage = {
    getItem(key: string) { return key === QUEUE_STORAGE_KEY ? raw || null : null; },
    setItem(key: string, value: string) { assert.equal(key, "locust.queue.v1"); raw = value; }, removeItem() {},
  };
  writeQueue({ ...snapshot, history: many }, storage);
  assert.deepEqual(readQueue(storage).history, many.slice(0, 20));
  assert.deepEqual(restoreQueue(JSON.stringify({ ...snapshot, version: 1, history: many })).history, many.slice(0, 20));
});


test("queue writes report quota, security and unavailable-storage failures and allow retry", () => {
  for (const reason of ["QuotaExceededError", "SecurityError"]) {
    let blocked = true;
    const values = new Map<string, string>();
    const storage = { getItem: (key: string) => values.get(key) ?? null, removeItem() {},
      setItem(key: string, value: string) { if (blocked) throw new Error(reason); values.set(key, value); } };
    assert.equal(writeQueue(snapshot, storage)?.saved, false);
    assert.equal(values.size, 0);
    blocked = false;
    assert.equal(writeQueue(snapshot, storage)?.saved, true);
    assert.deepEqual(readQueue(storage).items, [item]);
  }
  assert.equal(writeQueue(snapshot, null)?.saved, false);
});

test("partly invalid snapshots salvage valid items, settings and history independently", () => {
  const restored = restoreQueue(JSON.stringify({ ...snapshot, version: 1,
    items: [item, { ...item, id: "bad", progress: null }], history: [null, ...history] }));
  assert.deepEqual(restored, snapshot);
  assert.deepEqual(restoreQueue(JSON.stringify({ ...snapshot, version: 1,
    translationParams: {}, history: [null, ...history] })), { ...snapshot, translationParams: null });
  assert.deepEqual(restoreQueue(JSON.stringify({ ...snapshot, version: 1, history: null })), { ...snapshot, history: [] });
});

test("damaged original bytes survive subsequent writes, including repeated damage", () => {
  const originals = ["{broken", JSON.stringify({ ...snapshot, version: 1, items: [item, null] })];
  const values = new Map<string, string>();
  const storage = { getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => { values.set(key, value); }, removeItem() {} };
  for (const raw of originals) {
    values.set(QUEUE_STORAGE_KEY, raw);
    readQueue(storage);
    assert.equal(writeQueue({ ...snapshot, items: [...snapshot.items, { ...item, id: "added" }] }, storage)?.saved, true);
    assert.ok([...values.entries()].some(([key, value]) => key !== QUEUE_STORAGE_KEY && value === raw));
  }
  for (const raw of originals) assert.ok([...values.values()].includes(raw));
});

test("a failed recovery copy leaves the original untouched until a successful retry", () => {
  const original = "{unreadable";
  const values = new Map([[QUEUE_STORAGE_KEY, original]]);
  let blocked = true;
  const storage = { getItem: (key: string) => values.get(key) ?? null, removeItem() {},
    setItem(key: string, value: string) { if (blocked && key !== QUEUE_STORAGE_KEY) throw new Error("quota"); values.set(key, value); } };
  assert.equal(writeQueue(snapshot, storage)?.saved, false);
  assert.equal(values.get(QUEUE_STORAGE_KEY), original);
  blocked = false;
  assert.equal(writeQueue(snapshot, storage)?.saved, true);
  assert.ok([...values.entries()].some(([key, value]) => key !== QUEUE_STORAGE_KEY && value === original));
});

const owner = { jobId: "owned-job", itemId: item.id, projectKey: JSON.stringify(["/game.locust.db", "/game", "renpy"]) };
test("queue persists job ownership with its project identity and restores translation for reconciliation", () => {
  const state = { ...snapshot, items: [{ ...item, status: "translating" as const }], activeJob: owner };
  assert.deepEqual(restoreQueue(serializeQueue(state)), state);
});

test("invalid or mismatched job ownership cannot attach a queued project to another job", () => {
  for (const activeJob of [{ ...owner, itemId: "missing" }, { ...owner, projectKey: "invalid" },
    { ...owner, projectKey: JSON.stringify(["other.db", "/other", "renpy"]) }]) {
    const restored = restoreQueue(JSON.stringify({ ...snapshot, version: 1,
      items: [{ ...item, status: "translating" }], activeJob }));
    assert.equal(restored.items[0].status, "error");
    assert.equal(restored.items[0].error, "queue.interrupted");
    assert.equal(restored.activeJob ?? null, null);
  }
});
