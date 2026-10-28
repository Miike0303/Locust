import type { PatchCompression, ProjectInfo } from "./api";
import type { InjectUiMode } from "./injectModes";
import type { StringStorage } from "./storage";

export interface FastWorkflowOptions {
  sourceLang: string;
  targetLang: string;
  englishPivot: boolean;
  providerId: string;
  injectMode: InjectUiMode;
  compression: PatchCompression;
  outputDir: string;
  patchOutputPath: string;
  pivotOutputPath: string;
}

export const FAST_WORKFLOW_STAGES = ["translate", "pivot", "translateTarget", "validate", "inject", "pack"] as const;
export type FastWorkflowStageId = typeof FAST_WORKFLOW_STAGES[number];
export type FastWorkflowStageStatus = "pending" | "running" | "done" | "failed" | "skipped";
export interface FastWorkflowProgress { completed: number; total: number; etaSeconds?: number }
export interface FastWorkflowStageResult {
  jobId?: string;
  pivotDatabasePath?: string;
  project?: ProjectInfo;
  injectedRoot?: string;
  backupId?: string;
  outputPath?: string;
  injectionIds?: string[];
  patchId?: string;
  validationIssues?: number;
  providerCompleted?: boolean;
  writeStarted?: boolean;
}
export interface FastWorkflowStageReceipt {
  id: FastWorkflowStageId;
  status: FastWorkflowStageStatus;
  startedAt?: number;
  durationMs?: number;
  progress?: FastWorkflowProgress;
  error?: string;
  result?: FastWorkflowStageResult;
}
export interface FastWorkflowReceipt {
  version: 1;
  projectKey: string;
  project: ProjectInfo;
  optionsHash: string;
  options: FastWorkflowOptions;
  stages: FastWorkflowStageReceipt[];
  status: "idle" | "running" | "cancelled" | "failed" | "completed";
  createdAt: number;
  updatedAt: number;
  error?: string;
}
export interface FastWorkflowContext {
  signal: AbortSignal;
  onProgress: (progress: FastWorkflowProgress) => void;
  /** Durable substage evidence, saved before proceeding to the next RPC. */
  checkpoint: (result: Partial<FastWorkflowStageResult>) => void;
}
export interface FastWorkflowAdapter {
  execute: (stage: FastWorkflowStageId, receipt: FastWorkflowReceipt, context: FastWorkflowContext) => Promise<FastWorkflowStageResult | void>;
  reconcile: (stage: FastWorkflowStageId, receipt: FastWorkflowReceipt, context: FastWorkflowContext) => Promise<{
    outcome: "done" | "retry" | "blocked";
    result?: FastWorkflowStageResult;
  }>;
}

// Keep this module independent of stores and transports for deterministic recovery tests.
export function fastWorkflowProjectKey(project: ProjectInfo): string {
  return JSON.stringify([project.database_path ?? null, project.path, project.format_id]);
}

function canonicalOptions(options: FastWorkflowOptions): string {
  return JSON.stringify([
    options.sourceLang.trim(), options.targetLang.trim(), options.englishPivot,
    options.providerId.trim(), options.injectMode, options.compression,
    options.outputDir.trim(), options.patchOutputPath.trim(), options.pivotOutputPath.trim(),
  ]);
}

export function fastWorkflowOptionsHash(options: FastWorkflowOptions): string {
  const text = canonicalOptions(options);
  let hash = 2166136261;
  for (let i = 0; i < text.length; i++) hash = Math.imul(hash ^ text.charCodeAt(i), 16777619);
  return (hash >>> 0).toString(16).padStart(8, "0");
}

export function createFastWorkflowReceipt(project: ProjectInfo, options: FastWorkflowOptions, now = Date.now()): FastWorkflowReceipt {
  if (!validOptions(options)) throw new Error("fastWorkflow.invalidOptions");
  const copy = { ...options };
  return {
    version: 1, projectKey: fastWorkflowProjectKey(project), project: { ...project },
    optionsHash: fastWorkflowOptionsHash(copy), options: copy,
    stages: FAST_WORKFLOW_STAGES.map(id => ({
      id, status: !copy.englishPivot && (id === "pivot" || id === "translateTarget") ? "skipped" : "pending",
    })),
    status: "idle", createdAt: now, updatedAt: now,
  };
}

export function canResumeFastWorkflow(receipt: FastWorkflowReceipt, projectKey: string, options: FastWorkflowOptions): boolean {
  // Also compare canonical inputs: a small identifier hash must never authorize a collision.
  if (receipt.optionsHash !== fastWorkflowOptionsHash(options) || canonicalOptions(receipt.options) !== canonicalOptions(options)) return false;
  if (projectKey === receipt.projectKey) return true;
  const pivot = receipt.stages.find(stage => stage.id === "pivot");
  const planned = pivot?.result?.project;
  return !!(receipt.options.englishPivot && planned &&
    planned.path === receipt.project.path && planned.format_id === receipt.project.format_id &&
    planned.database_path === pivot?.result?.pivotDatabasePath &&
    fastWorkflowProjectKey(planned) === projectKey);
}

export function hasUncertainWorkflowWrite(receipt: FastWorkflowReceipt): boolean {
  return receipt.stages.some(stage => (stage.id === "inject" || stage.id === "pack") &&
    stage.startedAt !== undefined && stage.status !== "done" && stage.status !== "skipped");
}

const STORAGE_PREFIX = "locust.fastWorkflow.v1:";
function storageOrDefault(storage?: StringStorage): StringStorage {
  if (storage) return storage;
  if (typeof localStorage === "undefined") throw new Error("fastWorkflow.storageUnavailable");
  return localStorage;
}

function validProject(value: unknown): value is ProjectInfo {
  if (!value || typeof value !== "object") return false;
  const p = value as ProjectInfo;
  return typeof p.path === "string" && !!p.path && typeof p.format_id === "string" && !!p.format_id && typeof p.name === "string" &&
    (p.database_path === undefined || p.database_path === null || typeof p.database_path === "string");
}
function validOptions(value: unknown): value is FastWorkflowOptions {
  if (!value || typeof value !== "object") return false;
  const o = value as FastWorkflowOptions;
  return [o.sourceLang, o.targetLang, o.providerId, o.patchOutputPath].every(v => typeof v === "string" && !!v.trim()) &&
    typeof o.outputDir === "string" && typeof o.pivotOutputPath === "string" && typeof o.englishPivot === "boolean" &&
    ["replace", "add", "direct"].includes(o.injectMode) && ["default", "fast", "store"].includes(o.compression) &&
    o.sourceLang !== o.targetLang && (!o.englishPivot || (o.sourceLang === "ja" && o.targetLang !== "en" && !!o.pivotOutputPath.trim())) &&
    (o.injectMode !== "replace" || !!o.outputDir.trim());
}

/** Malformed/old receipts never authorize skipped stages or writes. */
export function parseFastWorkflowReceipt(raw: string): FastWorkflowReceipt | null {
  try {
    const r = JSON.parse(raw) as FastWorkflowReceipt;
    if (r.version !== 1 || !validProject(r.project) || !validOptions(r.options) ||
      r.projectKey !== fastWorkflowProjectKey(r.project) || r.optionsHash !== fastWorkflowOptionsHash(r.options) ||
      !["idle", "running", "cancelled", "failed", "completed"].includes(r.status) ||
      !Number.isFinite(r.createdAt) || !Number.isFinite(r.updatedAt) ||
      !Array.isArray(r.stages) || r.stages.length !== FAST_WORKFLOW_STAGES.length) return null;
    let unfinished = false;
    let activeSeen = false;
    for (let i = 0; i < r.stages.length; i++) {
      const stage = r.stages[i];
      const skip = !r.options.englishPivot && (stage.id === "pivot" || stage.id === "translateTarget");
      if (stage.id !== FAST_WORKFLOW_STAGES[i] || !["pending", "running", "done", "failed", "skipped"].includes(stage.status) ||
        (stage.status === "skipped") !== skip ||
        (stage.startedAt !== undefined && (!Number.isFinite(stage.startedAt) || stage.startedAt < 0)) ||
        (stage.durationMs !== undefined && (!Number.isFinite(stage.durationMs) || stage.durationMs < 0))) return null;
      if (stage.status === "done" && unfinished) return null;
      if ((stage.status === "running" || stage.status === "failed") && activeSeen) return null;
      if (stage.status !== "done" && stage.status !== "skipped") {
        if (unfinished && stage.status !== "pending") return null;
        if (stage.status === "running" || stage.status === "failed") activeSeen = true;
      }
      if (stage.status !== "done" && stage.status !== "skipped") unfinished = true;
      if (stage.progress !== undefined && (!stage.progress || typeof stage.progress !== "object" ||
        !Number.isFinite(stage.progress.completed) || stage.progress.completed < 0 ||
        !Number.isFinite(stage.progress.total) || stage.progress.total < 0 ||
        (stage.progress.etaSeconds !== undefined && (!Number.isFinite(stage.progress.etaSeconds) || stage.progress.etaSeconds < 0)))) return null;
      if (stage.result !== undefined) {
        const result = stage.result;
        if (!result || typeof result !== "object" ||
          [result.jobId, result.pivotDatabasePath, result.injectedRoot, result.backupId, result.outputPath, result.patchId].some(v => v !== undefined && typeof v !== "string") ||
          (result.project !== undefined && !validProject(result.project)) ||
          (result.validationIssues !== undefined && (!Number.isInteger(result.validationIssues) || result.validationIssues < 0)) ||
          (result.providerCompleted !== undefined && typeof result.providerCompleted !== "boolean") ||
          (result.writeStarted !== undefined && typeof result.writeStarted !== "boolean") ||
          (result.injectionIds !== undefined && (!Array.isArray(result.injectionIds) || result.injectionIds.some(v => typeof v !== "string")))) return null;
      }
    }
    if (r.status === "completed" && unfinished) return null;
    return r;
  } catch { return null; }
}

export function readFastWorkflowReceipt(projectKey: string, storage?: StringStorage, strict = false): FastWorkflowReceipt | null {
  try {
    const target = storageOrDefault(storage);
    const read = (key: string) => {
      try { return target.getItem(STORAGE_PREFIX + key); }
      catch { throw new Error("fastWorkflow.storageUnavailable"); }
    };
    let raw = read(projectKey);
    if (raw === null) return null;
    const alias = JSON.parse(raw) as { rootKey?: unknown };
    if (alias && typeof alias.rootKey === "string") {
      raw = read(alias.rootKey);
      if (raw === null) throw new Error("fastWorkflow.reconcileBlocked");
    }
    const receipt = parseFastWorkflowReceipt(raw);
    if (receipt && canResumeFastWorkflow(receipt, projectKey, receipt.options)) return receipt;
    throw new Error("fastWorkflow.reconcileBlocked");
  } catch (error) {
    if (strict) throw new Error(error instanceof Error && error.message === "fastWorkflow.storageUnavailable"
      ? "fastWorkflow.storageUnavailable" : "fastWorkflow.reconcileBlocked");
    return null;
  }
}

/** Fail before another mutation if recovery evidence cannot be persisted. */
export function writeFastWorkflowReceipt(receipt: FastWorkflowReceipt, storage?: StringStorage): void {
  try {
    const target = storageOrDefault(storage);
    const raw = JSON.stringify(receipt);
    target.setItem(STORAGE_PREFIX + receipt.projectKey, raw);
    const pivotProject = receipt.stages.find(stage => stage.id === "pivot")?.result?.project;
    if (pivotProject && fastWorkflowProjectKey(pivotProject) !== receipt.projectKey) {
      target.setItem(STORAGE_PREFIX + fastWorkflowProjectKey(pivotProject), JSON.stringify({ rootKey: receipt.projectKey }));
    }
  } catch { throw new Error("fastWorkflow.storageUnavailable"); }
}

/**
 * Receipt-driven runner. A provider terminal event completes only its own stage.
 * Running/failed writes always reconcile; lack of proof never permits a replay.
 */
export async function runFastWorkflow(
  initial: FastWorkflowReceipt,
  adapter: FastWorkflowAdapter,
  { signal, onChange, now = Date.now }: { signal: AbortSignal; onChange: (receipt: FastWorkflowReceipt) => void; now?: () => number },
): Promise<FastWorkflowReceipt> {
  let receipt: FastWorkflowReceipt = structuredClone(initial);
  const publish = (patch: Partial<FastWorkflowReceipt> = {}) => {
    receipt = { ...receipt, ...patch, updatedAt: now() };
    onChange(structuredClone(receipt));
  };
  const updateStage = (id: FastWorkflowStageId, patch: Partial<FastWorkflowStageReceipt>) => {
    receipt = { ...receipt, stages: receipt.stages.map(s => s.id === id ? { ...s, ...patch } : s) };
    publish();
  };
  publish({ status: "running", error: undefined });
  for (const initialStage of receipt.stages) {
    const id = initialStage.id;
    let stage = receipt.stages.find(s => s.id === id)!;
    if (stage.status === "done" || stage.status === "skipped") continue;
    if (signal.aborted) { publish({ status: "cancelled" }); return receipt; }
    const context: FastWorkflowContext = {
      signal,
      onProgress: progress => updateStage(id, { progress }),
      checkpoint: result => {
        const previous = receipt.stages.find(s => s.id === id)!;
        updateStage(id, { result: { ...previous.result, ...result } });
      },
    };
    const uncertain = (id === "inject" || id === "pack") && stage.startedAt !== undefined;
    const startedAt = stage.startedAt ?? now();
    try {
      // Persist intent *before* execute/reconcile can send a request.
      updateStage(id, { status: "running", startedAt, error: undefined });
      let result: FastWorkflowStageResult | void;
      if (uncertain) {
        const reconciled = await adapter.reconcile(id, receipt, context);
        if (reconciled.outcome === "blocked") throw new Error("fastWorkflow.reconcileBlocked");
        if (reconciled.outcome === "done") result = reconciled.result;
        else {
          if (signal.aborted) { publish({ status: "cancelled" }); return receipt; }
          result = await adapter.execute(id, receipt, context);
        }
      } else result = await adapter.execute(id, receipt, context);
      stage = receipt.stages.find(s => s.id === id)!;
      updateStage(id, { status: "done", durationMs: Math.max(0, now() - startedAt), result: { ...stage.result, ...result }, error: undefined });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      // Completed earlier stages remain intact, including a finished provider pass.
      const cancelled = signal.aborted && message !== "fastWorkflow.storageUnavailable";
      updateStage(id, { status: cancelled && id !== "inject" && id !== "pack" ? "pending" : "failed", error: cancelled ? undefined : message });
      publish({ status: cancelled ? "cancelled" : "failed", error: cancelled ? undefined : message });
      return receipt;
    }
  }
  publish({ status: "completed", error: undefined });
  return receipt;
}

/** Approximate future work using the live translation rate and measured stage costs. */
export function estimateWorkflowRemainingSeconds(receipt: FastWorkflowReceipt, now = Date.now()): number | null {
  if (receipt.status === "completed") return 0;
  const current = receipt.stages.find(stage => stage.status === "running");
  const measured = receipt.stages.filter(stage => stage.status === "done" && stage.durationMs !== undefined);
  const sum = measured.reduce((total, stage) => total + stage.durationMs!, 0) / 1000;
  const average = measured.length ? sum / measured.length : null;
  let estimate = 0;
  let known = false;
  for (const stage of receipt.stages) {
    if (stage.status === "done" || stage.status === "skipped") continue;
    if (stage === current && (stage.id === "translate" || stage.id === "translateTarget")) {
      const progress = stage.progress;
      const elapsed = stage.startedAt === undefined ? 0 : Math.max(0, now - stage.startedAt) / 1000;
      const eta = progress?.etaSeconds ?? (progress && progress.completed > 0 && progress.total > 0
        ? elapsed / progress.completed * Math.max(0, progress.total - progress.completed) : null);
      if (eta !== null && Number.isFinite(eta)) { estimate += Math.max(0, eta); known = true; continue; }
    }
    const sameKind = measured.find(s => s.id === stage.id ||
      (["translate", "translateTarget"].includes(s.id) && ["translate", "translateTarget"].includes(stage.id)));
    const seconds = sameKind?.durationMs !== undefined ? sameKind.durationMs / 1000 : average;
    if (seconds !== null) {
      const elapsed = stage === current && stage.startedAt !== undefined ? Math.max(0, now - stage.startedAt) / 1000 : 0;
      estimate += Math.max(0, seconds - elapsed);
      known = true;
    }
  }
  return known ? Math.ceil(estimate) : null;
}
