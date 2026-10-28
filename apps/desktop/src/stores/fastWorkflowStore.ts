import { create } from "zustand";
import {
  cancelTranslation, getInjectionStatus, getPatchRecordings, getStats, inject,
  openProjectDb, patchPack, patchStatus, patchVerify, runPivot, startTranslation, validate, validationKindLabel,
  type ProjectInfo, type ProgressEventCompleted,
} from "../lib/api";
import {
  canResumeFastWorkflow, createFastWorkflowReceipt, readFastWorkflowReceipt,
  hasUncertainWorkflowWrite, runFastWorkflow, writeFastWorkflowReceipt,
  type FastWorkflowAdapter, type FastWorkflowContext, type FastWorkflowOptions,
  type FastWorkflowReceipt, type FastWorkflowStageId, type FastWorkflowStageResult,
} from "../lib/fastWorkflow";
import { availableInjectModes } from "../lib/injectModes";
import { classifyInjectReport } from "../lib/injectOutcome";
import { openProjectTransition } from "../lib/openProjectFlow";
import { projectInfoAfterPivotOpen } from "../lib/pivot";
import { qaIsAdvisoryKind } from "../lib/qaDashboard";
import { draftProjectKey } from "../lib/projectScope";
import { invalidateProjectData, queryClient } from "../lib/queryClient";
import { subscribeToJob } from "../lib/ws";
import { useEditorStore } from "./editorStore";
import { useProjectStore } from "./projectStore";
import { useQueueStore } from "./queueStore";

interface FastWorkflowStore {
  receipt: FastWorkflowReceipt | null;
  isRunning: boolean;
  cancelRequested: boolean;
  error: string | null;
  load: (project: ProjectInfo) => void;
  start: (project: ProjectInfo, options: FastWorkflowOptions, fresh?: boolean) => Promise<void>;
  cancel: () => void;
}

let activeController: AbortController | null = null;

const failure = (key: string): Error => new Error(`fastWorkflow.${key}`);
const abortError = (): Error => Object.assign(new Error("fastWorkflow.cancelled"), { name: "AbortError" });
const stageResult = (receipt: FastWorkflowReceipt, id: FastWorkflowStageId) =>
  receipt.stages.find(stage => stage.id === id)?.result;
const samePath = (a: string, b: string): boolean =>
  a.replace(/\\/g, "/").replace(/\/$/, "") === b.replace(/\\/g, "/").replace(/\/$/, "");

/** Every request rechecks the current identity, including after awaited reads. */
function assertScope(receipt: FastWorkflowReceipt): ProjectInfo {
  const state = useProjectStore.getState();
  if (!state.project || state.projectTransitionInProgress ||
    state.confirmedOpenProjectKey !== draftProjectKey(state.project) ||
    !canResumeFastWorkflow(receipt, draftProjectKey(state.project), receipt.options)) {
    throw failure("projectChanged");
  }
  return state.project;
}

function assertNotCancelled(context: FastWorkflowContext): void {
  if (context.signal.aborted) throw abortError();
}

/** A saved pivot is reopened by database path; extraction is never repeated. */
async function prepareProject(
  stage: FastWorkflowStageId, receipt: FastWorkflowReceipt, context: FastWorkflowContext,
): Promise<void> {
  const current = assertScope(receipt);
  const pivot = stageResult(receipt, "pivot");
  const needsPivot = receipt.options.englishPivot && stage !== "translate" &&
    (stage !== "pivot" || !!pivot?.pivotDatabasePath);
  const expected = needsPivot ? pivot?.project : receipt.project;
  if (!expected) throw failure("projectChanged");
  if (draftProjectKey(current) === draftProjectKey(expected)) return;
  // A pivot stage that saved its DB before closing is handled by its executor.
  assertNotCancelled(context);
  if (!expected.database_path) throw failure("projectChanged");
  const opened = await openProjectTransition(() => openProjectDb(
    expected.database_path!, receipt.project.path, receipt.project.format_id,
  ));
  const project = projectInfoAfterPivotOpen(receipt.project, opened);
  if (!samePath(project.database_path ?? "", expected.database_path)) throw failure("projectChanged");
  useProjectStore.getState().setProject(project);
}

async function proveTranslationComplete(receipt: FastWorkflowReceipt): Promise<void> {
  assertScope(receipt);
  const stats = await getStats();
  assertScope(receipt);
  if (stats.pending !== 0 || stats.error !== 0 || stats.total < 1) throw failure("failedTranslation");
}

async function translateStage(
  id: "translate" | "translateTarget", receipt: FastWorkflowReceipt, context: FastWorkflowContext,
): Promise<FastWorkflowStageResult> {
  const saved = stageResult(receipt, id);
  if (saved?.providerCompleted) {
    await proveTranslationComplete(receipt);
    return saved;
  }
  assertNotCancelled(context);
  const ownerProjectKey = draftProjectKey(assertScope(receipt));
  let jobId = saved?.jobId;
  if (!jobId) {
    const job = await startTranslation({
      provider_id: receipt.options.providerId,
      options: {
        source_lang: id === "translateTarget" ? "en" : receipt.options.sourceLang,
        target_lang: id === "translate" && receipt.options.englishPivot ? "en" : receipt.options.targetLang,
        cost_limit_usd: null, game_context: null, use_glossary: true,
        use_memory: true, skip_approved: true,
      },
    });
    jobId = job.job_id;
    // Keep this ID even if the stream disappears or the app closes.
    try { context.checkpoint({ jobId, providerCompleted: false }); }
    catch (error) {
      // The provider already owns this ID, even when persistence fails.
      await cancelTranslation(jobId).catch(() => {});
      throw error;
    }
  }
  const ownedJobId = jobId;
  const startedAt = Date.now();
  let completed: ProgressEventCompleted;
  try {
    completed = await new Promise<ProgressEventCompleted>((resolve, reject) => {
      let settled = false;
      let close = () => {};
      const finish = (action: () => void) => {
        if (settled) return;
        settled = true;
        context.signal.removeEventListener("abort", cancel);
        close();
        action();
      };
      const cancel = () => {
        // Cancellation is a request, not proof the provider stopped. Keep the
        // subscription until its terminal frame, or preserve the ID on loss.
        // This endpoint targets the captured job ID, independent of whichever
        // project may now be open. It cannot cancel another project's job.
        void cancelTranslation(ownedJobId).catch(() => {
          finish(() => reject(failure("reconcileBlocked")));
        });
      };
      close = subscribeToJob(ownedJobId, {
        onStarted: event => context.onProgress({ completed: 0, total: event.total }),
        onBatchCompleted: event => {
          const elapsed = Math.max(0, Date.now() - startedAt) / 1000;
          context.onProgress({
            completed: event.completed, total: event.total,
            etaSeconds: event.completed > 0 ? elapsed / event.completed * Math.max(0, event.total - event.completed) : undefined,
          });
        },
        onCompleted: event => finish(() => resolve(event)),
        onFailed: () => finish(() => {
          // A terminal failure authorizes a later pending-only provider retry.
          try { context.checkpoint({ jobId: undefined, providerCompleted: false }); }
          catch (error) { reject(error); return; }
          reject(context.signal.aborted ? abortError() : failure("failedTranslation"));
        }),
        onClosed: () => finish(() => reject(failure("reconcileBlocked"))),
      }, "translate", ownerProjectKey);
      context.signal.addEventListener("abort", cancel, { once: true });
      if (context.signal.aborted) cancel();
    });
  } catch (error) {
    // A server restart may discard the old job while its completed rows remain.
    // Only database proof can recover that outcome without another provider run.
    if (!context.signal.aborted) {
      try {
        await proveTranslationComplete(receipt);
        context.checkpoint({ providerCompleted: true });
        return { jobId: ownedJobId, providerCompleted: true };
      } catch { /* Preserve uncertain job evidence for a subsequent resume. */ }
    }
    throw error;
  }
  if (completed.remaining !== undefined && completed.remaining !== 0) {
    context.checkpoint({ jobId: undefined, providerCompleted: false });
    throw context.signal.aborted ? abortError() : failure("failedTranslation");
  }
  context.checkpoint({ providerCompleted: true });
  await proveTranslationComplete(receipt);
  context.onProgress({ completed: completed.total_translated, total: completed.total_translated, etaSeconds: 0 });
  return { jobId: ownedJobId, providerCompleted: true };
}

async function recordingRoot(receipt: FastWorkflowReceipt): Promise<string> {
  assertScope(receipt);
  const recordings = await getPatchRecordings();
  assertScope(receipt);
  const root = recordings.roots?.[receipt.options.targetLang];
  if (!recordings.languages.includes(receipt.options.targetLang) || !root) throw failure("reconcileBlocked");
  return root;
}

/** Adapts the pure stage runner to the existing desktop HTTP/Tauri endpoints. */
export const fastWorkflowAdapter: FastWorkflowAdapter = {
  async execute(stage, receipt, context) {
    await prepareProject(stage, receipt, context);
    assertNotCancelled(context);
    assertScope(receipt);
    const options = receipt.options;
    if (stage === "translate" || stage === "translateTarget") return translateStage(stage, receipt, context);
    if (stage === "pivot") {
      let databasePath = stageResult(receipt, "pivot")?.pivotDatabasePath;
      if (!databasePath) {
        const pivot = await runPivot(options.pivotOutputPath, true);
        databasePath = pivot.database_path;
        context.checkpoint({
          pivotDatabasePath: databasePath,
          project: { ...receipt.project, database_path: databasePath },
        });
      }
      // Finish this noncancellable stage even if Cancel was pressed during pivot.
      assertScope(receipt);
      const opened = await openProjectTransition(() => openProjectDb(
        databasePath, receipt.project.path, receipt.project.format_id,
      ));
      const project = projectInfoAfterPivotOpen(receipt.project, opened);
      useProjectStore.getState().setProject(project);
      context.checkpoint({ pivotDatabasePath: databasePath, project });
      return { pivotDatabasePath: databasePath, project };
    }
    if (stage === "validate") {
      const report = await validate();
      assertScope(receipt);
      const textIssues = report.validation.issues_found;
      const counts = Object.entries(report.validation.by_kind);
      if (!Number.isSafeInteger(textIssues) || textIssues < 0 ||
        counts.some(([, count]) => !Number.isSafeInteger(count) || count < 0)) throw failure("validationFailed");
      const fontIssues = (report.font_issues?.length ?? 0) + report.fonts.filter(font => !font.has_full_coverage).length;
      const issues = textIssues + fontIssues;
      context.checkpoint({ validationIssues: issues });
      const advisory = counts.reduce((sum, [kind, count]) => sum + (qaIsAdvisoryKind(kind) ? count : 0), 0);
      // Only the existing advisory kinds can pass. Unknown/missing kinds and
      // inconsistent summary counts retain their blocking interpretation.
      if (fontIssues > 0 || advisory > textIssues || counts.reduce((sum, [, count]) => sum + count, 0) > textIssues ||
        textIssues - advisory > 0 || counts.some(([kind, count]) => count > 0 && !qaIsAdvisoryKind(kind)) ||
        report.validation.issues.some(issue => !qaIsAdvisoryKind(validationKindLabel(issue.kind)))) {
        throw failure("validationFailed");
      }
      return { validationIssues: issues };
    }
    if (stage === "inject") {
      context.checkpoint({ writeStarted: false });
      const before = await getInjectionStatus(receipt.project.path);
      assertScope(receipt);
      if (before.pending || !before.applied) throw failure("reconcileBlocked");
      context.checkpoint({ injectionIds: before.applied.map(item => item.transaction_id) });
      assertNotCancelled(context);
      assertScope(receipt);
      context.checkpoint({ writeStarted: true });
      const report = await inject({
        project_path: receipt.project.path, format_id: receipt.project.format_id,
        mode: options.injectMode === "direct" ? undefined : options.injectMode,
        direct: options.injectMode === "direct", languages: [options.targetLang],
        output_dir: options.injectMode === "direct" ? undefined : options.outputDir.trim() || undefined,
        ...(options.injectMode === "replace" ? { require_binary_slot_fit: true } : {}),
      });
      const backupId = report.pristine_backup_id ?? report.backup_id;
      const reportedRoot = report.injected_roots?.[options.targetLang];
      // Preserve the response before optional postflight reads can fail.
      context.checkpoint({ backupId, injectedRoot: reportedRoot });
      if (classifyInjectReport(report) !== "success") throw failure("injectionFailed");
      const root = await recordingRoot(receipt);
      if (reportedRoot && !samePath(reportedRoot, root)) throw failure("reconcileBlocked");
      return { injectedRoot: root, backupId };
    }
    context.checkpoint({ writeStarted: false });
    const root = await recordingRoot(receipt);
    const injection = stageResult(receipt, "inject");
    if (!injection?.injectedRoot || !samePath(root, injection.injectedRoot)) throw failure("reconcileBlocked");
    assertNotCancelled(context);
    assertScope(receipt);
    context.checkpoint({ outputPath: options.patchOutputPath, writeStarted: true });
    const report = await patchPack({
      game_path: root, output_path: options.patchOutputPath, languages: [options.targetLang],
      compression: options.compression,
      ...(options.injectMode === "direct" && injection.backupId
        ? { pristine: true, pristine_backup_id: injection.backupId } : {}),
    });
    context.checkpoint({ outputPath: report.output_path, patchId: report.patch_id });
    assertScope(receipt);
    if (report.files_packed < 1 || report.language !== options.targetLang ||
      report.size_bytes < 1 || !report.patch_id || !samePath(report.output_path, options.patchOutputPath) ||
      report.engine !== receipt.project.format_id || !samePath(report.recorded_root, root)) throw failure("packingFailed");
    return { outputPath: report.output_path, patchId: report.patch_id };
  },

  async reconcile(stage, receipt, context) {
    await prepareProject(stage, receipt, context);
    const saved = stageResult(receipt, stage);
    // Explicit preflight intent proves that this executor never dispatched a write.
    if (saved?.writeStarted === false) return { outcome: "retry" };
    const root = stage === "pack" ? stageResult(receipt, "inject")?.injectedRoot : receipt.project.path;
    if (!root) return { outcome: "blocked" };
    assertScope(receipt);
    const injectionStatus = await getInjectionStatus(root);
    assertScope(receipt);
    const status = await patchStatus({ game_path: root });
    assertScope(receipt);
    if (injectionStatus.pending || status.injection_pending || status.status === "interrupted" || status.status === "unknown") {
      return { outcome: "blocked" };
    }
    const recordedRoot = await recordingRoot(receipt);
    if (stage === "inject") {
      // Replace has no committed transaction journal. Its interrupted output
      // cannot be attributed safely, so retain it for inspection.
      if (receipt.options.injectMode === "replace" || !saved?.injectionIds || !injectionStatus.applied) return { outcome: "blocked" };
      const fresh = injectionStatus.applied.filter(item => !saved.injectionIds!.includes(item.transaction_id));
      const startedAt = receipt.stages.find(item => item.id === "inject")?.startedAt ?? Infinity;
      const appliedAt = fresh.length === 1 && fresh[0].applied_at ? Date.parse(fresh[0].applied_at) : NaN;
      if (fresh.length !== 1 || fresh[0].mode !== receipt.options.injectMode ||
        fresh[0].language !== receipt.options.targetLang || fresh[0].changed_files < 1 ||
        !Number.isFinite(appliedAt) || appliedAt < startedAt - 1000 || appliedAt > Date.now() + 30_000 ||
        !samePath(recordedRoot, injectionStatus.game_root)) return { outcome: "blocked" };
      return { outcome: "done", result: { ...saved, injectedRoot: recordedRoot } };
    }
    if (stage !== "pack" || !saved?.outputPath || !samePath(root, recordedRoot)) return { outcome: "blocked" };
    assertScope(receipt);
    // ZIP application status cannot establish pack completion. Verify the exact
    // owned destination and its manifest instead; never retry an unknown pack.
    const verified = await patchVerify({ game_path: root, zip_path: saved.outputPath });
    assertScope(receipt);
    const manifest = verified.manifest as Record<string, unknown> | null;
    const startedAt = receipt.stages.find(item => item.id === "pack")?.startedAt ?? Infinity;
    const createdAt = typeof manifest?.created_at === "string" ? Date.parse(manifest.created_at) : NaN;
    if (!manifest || manifest.engine !== receipt.project.format_id || manifest.language !== receipt.options.targetLang ||
      typeof manifest.patch_id !== "string" || !manifest.patch_id ||
      (saved.patchId !== undefined && saved.patchId !== manifest.patch_id) ||
      !Number.isFinite(createdAt) || createdAt < startedAt - 1000 || createdAt > Date.now() + 30_000 ||
      !Array.isArray(manifest.files) || manifest.files.length < 1 ||
      !["Clean", "AlreadyApplied"].includes(verified.outcome) || verified.backup_compromised || verified.conflicts.length > 0) {
      return { outcome: "blocked" };
    }
    return { outcome: "done", result: { ...saved, patchId: manifest.patch_id } };
  },
};

export const useFastWorkflowStore = create<FastWorkflowStore>((set, get) => ({
  receipt: null, isRunning: false, cancelRequested: false, error: null,
  load(project) {
    if (get().isRunning) return;
    try {
      set({ receipt: readFastWorkflowReceipt(draftProjectKey(project), undefined, true), error: null, cancelRequested: false });
    } catch (error) {
      set({ error: error instanceof Error ? error.message : String(error) });
    }
  },
  cancel() {
    if (!get().isRunning) return;
    set({ cancelRequested: true });
    activeController?.abort();
  },
  async start(project, options, fresh = false) {
    if (get().isRunning || useQueueStore.getState().isRunning || useEditorStore.getState().isTranslating) {
      set({ error: "fastWorkflow.busy" });
      return;
    }
    let receipt: FastWorkflowReceipt;
    try {
      const stored = readFastWorkflowReceipt(draftProjectKey(project), undefined, true);
      if (fresh && stored && (hasUncertainWorkflowWrite(stored) || stored.stages.some(stage =>
        (stage.id === "translate" || stage.id === "translateTarget") && stage.status !== "done" &&
        !!stage.result?.jobId && !stage.result.providerCompleted))) throw failure("reconcileBlocked");
      if (!fresh && stored && !canResumeFastWorkflow(stored, draftProjectKey(project), options)) throw failure("optionsChanged");
      receipt = !fresh && stored ? stored : createFastWorkflowReceipt(project, options);
      assertScope(receipt);
      if (!availableInjectModes(project.supported_modes).includes(options.injectMode)) throw failure("invalidOptions");
      writeFastWorkflowReceipt(receipt);
    } catch (error) {
      set({ error: error instanceof Error ? error.message : String(error) });
      return;
    }
    const controller = new AbortController();
    activeController = controller;
    // Reserve the existing queue/single-job gate for the entire workflow.
    useEditorStore.getState().setTranslating(true);
    set({ receipt, isRunning: true, cancelRequested: false, error: null });
    try {
      const final = await runFastWorkflow(receipt, fastWorkflowAdapter, {
        signal: controller.signal,
        onChange(next) {
          set({ receipt: next, error: next.error ?? null });
          writeFastWorkflowReceipt(next);
        },
      });
      set({ receipt: final, error: final.error ?? null });
    } catch (error) {
      set({ error: error instanceof Error ? error.message : String(error) });
    } finally {
      activeController = null;
      useEditorStore.getState().setTranslating(false);
      set({ isRunning: false });
      const current = useProjectStore.getState().project;
      if (current) await invalidateProjectData(queryClient, draftProjectKey(current), "active");
    }
  },
}));
