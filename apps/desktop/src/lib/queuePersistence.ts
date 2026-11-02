import type { TranslationStartParams } from "./api";
import type { QueueItem } from "../stores/queueStore";
import { browserStorage, type StringStorage } from "./storage";

export const QUEUE_STORAGE_KEY = "locust.queue.v1";
export const QUEUE_RECOVERY_KEY = `${QUEUE_STORAGE_KEY}.recovery`;
export const MAX_QUEUE_HISTORY = 20;
export interface QueueHistoryEntry {
  id: string;
  projectName: string;
  projectPath: string;
  status: "done" | "error" | "cancelled";
  finishedAt: string;
  completed: number;
  total: number;
  costSoFar: number;
  costIsComplete: boolean;
  validationIssues: number | null;
}
export interface QueueJobOwner {
  jobId: string;
  itemId: string;
  /** Canonical [database path, source path, format] identity, independent of the open UI project. */
  projectKey: string;
}
export interface PersistedQueue {
  activeJob?: QueueJobOwner | null;
  /** Read metadata only; never serialized into the live snapshot. */
  recoveryAvailable?: boolean;
  items: QueueItem[];
  translationParams: TranslationStartParams | null;
  history: QueueHistoryEntry[];
}
const emptyQueue = (): PersistedQueue => ({ items: [], translationParams: null, history: [] });
const record = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
const nonnegative = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v) && v >= 0;
const nullableString = (v: unknown) => v === null || typeof v === "string";
const issueCount = (v: unknown) => v === null || (nonnegative(v) && Number.isInteger(v));
const statuses = new Set(["pending", "extracting", "translating", "validating", "done", "error", "cancelled"]);
function validItem(v: unknown): v is QueueItem {
  if (!record(v) || !record(v.progress)) return false;
  const p = v.progress;
  return typeof v.id === "string" && typeof v.projectName === "string" && typeof v.projectPath === "string" &&
    nullableString(v.formatId) && typeof v.status === "string" && statuses.has(v.status) &&
    nullableString(v.error) && issueCount(v.validationIssues) && nullableString(v.validationError) &&
    nonnegative(p.completed) && nonnegative(p.total) && nonnegative(p.costSoFar) &&
    (p.costIsComplete === undefined || typeof p.costIsComplete === "boolean") &&
    (p.startedAt === null || nonnegative(p.startedAt));
}
function validParams(v: unknown): v is TranslationStartParams | null {
  if (v === null) return true;
  if (!record(v) || !record(v.options)) return false;
  const o = v.options;
  return typeof v.provider_id === "string" &&
    (v.fallback_provider_ids === undefined || (Array.isArray(v.fallback_provider_ids) && v.fallback_provider_ids.every(id => typeof id === "string"))) &&
    typeof o.source_lang === "string" && typeof o.target_lang === "string" &&
    [o.batch_size, o.max_concurrent].every(n => n === undefined || (nonnegative(n) && n > 0 && Number.isInteger(n))) &&
    (o.max_batch_tokens === undefined || o.max_batch_tokens === null || (nonnegative(o.max_batch_tokens) && Number.isInteger(o.max_batch_tokens))) &&
    (o.cost_limit_usd === null || nonnegative(o.cost_limit_usd)) && nullableString(o.game_context) &&
    typeof o.use_glossary === "boolean" && typeof o.use_memory === "boolean" && typeof o.skip_approved === "boolean";
}
function validHistory(v: unknown): v is QueueHistoryEntry {
  return record(v) && typeof v.id === "string" && typeof v.projectName === "string" && typeof v.projectPath === "string" &&
    (v.status === "done" || v.status === "error" || v.status === "cancelled") &&
    typeof v.finishedAt === "string" && Number.isFinite(Date.parse(v.finishedAt)) &&
    nonnegative(v.completed) && nonnegative(v.total) && nonnegative(v.costSoFar) &&
    typeof v.costIsComplete === "boolean" && issueCount(v.validationIssues);
}
function copyItem(item: QueueItem): QueueItem {
  const { id, projectName, projectPath, formatId, status, error, validationIssues, validationError } = item;
  const { completed, total, costSoFar, costIsComplete, startedAt } = item.progress;
  return { id, projectName, projectPath, formatId, status, error, validationIssues, validationError,
    progress: { completed, total, costSoFar, ...(costIsComplete === undefined ? {} : { costIsComplete }), startedAt } };
}

function validOwner(value: unknown, items: QueueItem[]): value is QueueJobOwner {
  if (!record(value) || typeof value.jobId !== "string" || !value.jobId ||
    typeof value.itemId !== "string" || typeof value.projectKey !== "string") return false;
  const item = items.find(i => i.id === value.itemId && i.status === "translating");
  if (!item) return false;
  try {
    const key: unknown = JSON.parse(value.projectKey);
    return Array.isArray(key) && key.length === 3 && nullableString(key[0]) &&
      key[1] === item.projectPath && typeof key[2] === "string" && key[2] === item.formatId;
  } catch { return false; }
}

/** Pure, versioned snapshot: serialize only durable work and its backend owner. */
export function serializeQueue<T extends PersistedQueue>(state: T): string | null {
  try {
    return JSON.stringify({ version: 1, items: state.items.map(copyItem),
      translationParams: state.translationParams, history: state.history.slice(0, MAX_QUEUE_HISTORY),
      ...(state.activeJob ? { activeJob: { jobId: state.activeJob.jobId, itemId: state.activeJob.itemId,
        projectKey: state.activeJob.projectKey } } : {}),
    });
  } catch { return null; }
}

function decodeQueue(raw: string | null): { snapshot: PersistedQueue; damaged: boolean } {
  if (raw === null) return { snapshot: emptyQueue(), damaged: false };
  try {
    const value: unknown = JSON.parse(raw);
    if (!record(value) || value.version !== 1) {
      return { snapshot: emptyQueue(), damaged: true };
    }
    const items = Array.isArray(value.items) ? value.items.filter(validItem) : [];
    const history = Array.isArray(value.history) ? value.history.filter(validHistory) : [];
    const params = value.translationParams;
    const paramsValid = validParams(params);
    const owner = value.activeJob;
    const ownerValid = validOwner(owner, items);
    const activeJob = ownerValid ? { jobId: owner.jobId, itemId: owner.itemId, projectKey: owner.projectKey } : null;
    return {
      damaged: !Array.isArray(value.items) || items.length !== value.items.length ||
        !Array.isArray(value.history) || history.length !== value.history.length || !paramsValid ||
        (value.activeJob != null && !ownerValid),
      snapshot: {
        items: items.map(item => {
          const copy = copyItem(item);
          return ["extracting", "translating", "validating"].includes(copy.status) && copy.id !== activeJob?.itemId
            ? { ...copy, status: "error", error: "queue.interrupted" } : copy;
        }),
        translationParams: paramsValid ? params : null,
        history: history.slice(0, MAX_QUEUE_HISTORY),
        ...(activeJob ? { activeJob } : {}),
      },
    };
  } catch { return { snapshot: emptyQueue(), damaged: true }; }
}

/** Legacy interrupted work stays idle; owned translations are reconciled by the store. */
export function restoreQueue(raw: string | null): PersistedQueue {
  return decodeQueue(raw).snapshot;
}

export function readQueue(storage?: StringStorage | null): PersistedQueue {
  try {
    const target = storage === undefined ? browserStorage() : storage;
    const { snapshot, damaged } = decodeQueue(target?.getItem(QUEUE_STORAGE_KEY) ?? null);
    return damaged || target?.getItem(QUEUE_RECOVERY_KEY) != null
      ? { ...snapshot, recoveryAvailable: true } : snapshot;
  } catch { return emptyQueue(); }
}

export interface QueueSaveResult { saved: boolean; recoveryAvailable: boolean }

/** Preserve damaged bytes before replacement. A failed copy must leave the live key untouched. */
export function writeQueue(state: PersistedQueue, storage?: StringStorage | null): QueueSaveResult {
  let recoveryAvailable = state.recoveryAvailable === true;
  try {
    const target = storage === undefined ? browserStorage() : storage;
    const raw = serializeQueue(state);
    if (!target || raw === null) return { saved: false, recoveryAvailable };
    const original = target.getItem(QUEUE_STORAGE_KEY);
    recoveryAvailable = recoveryAvailable || target.getItem(QUEUE_RECOVERY_KEY) !== null;
    if (original !== null && decodeQueue(original).damaged) {
      recoveryAvailable = true;
      // Keep every damaged generation, including an older recovery copy.
      let suffix = 0;
      let key = QUEUE_RECOVERY_KEY;
      let previous = target.getItem(key);
      while (previous !== null && previous !== original) {
        key = `${QUEUE_RECOVERY_KEY}.${++suffix}`;
        previous = target.getItem(key);
      }
      if (previous === null) target.setItem(key, original);
      if (target.getItem(key) !== original) return { saved: false, recoveryAvailable };
    }
    target.setItem(QUEUE_STORAGE_KEY, raw);
    return { saved: true, recoveryAvailable };
  } catch {
    // Queue execution can continue, but the caller must keep the failure visible.
    return { saved: false, recoveryAvailable };
  }
}
