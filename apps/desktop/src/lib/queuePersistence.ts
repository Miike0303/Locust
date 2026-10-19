import type { TranslationStartParams } from "./api";
import type { QueueItem } from "../stores/queueStore";
import { browserStorage, type StringStorage } from "./storage";

export const QUEUE_STORAGE_KEY = "locust.queue.v1";
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
export interface PersistedQueue {
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

/** Pure, versioned snapshot: never serialize the store or its runtime fields. */
export function serializeQueue<T extends PersistedQueue>(state: T): string | null {
  try {
    return JSON.stringify({ version: 1, items: state.items.map(copyItem),
      translationParams: state.translationParams, history: state.history.slice(0, MAX_QUEUE_HISTORY) });
  } catch {
    return null;
  }
}

/** Pure restore; interrupted runs require an explicit retry and never auto-start. */
export function restoreQueue(raw: string | null): PersistedQueue {
  try {
    if (raw === null) return emptyQueue();
    const value: unknown = JSON.parse(raw);
    if (!record(value) || value.version !== 1 || !Array.isArray(value.items) || !value.items.every(validItem) ||
      !validParams(value.translationParams) || !Array.isArray(value.history) || !value.history.every(validHistory)) return emptyQueue();
    return {
      items: value.items.map(item => {
        const copy = copyItem(item);
        return ["extracting", "translating", "validating"].includes(copy.status)
          ? { ...copy, status: "error", error: "queue.interrupted" } : copy;
      }),
      translationParams: value.translationParams,
      history: value.history.slice(0, MAX_QUEUE_HISTORY),
    };
  } catch {
    return emptyQueue();
  }
}

export function readQueue(storage?: StringStorage | null): PersistedQueue {
  try {
    return restoreQueue((storage === undefined ? browserStorage() : storage)?.getItem(QUEUE_STORAGE_KEY) ?? null);
  } catch {
    return emptyQueue();
  }
}
export function writeQueue(state: PersistedQueue, storage?: StringStorage | null): void {
  try {
    const raw = serializeQueue(state);
    if (raw !== null) (storage === undefined ? browserStorage() : storage)?.setItem(QUEUE_STORAGE_KEY, raw);
  } catch {
    // Blocked storage and quota errors must never interrupt queue execution.
  }
}
