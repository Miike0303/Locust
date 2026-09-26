// Each revision has its own immutable storage key. A tab only retires the exact
// predecessor it edited, so unrelated entries and concurrent branches survive.
export const DRAFT_PREFIX = "locust.editor.draft.v1.";
export const MAX_DRAFT_TEXT = 256 * 1024;
export const MAX_DRAFT_RECORDS = 1024;
const MAX_RECORD_SIZE = MAX_DRAFT_TEXT * 2 + 16384;
export type DraftStorageIssue = "unavailable" | "invalid" | "tooLarge" | "full";
export interface DurableDraft {
  version: 1;
  revision: string;
  parent: string | null;
  entryKey: string;
  text: string;
  error: string | null;
  updatedAt: number;
}
export interface DraftStorage {
  readonly length: number;
  key(index: number): string | null;
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}
export function browserDraftStorage(): DraftStorage {
  if (typeof window === "undefined") throw new Error("No browser storage");
  return window.localStorage;
}
export function createDraftPersistence(getStorage: () => DraftStorage = browserDraftStorage) {
  const serialized = new WeakMap<DurableDraft, string>();
  function read(): { records: DurableDraft[]; issue: DraftStorageIssue | null } {
    const records: DurableDraft[] = [];
    let issue: DraftStorageIssue | null = null;
    try {
      const storage = getStorage();
      // Snapshot keys before reading: other windows can add/remove records.
      const keys: string[] = [];
      for (let i = 0; i < storage.length; i++) {
        const key = storage.key(i);
        if (key?.startsWith(DRAFT_PREFIX)) {
          if (keys.length >= MAX_DRAFT_RECORDS) { issue = "full"; break; }
          keys.push(key);
        }
      }
      if (keys.length > MAX_DRAFT_RECORDS) issue = "full";
      for (const key of keys) {
        const raw = storage.getItem(key);
        if (raw === null) continue;
        try {
          if (raw.length > MAX_RECORD_SIZE) throw new Error("Oversized draft");
          const value = JSON.parse(raw) as DurableDraft;
          if (!value || value.version !== 1 || typeof value.revision !== "string" ||
            key !== DRAFT_PREFIX + value.revision || value.revision.length > 128 || !value.revision.length ||
            value.parent === value.revision ||
            !(value.parent === null || typeof value.parent === "string") ||
            typeof value.entryKey !== "string" || value.entryKey.length > 8192 ||
            typeof value.text !== "string" || value.text.length > MAX_DRAFT_TEXT ||
            !(value.error === null || (typeof value.error === "string" && value.error.length <= 4096)) ||
            !Number.isFinite(value.updatedAt)) throw new Error("Invalid draft");
          serialized.set(value, raw);
          records.push(value);
        } catch { if (issue !== "full") issue = "invalid"; } // Preserve invalid bytes for recovery.
      }
    } catch { issue = "unavailable"; }
    return { records, issue };
  }
  function write(entryKey: string, text: string, error: string | null, parent: string | null) {
    if (text.length > MAX_DRAFT_TEXT || entryKey.length > 8192) {
      return { record: null, issue: "tooLarge" as DraftStorageIssue };
    }
    const before = read();
    // Read failure must not be treated as an empty store and overwritten.
    if (before.issue === "unavailable" || before.issue === "full" || before.records.length >= MAX_DRAFT_RECORDS) {
      return { record: null, issue: before.issue ?? "full" as DraftStorageIssue };
    }
    try {
      const revision = typeof crypto.randomUUID === "function" ? crypto.randomUUID()
        : Array.from(crypto.getRandomValues(new Uint32Array(4)), value => value.toString(16).padStart(8, "0")).join("");
      const record: DurableDraft = { version: 1, revision, parent,
        entryKey, text, error: error?.slice(0, 4096) ?? null, updatedAt: Date.now() };
      const storage = getStorage();
      const raw = JSON.stringify(record);
      if (raw.length > MAX_RECORD_SIZE) return { record: null, issue: "tooLarge" as DraftStorageIssue };
      storage.setItem(DRAFT_PREFIX + record.revision, raw);
      serialized.set(record, raw);
      // Never delete the previous revision until the replacement is durable.
      // Checking ownership also prevents a malformed reference deleting another project.
      const previous = before.records.find(r => r.revision === parent && r.entryKey === entryKey);
      if (previous) {
        const cleanupIssue = remove(previous);
        return { record, issue: cleanupIssue ?? before.issue };
      }
      return { record, issue: before.issue };
    } catch {
      // The old revision is still available on quota/write failure.
      return { record: null, issue: "unavailable" as DraftStorageIssue };
    }
  }
  function remove(record: DurableDraft): DraftStorageIssue | null {
    try {
      const storage = getStorage();
      const all = read();
      if (all.issue === "unavailable") return all.issue;
      const lineage: DurableDraft[] = [];
      const visited = new Set<string>();
      let cursor: DurableDraft | undefined = record;
      while (cursor && !visited.has(cursor.revision)) {
        lineage.unshift(cursor);
        visited.add(cursor.revision);
        const parent: string | null = cursor.parent;
        cursor = all.records.find(r => r.revision === parent && r.entryKey === record.entryKey);
      }
      // Retire superseded ancestors first: if deletion fails, the child remains
      // and masks its old ancestors after reload (no obsolete text resurrection).
      for (const item of lineage) {
        const raw = storage.getItem(DRAFT_PREFIX + item.revision);
        if (raw === null) continue;
        if (raw !== (serialized.get(item) ?? JSON.stringify(item))) return "invalid";
        storage.removeItem(DRAFT_PREFIX + item.revision);
      }
      return null;
    } catch { return "unavailable"; }
  }
  return { read, write, remove };
}
