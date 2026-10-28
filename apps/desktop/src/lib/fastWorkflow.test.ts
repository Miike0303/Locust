import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, test } from "node:test";
import { createElement, useEffect, useRef, useState } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ProjectInfo } from "./api";
import type { StringStorage } from "./storage";
import {
  FAST_WORKFLOW_STAGES,
  canResumeFastWorkflow,
  createFastWorkflowReceipt,
  estimateWorkflowRemainingSeconds,
  fastWorkflowOptionsHash,
  fastWorkflowProjectKey,
  hasUncertainWorkflowWrite,
  parseFastWorkflowReceipt,
  readFastWorkflowReceipt,
  runFastWorkflow,
  writeFastWorkflowReceipt,
  type FastWorkflowAdapter,
  type FastWorkflowContext,
  type FastWorkflowOptions,
  type FastWorkflowReceipt,
  type FastWorkflowStageId,
  type FastWorkflowStageResult,
} from "./fastWorkflow";

const project: ProjectInfo = {
  path: "C:/games/fixture", format_id: "renpy", name: "Fixture",
  database_path: "C:/projects/fixture/source.db", supported_modes: ["replace", "add"],
};
const options: FastWorkflowOptions = {
  sourceLang: "ja", targetLang: "es", englishPivot: true, providerId: "google",
  injectMode: "direct", compression: "fast", outputDir: "",
  patchOutputPath: "C:/exports/fixture-run-1.zip", pivotOutputPath: "C:/projects/fixture/run-1-pivot.db",
};
const pivotProject: ProjectInfo = { ...project, database_path: options.pivotOutputPath };
const activeStages = (r: FastWorkflowReceipt) => r.stages.filter(s => s.status !== "skipped").map(s => s.id);
const stage = (r: FastWorkflowReceipt, id: FastWorkflowStageId) => r.stages.find(s => s.id === id)!;
const fresh = (overrides: Partial<FastWorkflowOptions> = {}) => createFastWorkflowReceipt(project, { ...options, ...overrides }, 100);
const clone = <T>(value: T): T => structuredClone(value);
function resultFor(id: FastWorkflowStageId): FastWorkflowStageResult {
  switch (id) {
    case "translate": return { jobId: "english-job" };
    case "pivot": return { pivotDatabasePath: options.pivotOutputPath, project: pivotProject };
    case "translateTarget": return { jobId: "spanish-job" };
    case "validate": return { validationIssues: 0 };
    case "inject": return { injectedRoot: project.path, backupId: "backup-1", injectionIds: ["transaction-1"] };
    case "pack": return { outputPath: options.patchOutputPath, patchId: "patch-1" };
  }
}
function adapterHarness() {
  const executed: FastWorkflowStageId[] = [];
  const reconciled: FastWorkflowStageId[] = [];
  const adapter: FastWorkflowAdapter = {
    async execute(id) { executed.push(id); return resultFor(id); },
    async reconcile(id) { reconciled.push(id); return { outcome: "blocked" }; },
  };
  return { adapter, executed, reconciled };
}
async function run(receipt: FastWorkflowReceipt, adapter: FastWorkflowAdapter, extra: {
  controller?: AbortController;
  onChange?: (receipt: FastWorkflowReceipt) => void;
  now?: () => number;
} = {}) {
  const snapshots: FastWorkflowReceipt[] = [];
  const controller = extra.controller ?? new AbortController();
  const result = await runFastWorkflow(receipt, adapter, {
    signal: controller.signal,
    now: extra.now ?? (() => 1000),
    onChange(value) { snapshots.push(clone(value)); extra.onChange?.(value); },
  });
  return { result, snapshots };
}
function completedPrefix(before: FastWorkflowStageId): FastWorkflowReceipt {
  const receipt = fresh();
  for (const id of FAST_WORKFLOW_STAGES) {
    if (id === before) break;
    Object.assign(stage(receipt, id), { status: "done", startedAt: 100, durationMs: 10, result: resultFor(id) });
  }
  return receipt;
}
function memoryStorage() {
  const data = new Map<string, string>();
  const storage: StringStorage = {
    getItem: key => data.get(key) ?? null,
    setItem: (key, value) => { data.set(key, value); },
    removeItem: key => { data.delete(key); },
  };
  return { storage, data };
}
function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}

test("direct workflow translates, validates, injects and packs; provider completion alone is not workflow completion", async () => {
  const initial = fresh({ englishPivot: false });
  const original = clone(initial);
  const h = adapterHarness();
  const { result, snapshots } = await run(initial, h.adapter);
  assert.deepEqual(h.executed, ["translate", "validate", "inject", "pack"]);
  assert.deepEqual(h.reconciled, []);
  assert.equal(result.status, "completed");
  assert.equal(stage(result, "pivot").status, "skipped");
  assert.equal(stage(result, "translateTarget").status, "skipped");
  const providerFinished = snapshots.find(r => stage(r, "translate").status === "done")!;
  assert.equal(providerFinished.status, "running");
  assert.equal(stage(providerFinished, "validate").status, "pending");
  assert.deepEqual(initial, original, "running never mutates the caller's receipt");
});

test("English pivot workflow saves every stage in order and keeps its planned project identity", async () => {
  const h = adapterHarness();
  const { result } = await run(fresh(), h.adapter);
  assert.deepEqual(h.executed, [...FAST_WORKFLOW_STAGES]);
  assert.equal(result.status, "completed");
  assert.deepEqual(stage(result, "pivot").result?.project, pivotProject);
  assert.equal(result.projectKey, fastWorkflowProjectKey(project));
  assert.ok(canResumeFastWorkflow(result, fastWorkflowProjectKey(pivotProject), options));
});

for (const englishPivot of [false, true]) {
  for (const completed of activeStages(fresh({ englishPivot }))) {
    test(`resume after ${completed} (${englishPivot ? "pivot" : "direct"}) preserves completed receipts without repeating them`, async () => {
      const controller = new AbortController();
      const h = adapterHarness();
      const { result: interrupted } = await run(fresh({ englishPivot }), h.adapter, {
        controller,
        onChange(value) { if (stage(value, completed).status === "done") controller.abort(); },
      });
      const saved = parseFastWorkflowReceipt(JSON.stringify(interrupted));
      assert.ok(saved);
      const done = saved.stages.filter(s => s.status === "done");
      const resumed = adapterHarness();
      const { result } = await run(saved, resumed.adapter);
      assert.equal(result.status, "completed");
      assert.deepEqual(resumed.executed, activeStages(fresh({ englishPivot })).slice(done.length));
      assert.deepEqual(resumed.reconciled, []);
      for (const receipt of done) assert.deepEqual(stage(result, receipt.id), receipt);
      if (completed === "translate" && englishPivot) {
        assert.equal(resumed.executed[0], "pivot");
        assert.ok(!resumed.executed.includes("translate"), "finished English is not sent to the provider again");
      }
    });
  }
}

for (const failed of FAST_WORKFLOW_STAGES) {
  test(`failure in ${failed} preserves earlier receipts and prevents later stages`, async () => {
    const h = adapterHarness();
    h.adapter.execute = async id => {
      h.executed.push(id);
      if (id === failed) throw new Error(`${failed} refused`);
      return resultFor(id);
    };
    const { result } = await run(fresh(), h.adapter);
    assert.equal(result.status, "failed");
    assert.equal(result.error, `${failed} refused`);
    assert.equal(stage(result, failed).status, "failed");
    const index = FAST_WORKFLOW_STAGES.indexOf(failed);
    assert.deepEqual(h.executed, FAST_WORKFLOW_STAGES.slice(0, index + 1));
    for (const id of FAST_WORKFLOW_STAGES.slice(0, index)) {
      assert.equal(stage(result, id).status, "done");
      assert.deepEqual(stage(result, id).result, resultFor(id));
    }
    for (const id of FAST_WORKFLOW_STAGES.slice(index + 1)) assert.equal(stage(result, id).status, "pending");
    assert.ok(parseFastWorkflowReceipt(JSON.stringify(result)), "a failed receipt remains resumable");
  });
}

test("progress and substage checkpoints are durable before a dependent stage runs", async () => {
  const { storage } = memoryStorage();
  const h = adapterHarness();
  h.adapter.execute = async (id, receipt, context) => {
    if (id === "translate") {
      context.checkpoint({ jobId: "saved-job" });
      context.onProgress({ completed: 3, total: 8, etaSeconds: 12 });
      const saved = readFastWorkflowReceipt(receipt.projectKey, storage)!;
      assert.equal(stage(saved, id).status, "running");
      assert.equal(stage(saved, id).result?.jobId, "saved-job");
      assert.deepEqual(stage(saved, id).progress, { completed: 3, total: 8, etaSeconds: 12 });
      return;
    }
    const saved = readFastWorkflowReceipt(receipt.projectKey, storage)!;
    const previous = FAST_WORKFLOW_STAGES[FAST_WORKFLOW_STAGES.indexOf(id) - 1];
    assert.equal(stage(saved, previous).status, "done");
    return resultFor(id);
  };
  const { result } = await run(fresh(), h.adapter, { onChange: value => writeFastWorkflowReceipt(value, storage) });
  assert.equal(result.status, "completed");
  assert.equal(stage(result, "translate").result?.jobId, "saved-job");
});

test("options hash covers every workflow choice and canonical comparison rejects colliding inputs", () => {
  const receipt = fresh();
  const changes: Partial<FastWorkflowOptions>[] = [
    { sourceLang: "en", englishPivot: false }, { targetLang: "fr" }, { englishPivot: false },
    { providerId: "deepl" }, { injectMode: "add" }, { compression: "store" },
    { outputDir: "C:/copy" }, { patchOutputPath: "C:/exports/other.zip" }, { pivotOutputPath: "C:/projects/other.db" },
  ];
  for (const change of changes) {
    const changed = { ...options, ...change };
    assert.notEqual(fastWorkflowOptionsHash(changed), receipt.optionsHash);
    assert.equal(canResumeFastWorkflow(receipt, receipt.projectKey, changed), false);
    const forgedHash = { ...receipt, optionsHash: fastWorkflowOptionsHash(changed) };
    assert.equal(canResumeFastWorkflow(forgedHash, receipt.projectKey, changed), false);
  }
  assert.ok(canResumeFastWorkflow(receipt, receipt.projectKey, { ...options, providerId: " google ", patchOutputPath: ` ${options.patchOutputPath} ` }));
});

test("changed game, format or database rejects resume; only the recorded pivot project is accepted", () => {
  const receipt = fresh();
  for (const changed of [{ path: "C:/games/other" }, { format_id: "html-game" }, { database_path: "C:/projects/other.db" }]) {
    assert.equal(canResumeFastWorkflow(receipt, fastWorkflowProjectKey({ ...project, ...changed }), options), false);
  }
  assert.equal(canResumeFastWorkflow(receipt, fastWorkflowProjectKey(pivotProject), options), false, "an uncreated planned output is not an open project");
  Object.assign(stage(receipt, "pivot"), { result: resultFor("pivot") });
  assert.equal(canResumeFastWorkflow(receipt, fastWorkflowProjectKey(pivotProject), options), true);
  for (const changed of [{ path: "C:/games/other" }, { format_id: "html-game" }, { database_path: "C:/projects/other.db" }]) {
    const invalid = clone(receipt);
    stage(invalid, "pivot").result!.project = { ...pivotProject, ...changed };
    assert.equal(canResumeFastWorkflow(invalid, fastWorkflowProjectKey(stage(invalid, "pivot").result!.project!), options), false);
  }
});

for (const id of ["inject", "pack"] as const) {
  for (const outcome of ["done", "retry", "blocked"] as const) {
    test(`uncertain ${id}: ${outcome} reconciliation precedes and controls any repeated write`, async () => {
      const receipt = completedPrefix(id);
      Object.assign(stage(receipt, id), { status: "running", startedAt: 200, result: id === "inject" ? { injectionIds: ["before"] } : undefined });
      assert.equal(hasUncertainWorkflowWrite(receipt), true);
      const calls: string[] = [];
      const h = adapterHarness();
      h.adapter.reconcile = async (current, saved) => {
        calls.push(`reconcile:${current}`);
        assert.equal(stage(saved, current).status, "running");
        assert.equal(stage(saved, current).startedAt, 200);
        return { outcome, result: resultFor(id) };
      };
      h.adapter.execute = async current => { calls.push(`execute:${current}`); return resultFor(current); };
      const { result } = await run(receipt, h.adapter);
      assert.equal(calls[0], `reconcile:${id}`);
      assert.equal(calls.filter(call => call === `execute:${id}`).length, outcome === "retry" ? 1 : 0);
      for (const prior of receipt.stages.filter(s => s.status === "done")) assert.deepEqual(stage(result, prior.id), prior);
      if (outcome === "blocked") {
        assert.equal(result.status, "failed");
        assert.equal(result.error, "fastWorkflow.reconcileBlocked");
        assert.deepEqual(calls, [`reconcile:${id}`]);
        assert.equal(hasUncertainWorkflowWrite(result), true);
      } else {
        assert.equal(result.status, "completed");
        assert.equal(stage(result, id).status, "done");
        assert.equal(hasUncertainWorkflowWrite(result), false);
      }
    });
  }
  test(`failed ${id} with persisted intent still reconciles instead of repeating immediately`, async () => {
    const receipt = completedPrefix(id);
    Object.assign(stage(receipt, id), { status: "failed", startedAt: 200, error: "connection lost" });
    const h = adapterHarness();
    const { result } = await run(receipt, h.adapter);
    assert.deepEqual(h.reconciled, [id]);
    assert.deepEqual(h.executed, []);
    assert.equal(result.status, "failed");
    assert.equal(hasUncertainWorkflowWrite(result), true);
  });
  test(`${id} reconciliation network failure never authorizes a repeated write`, async () => {
    const receipt = completedPrefix(id);
    Object.assign(stage(receipt, id), { status: "running", startedAt: 200 });
    const h = adapterHarness();
    h.adapter.reconcile = async () => { throw new Error("status unavailable"); };
    const { result } = await run(receipt, h.adapter);
    assert.deepEqual(h.executed, []);
    assert.equal(result.error, "status unavailable");
    assert.equal(hasUncertainWorkflowWrite(result), true);
  });
}

test("cancel before starting sends no request", async () => {
  const controller = new AbortController();
  controller.abort();
  const h = adapterHarness();
  const { result } = await run(fresh(), h.adapter, { controller });
  assert.equal(result.status, "cancelled");
  assert.deepEqual(h.executed, []);
});

test("cancel translation waits for adapter cancellation and leaves later stages pending", async () => {
  const controller = new AbortController();
  const started = deferred<FastWorkflowContext>();
  const h = adapterHarness();
  let cancellationObserved = false;
  h.adapter.execute = async (id, _receipt, context) => {
    h.executed.push(id);
    context.checkpoint({ jobId: "cancel-me" });
    started.resolve(context);
    await new Promise<void>((_resolve, reject) => context.signal.addEventListener("abort", () => {
      cancellationObserved = true;
      reject(new Error("cancelled"));
    }, { once: true }));
  };
  const running = run(fresh(), h.adapter, { controller });
  await started.promise;
  controller.abort();
  const { result } = await running;
  assert.equal(cancellationObserved, true);
  assert.equal(result.status, "cancelled");
  assert.equal(stage(result, "translate").status, "pending");
  assert.equal(stage(result, "translate").result?.jobId, "cancel-me");
  assert.deepEqual(h.executed, ["translate"]);
  assert.equal(stage(result, "pivot").status, "pending");
});

test("cancel during noncancellable injection waits, saves its success, then stops before pack", async () => {
  const controller = new AbortController();
  const entered = deferred<void>();
  const finish = deferred<FastWorkflowStageResult>();
  const h = adapterHarness();
  h.adapter.execute = async id => {
    h.executed.push(id);
    assert.equal(id, "inject");
    entered.resolve();
    return finish.promise;
  };
  let latest: FastWorkflowReceipt | undefined;
  const running = run(completedPrefix("inject"), h.adapter, { controller, onChange(value) { latest = value; } });
  await entered.promise;
  controller.abort();
  assert.equal(latest?.status, "running", "a cancellation request does not release a still-writing stage");
  finish.resolve(resultFor("inject"));
  const { result } = await running;
  assert.equal(result.status, "cancelled");
  assert.equal(stage(result, "inject").status, "done");
  assert.deepEqual(stage(result, "inject").result, resultFor("inject"));
  assert.equal(stage(result, "pack").status, "pending");
  assert.equal(hasUncertainWorkflowWrite(result), false);
  assert.deepEqual(h.executed, ["inject"]);
});

test("cancel during uncertain-write reconciliation prevents a newly authorized retry", async () => {
  const receipt = completedPrefix("inject");
  Object.assign(stage(receipt, "inject"), { status: "running", startedAt: 200 });
  const controller = new AbortController();
  const h = adapterHarness();
  h.adapter.reconcile = async () => { controller.abort(); return { outcome: "retry" }; };
  const { result } = await run(receipt, h.adapter, { controller });
  assert.equal(result.status, "cancelled");
  assert.deepEqual(h.executed, []);
  assert.equal(hasUncertainWorkflowWrite(result), true);
});

test("a failed noncancellable write remains uncertain even when cancellation was requested", async () => {
  const controller = new AbortController();
  const h = adapterHarness();
  h.adapter.execute = async () => { controller.abort(); throw new Error("connection lost"); };
  const { result } = await run(completedPrefix("inject"), h.adapter, { controller });
  assert.equal(result.status, "cancelled");
  assert.equal(stage(result, "inject").status, "failed");
  assert.equal(hasUncertainWorkflowWrite(result), true);
  assert.equal(stage(result, "pack").status, "pending");
});

test("receipts persist under source and planned pivot identities and reject foreign or corrupt data", async () => {
  const { storage, data } = memoryStorage();
  const { result } = await run(fresh(), adapterHarness().adapter);
  writeFastWorkflowReceipt(result, storage);
  const persisted = parseFastWorkflowReceipt(JSON.stringify(result));
  assert.deepEqual(readFastWorkflowReceipt(result.projectKey, storage), persisted);
  assert.deepEqual(readFastWorkflowReceipt(fastWorkflowProjectKey(pivotProject), storage), persisted);
  assert.equal(readFastWorkflowReceipt("foreign-project", storage), null);
  const sourceKey = [...data.keys()].find(key => key.endsWith(result.projectKey))!;
  storage.setItem(sourceKey, "{broken");
  assert.equal(readFastWorkflowReceipt(result.projectKey, storage), null);
  assert.throws(() => readFastWorkflowReceipt(result.projectKey, storage, true), /fastWorkflow.reconcileBlocked/);
});

test("receipt parser rejects malformed options, stage order, impossible completion and corrupted evidence", () => {
  const malformed: Array<(r: any) => void> = [
    r => { r.version = 2; }, r => { r.project.path = ""; }, r => { r.projectKey = "other"; },
    r => { r.optionsHash = "stale"; }, r => { r.options.targetLang = ""; },
    r => { r.options.compression = "turbo"; }, r => { r.createdAt = null; },
    r => { r.stages.pop(); }, r => { r.stages.reverse(); },
    r => { r.stages[0].status = "skipped"; }, r => { r.stages[1].status = "done"; },
    r => { r.status = "completed"; }, r => { r.stages[0].durationMs = -1; },
    r => { r.stages[0].startedAt = -1; }, r => { r.stages[0].result = { jobId: 4 }; },
    r => { r.stages[0].result = { injectionIds: [42] }; }, r => { r.stages[0].result = { project: {} }; },
    r => { r.stages[0].progress = { completed: -1, total: 3 }; },
    r => { r.stages[0].progress = { completed: 1, total: 3, etaSeconds: -1 }; },
    r => { r.stages[0].result = { validationIssues: "zero" }; },
  ];
  assert.equal(parseFastWorkflowReceipt("null"), null);
  assert.equal(parseFastWorkflowReceipt("not json"), null);
  for (const mutate of malformed) {
    const receipt = clone(fresh());
    mutate(receipt);
    assert.equal(parseFastWorkflowReceipt(JSON.stringify(receipt)), null, JSON.stringify(receipt));
  }
});

test("unavailable storage refuses writes and does not turn read failures into resumable work", () => {
  const broken: StringStorage = {
    getItem() { throw new Error("denied"); }, setItem() { throw new Error("quota exceeded"); }, removeItem() {},
  };
  assert.equal(readFastWorkflowReceipt(fresh().projectKey, broken), null);
  assert.throws(() => readFastWorkflowReceipt(fresh().projectKey, broken, true), /fastWorkflow.storageUnavailable/);
  assert.throws(() => writeFastWorkflowReceipt(fresh(), broken), /fastWorkflow.storageUnavailable/);
});

test("durability failure before stage intent prevents every provider and filesystem write", async () => {
  const h = adapterHarness();
  const broken: StringStorage = { getItem: () => null, setItem() { throw new Error("quota"); }, removeItem() {} };
  await assert.rejects(run(fresh(), h.adapter, { onChange: receipt => writeFastWorkflowReceipt(receipt, broken) }), /fastWorkflow.storageUnavailable/);
  assert.deepEqual(h.executed, []);
});

test("ETA is unknown before measurement, uses current translation rate, measured future work and completes at zero", () => {
  const receipt = fresh({ englishPivot: false });
  assert.equal(estimateWorkflowRemainingSeconds(receipt, 1000), null);
  Object.assign(stage(receipt, "translate"), { status: "running", startedAt: 0, progress: { completed: 4, total: 10 } });
  assert.equal(estimateWorkflowRemainingSeconds(receipt, 8000), 12);
  stage(receipt, "translate").progress!.etaSeconds = 7;
  assert.equal(estimateWorkflowRemainingSeconds(receipt, 8000), 7);
  Object.assign(stage(receipt, "translate"), { status: "done", durationMs: 20_000 });
  Object.assign(stage(receipt, "validate"), { status: "running", startedAt: 20_000 });
  assert.equal(estimateWorkflowRemainingSeconds(receipt, 25_000), 55);
  receipt.status = "completed";
  assert.equal(estimateWorkflowRemainingSeconds(receipt, 25_000), 0);
});

test("pivot ETA includes the future target translation using the measured English pass", () => {
  const receipt = completedPrefix("translateTarget");
  stage(receipt, "translate").durationMs = 40_000;
  stage(receipt, "pivot").durationMs = 2_000;
  Object.assign(stage(receipt, "translateTarget"), { status: "running", startedAt: 0, progress: { completed: 1, total: 4 } });
  assert.equal(estimateWorkflowRemainingSeconds(receipt, 4000), 75);
  assert.ok(Number.isFinite(estimateWorkflowRemainingSeconds(receipt, 4000)));
});

// The view uses the real locale catalog and React server renderer; no browser,
// provider or filesystem endpoint is touched by stage-state rendering.
// Transport tests below import the actual adapter. Only RPCs and the application's
// shared ownership gates are replaced, preserving its real orchestration logic.
type Rpc = (...args: any[]) => unknown;
const transport = {
  calls: [] as Array<[string, ...any[]]>,
  responses: {} as Record<string, unknown | Rpc>,
  subscriptions: [] as Array<{ jobId: string; handlers: any }>,
  unsubscribed: 0,
  workflowStore: null as any,
  modalOptions: undefined as FastWorkflowOptions | undefined,
  react: { useEffect, useRef, useState },
  projectState: {
    project: project as ProjectInfo | null,
    projectTransitionInProgress: false,
    confirmedOpenProjectKey: fastWorkflowProjectKey(project),
    setProject(value: ProjectInfo) {
      transport.projectState.project = value;
      transport.projectState.confirmedOpenProjectKey = fastWorkflowProjectKey(value);
    },
  },
  editor: { isTranslating: false, setTranslating(value: boolean) { transport.editor.isTranslating = value; } },
  queue: { isRunning: false },
  async rpc(name: string, args: any[]) {
    transport.calls.push([name, ...args]);
    assert.ok(name in transport.responses, `unexpected endpoint: ${name}`);
    const response = transport.responses[name];
    return typeof response === "function" ? (response as Rpc)(...args) : clone(response);
  },
};
(globalThis as any).__fastWorkflowTransport = transport;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.replace(/\\/g, "/").endsWith("/components/FastWorkflowModal.tsx")) {
      const stubs: Record<string, string> = {
        "../stores/fastWorkflowStore": "export const useFastWorkflowStore=Object.assign(()=>h.workflowStore.getState(),{getState:()=>h.workflowStore.getState()});",
        react: "export const useEffect=h.react.useEffect;export const useRef=h.react.useRef;export const useState=initial=>initial===null&&h.modalOptions!==undefined?[h.modalOptions,()=>{}]:h.react.useState(initial);",
      };
      if (stubs[specifier]) return {
        shortCircuit: true,
        url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__fastWorkflowTransport;" + stubs[specifier])}`,
      };
    }
    if (context.parentURL?.replace(/\\/g, "/").endsWith("/stores/fastWorkflowStore.ts")) {
      const apiNames = ["cancelTranslation", "getInjectionStatus", "getPatchRecordings", "getStats", "inject", "openProjectDb", "patchPack", "patchStatus", "patchVerify", "runPivot", "startTranslation", "validate"];
      const stubs: Record<string, string> = {
        "../lib/api": apiNames.map(name => `export const ${name}=(...args)=>h.rpc('${name}',args);`).join("\n") + "\nexport const validationKindLabel=kind=>typeof kind==='string'?kind:Object.keys(kind)[0];",
        "../lib/ws": "export function subscribeToJob(jobId,handlers){h.subscriptions.push({jobId,handlers});return()=>{h.unsubscribed++;};}",
        "./projectStore": "export const useProjectStore=Object.assign(selector=>selector?selector(h.projectState):h.projectState,{getState:()=>h.projectState});",
        "./editorStore": "export const useEditorStore={getState:()=>h.editor};",
        "./queueStore": "export const useQueueStore={getState:()=>h.queue};",
        "../lib/openProjectFlow": "export const openProjectTransition=operation=>operation();",
        "../lib/queryClient": "export const queryClient={};export const invalidateProjectData=async()=>{};",
      };
      if (stubs[specifier]) return {
        shortCircuit: true,
        url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__fastWorkflowTransport;" + stubs[specifier])}`,
      };
    }
    return nextResolve(specifier, context);
  },
});
const { fastWorkflowAdapter, useFastWorkflowStore } = await import("../stores/fastWorkflowStore");
transport.workflowStore = useFastWorkflowStore;
const { setLocale, t } = await import("./i18n");
const { default: FastWorkflowModal, FastWorkflowView } = await import("../components/FastWorkflowModal");
after(() => { setLocale("en"); hooks.deregister(); Reflect.deleteProperty(globalThis, "__fastWorkflowTransport"); });
for (const locale of ["en", "es"] as const) {
  test(`${locale}: stage view shows pending, running, done, failed and skipped states with live progress`, () => {
    setLocale(locale);
    const receipt = fresh({ englishPivot: false });
    Object.assign(stage(receipt, "translate"), { status: "done", durationMs: 1000 });
    Object.assign(stage(receipt, "validate"), { status: "done", durationMs: 100 });
    Object.assign(stage(receipt, "inject"), { status: "running", startedAt: Date.now(), progress: { completed: 3, total: 8 } });
    const html = renderToStaticMarkup(createElement(FastWorkflowView, { receipt, isRunning: true }));
    assert.match(html, /aria-live="polite"/);
    for (const state of ["pending", "running", "done", "skipped"] as const) assert.ok(html.includes(t(`fastWorkflow.stageStatus.${state}`)), `missing ${state}`);
    assert.ok(html.includes("3"));
    assert.ok(html.includes("8"));
    stage(receipt, "inject").status = "failed";
    receipt.status = "failed";
    const failure = renderToStaticMarkup(createElement(FastWorkflowView, { receipt, isRunning: false, error: "Synthetic write refused" }));
    assert.ok(failure.includes(t("fastWorkflow.stageStatus.failed")));
    assert.ok(failure.includes("Synthetic write refused"));
  });
  test(`${locale}: provider-done view does not claim workflow completion; full completion exposes the patch`, () => {
    setLocale(locale);
    const receipt = completedPrefix("pivot");
    receipt.status = "running";
    const partial = renderToStaticMarkup(createElement(FastWorkflowView, { receipt, isRunning: true }));
    assert.ok(!partial.includes(t("fastWorkflow.completed")));
    for (const id of FAST_WORKFLOW_STAGES) Object.assign(stage(receipt, id), { status: "done", result: resultFor(id), durationMs: 10 });
    receipt.status = "completed";
    const done = renderToStaticMarkup(createElement(FastWorkflowView, { receipt, isRunning: false }));
    assert.ok(done.includes(t("fastWorkflow.completed")));
    assert.ok(done.includes(options.patchOutputPath));
  });
}

function modalMarkup(props: { project?: ProjectInfo | null; open?: boolean } = {}): string {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  client.setQueryData(["providers"], [{ id: "google", name: "Google", is_free: true, requires_api_key: false, configured: true }]);
  client.setQueryData(["config"], { providers: {} });
  try {
    return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(FastWorkflowModal, {
      open: props.open ?? true, project: props.project === undefined ? project : props.project, onClose() {},
      sampleSources: ["これは日本語のゲームの台詞です。主人公はここから旅を始めます。"],
    })));
  } finally { client.clear(); }
}
function resetModalState(receipt: FastWorkflowReceipt | null = null, running = false, cancelling = false) {
  transport.modalOptions = undefined;
  useFastWorkflowStore.setState({ receipt, isRunning: running, cancelRequested: cancelling, error: null });
}

test("actual modal is empty when closed or no project is open", () => {
  resetModalState();
  assert.equal(modalMarkup({ open: false }), "");
  assert.equal(modalMarkup({ project: null }), "");
});

for (const locale of ["en", "es"] as const) {
  test(`${locale}: actual modal has bounded labelled dialog and Google/fast defaults`, () => {
    setLocale(locale); resetModalState();
    const html = modalMarkup();
    assert.match(html, /role="dialog"/);
    assert.match(html, /aria-modal="true"/);
    assert.match(html, /aria-labelledby="[^"]+"/);
    assert.match(html, /max-h-/);
    assert.match(html, /overflow-y-auto/);
    assert.match(html, /aria-live="polite"/);
    assert.match(html, /<option value="google" selected=""/);
    assert.match(html, /<option value="fast" selected=""/);
    for (const key of ["title", "source", "target", "provider", "patchOutput", "englishPivot", "start"] as const) {
      assert.ok(html.includes(t(`fastWorkflow.${key}`)), key);
    }
    assert.ok(!transport.calls.some(([name]) => name === "startTranslation"));
  });

  test(`${locale}: actual modal renders saved running/cancelling work with disabled edits and close`, () => {
    setLocale(locale);
    const receipt = directReceipt("inject");
    Object.assign(stage(receipt, "inject"), { status: "running", startedAt: Date.now() });
    receipt.status = "running";
    resetModalState(receipt, true);
    const html = modalMarkup();
    assert.match(html, /<fieldset disabled=""/);
    assert.ok(html.includes(t("fastWorkflow.cancel")));
    assert.ok(!html.includes(t("fastWorkflow.completed")));
    const close = (html.match(/<button\b[^>]*>/g) ?? []).find(button => button.includes(`aria-label="${t("common.close")}"`));
    assert.ok(close?.includes('disabled=""'));
    useFastWorkflowStore.setState({ cancelRequested: true });
    const cancelling = modalMarkup();
    assert.ok(cancelling.includes(t("fastWorkflow.cancelling")));
    const cancelButton = (cancelling.match(/<button\b[^>]*>[\s\S]*?<\/button>/g) ?? []).find(button => button.includes(t("fastWorkflow.cancelling")));
    assert.ok(cancelButton?.includes('disabled=""'));
    resetModalState();
  });

  test(`${locale}: actual modal shows resumable cancellation and completed patch states`, () => {
    setLocale(locale);
    const receipt = directReceipt("inject");
    receipt.status = "cancelled";
    resetModalState(receipt);
    const cancelled = modalMarkup();
    assert.ok(cancelled.includes(t("fastWorkflow.workflowStatus.cancelled")));
    assert.ok(cancelled.includes(t("fastWorkflow.resume")));
    assert.ok(cancelled.includes(t("fastWorkflow.resumeHint")));
    for (const id of ["inject", "pack"] as const) Object.assign(stage(receipt, id), { status: "done", result: resultFor(id), durationMs: 10 });
    receipt.status = "completed";
    resetModalState(receipt);
    const completed = modalMarkup();
    assert.ok(completed.includes(t("fastWorkflow.completed")));
    assert.ok(completed.includes(options.patchOutputPath));
    assert.ok(!completed.includes(t("fastWorkflow.resumeHint")));
    resetModalState();
  });
}

test("actual modal restores a project-bound receipt when reopened from its planned pivot alias", async () => {
  const { storage } = memoryStorage();
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  try {
    Object.defineProperty(globalThis, "localStorage", { configurable: true, value: storage });
    const receipt = completedPrefix("translateTarget");
    receipt.status = "cancelled";
    writeFastWorkflowReceipt(receipt, storage);
    resetModalState();
    useFastWorkflowStore.getState().load(pivotProject);
    assert.equal(useFastWorkflowStore.getState().receipt?.projectKey, fastWorkflowProjectKey(project));
    const html = modalMarkup({ project: pivotProject });
    assert.ok(html.includes(t("fastWorkflow.resumeHint")));
    assert.match(html, /data-stage="translate" data-status="done"/);
    assert.match(html, /data-stage="pivot" data-status="done"/);
    assert.ok(html.includes(options.pivotOutputPath));
  } finally {
    if (descriptor) Object.defineProperty(globalThis, "localStorage", descriptor);
    else Reflect.deleteProperty(globalThis, "localStorage");
    resetModalState();
  }
});

test("partially cleared modal form remains renderable and explains invalid options", () => {
  resetModalState();
  transport.modalOptions = { ...options, targetLang: "", patchOutputPath: "" };
  try {
    const html = modalMarkup();
    assert.ok(html.includes(t("fastWorkflow.invalidOptions")));
    assert.match(html, /role="dialog"/);
    const startButton = (html.match(/<button\b[^>]*>[\s\S]*?<\/button>/g) ?? []).find(button => button.includes(t("fastWorkflow.start")));
    assert.ok(startButton?.includes('disabled=""'));
  } finally { resetModalState(); }
});

function resetTransport() {
  transport.calls = []; transport.subscriptions = []; transport.unsubscribed = 0;
  transport.editor.isTranslating = false; transport.queue.isRunning = false;
  transport.projectState.projectTransitionInProgress = false;
  transport.projectState.setProject(project);
  transport.responses = {
    startTranslation: { job_id: "transport-job" }, cancelTranslation: undefined,
    getStats: { total: 4, pending: 0, error: 0, translated: 4, reviewed: 0, approved: 0, total_cost_usd: 0 },
    runPivot: { database_path: options.pivotOutputPath, entries: 4 },
    openProjectDb: {
      project_path: project.path, format_id: project.format_id, project_name: project.name,
      database_path: options.pivotOutputPath, supported_modes: project.supported_modes, total_strings: 4,
    },
    validate: { validation: { issues_found: 0, by_kind: {}, issues: [] }, fonts: [], font_issues: [] },
    getInjectionStatus: { game_root: project.path, pending: null, applied: [] },
    getPatchRecordings: { languages: [options.targetLang], roots: { [options.targetLang]: project.path } },
    patchStatus: { status: "not_patched", injections: [], injection_pending: false },
    inject: {
      mode: "direct", languages_processed: [options.targetLang], languages_failed: [],
      backup_id: "latest-backup", pristine_backup_id: "pristine-backup", strings_written: 4,
      files_modified: 1, reports: {}, outcomes: [[options.targetLang, { Recorded: { files: 1 } }]],
    },
    patchPack: {
      output_path: options.patchOutputPath, patch_id: "packed-id", files_packed: 1,
      language: options.targetLang, engine: project.format_id, recorded_root: project.path,
    },
    patchVerify: {
      outcome: "AlreadyApplied", conflicts: [], backup_compromised: false,
      manifest: {
        patch_id: "packed-id", language: options.targetLang, engine: project.format_id,
        created_at: new Date().toISOString(), files: [{ path: "script.rpy" }],
      },
    },
  };
}
function directReceipt(before: FastWorkflowStageId) {
  const receipt = completedPrefix(before);
  receipt.options.englishPivot = false;
  receipt.optionsHash = fastWorkflowOptionsHash(receipt.options);
  for (const id of ["pivot", "translateTarget"] as const) stage(receipt, id).status = "skipped";
  return receipt;
}
function contextFor(receipt: FastWorkflowReceipt, id: FastWorkflowStageId, controller = new AbortController()): FastWorkflowContext {
  return {
    signal: controller.signal,
    checkpoint(result) { stage(receipt, id).result = { ...stage(receipt, id).result, ...result }; },
    onProgress(progress) { stage(receipt, id).progress = progress; },
  };
}
function uncertain(id: "inject" | "pack") {
  const receipt = directReceipt(id);
  Object.assign(stage(receipt, id), {
    status: "running", startedAt: Date.now() - 500,
    result: id === "inject" ? { writeStarted: true, injectionIds: ["old-transaction"] } : { writeStarted: true, outputPath: options.patchOutputPath },
  });
  return receipt;
}
function appliedInjection(extra: Record<string, unknown> = {}) {
  return {
    transaction_id: "new-transaction", mode: "direct", language: options.targetLang,
    changed_files: 1, applied_at: new Date().toISOString(), ...extra,
  };
}
async function waitForSubscription() {
  for (let i = 0; i < 20 && transport.subscriptions.length === 0; i++) await Promise.resolve();
  assert.equal(transport.subscriptions.length, 1, "translation subscribes after starting its job");
  return transport.subscriptions[0];
}

test("actual adapter saves inject baseline before its write and packs with fast compression plus the exact pristine backup", async () => {
  resetTransport();
  transport.responses.getInjectionStatus = { game_root: project.path, pending: null, applied: [appliedInjection({ transaction_id: "baseline" })] };
  const receipt = directReceipt("inject");
  let writeIntentSeen = false;
  transport.responses.inject = (params: unknown) => {
    assert.deepEqual(stage(receipt, "inject").result?.injectionIds, ["baseline"]);
    assert.equal(stage(receipt, "inject").result?.writeStarted, true);
    writeIntentSeen = true;
    assert.deepEqual(params, {
      project_path: project.path, format_id: "renpy", mode: undefined,
      direct: true, languages: ["es"], output_dir: undefined,
    });
    return { mode: "direct", languages_processed: ["es"], languages_failed: [], backup_id: "backup", pristine_backup_id: "exact-pristine", strings_written: 4, reports: {} };
  };
  const injected = await fastWorkflowAdapter.execute("inject", receipt, contextFor(receipt, "inject"));
  assert.equal(writeIntentSeen, true);
  assert.deepEqual(injected, { injectedRoot: project.path, backupId: "exact-pristine" });
  stage(receipt, "inject").result = { ...stage(receipt, "inject").result, ...injected };
  const packed = await fastWorkflowAdapter.execute("pack", receipt, contextFor(receipt, "pack"));
  assert.deepEqual(transport.calls.find(([name]) => name === "patchPack"), ["patchPack", {
    game_path: project.path, output_path: options.patchOutputPath, languages: ["es"],
    compression: "fast", pristine: true, pristine_backup_id: "exact-pristine",
  }]);
  assert.deepEqual(packed, { outputPath: options.patchOutputPath, patchId: "packed-id" });
});

for (const mode of ["add", "replace"] as const) {
  test(`actual adapter ${mode} injection uses existing mode/output arguments and packs the recorded root`, async () => {
    resetTransport();
    const receipt = directReceipt("inject");
    receipt.options.injectMode = mode;
    receipt.options.outputDir = "C:/translated-copy";
    receipt.optionsHash = fastWorkflowOptionsHash(receipt.options);
    const root = mode === "replace" ? "C:/translated-copy/fixture-es" : project.path;
    transport.responses.inject = { mode, languages_processed: ["es"], languages_failed: [], backup_id: "backup", strings_written: 4, reports: {}, injected_roots: { es: root } };
    transport.responses.getPatchRecordings = { languages: ["es"], roots: { es: root } };
    transport.responses.patchPack = { output_path: options.patchOutputPath, patch_id: "packed-id", files_packed: 1, language: "es", engine: "renpy", recorded_root: root };
    const injected = await fastWorkflowAdapter.execute("inject", receipt, contextFor(receipt, "inject"));
    stage(receipt, "inject").result = { ...stage(receipt, "inject").result, ...injected };
    await fastWorkflowAdapter.execute("pack", receipt, contextFor(receipt, "pack"));
    const injectArgs = transport.calls.find(([name]) => name === "inject")![1];
    assert.equal(injectArgs.mode, mode);
    assert.equal(injectArgs.direct, false);
    assert.equal(injectArgs.output_dir, receipt.options.outputDir);
    assert.equal(injectArgs.require_binary_slot_fit, mode === "replace" ? true : undefined);
    const packArgs = transport.calls.find(([name]) => name === "patchPack")![1];
    assert.equal(packArgs.game_path, root);
    assert.equal(packArgs.compression, "fast");
    assert.equal(packArgs.pristine_backup_id, undefined);
  });
}

for (const report of [
  { languages_processed: [], languages_failed: [["es", "failed"]], strings_written: 0 },
  { languages_processed: ["es"], languages_failed: [["fr", "failed"]], strings_written: 2 },
  { languages_processed: ["es"], languages_failed: [], strings_written: 0 },
]) {
  test(`actual adapter rejects ${report.strings_written ? "partial" : report.languages_failed.length ? "failed" : "empty"} injection instead of packing`, async () => {
    resetTransport();
    transport.responses.inject = { ...report, backup_id: "backup", mode: "direct", reports: {} };
    const { result } = await run(directReceipt("inject"), fastWorkflowAdapter);
    assert.equal(result.error, "fastWorkflow.injectionFailed");
    assert.equal(stage(result, "inject").status, "failed");
    assert.ok(!transport.calls.some(([name]) => name === "patchPack"));
    assert.equal(hasUncertainWorkflowWrite(result), true);
  });
}

test("actual adapter reconciles a newly committed matching injection without injecting again", async () => {
  resetTransport();
  transport.responses.getInjectionStatus = { game_root: project.path, pending: null, applied: [appliedInjection()] };
  const receipt = uncertain("inject");
  const result = await fastWorkflowAdapter.reconcile("inject", receipt, contextFor(receipt, "inject"));
  assert.equal(result.outcome, "done");
  assert.equal(result.result?.injectedRoot, project.path);
  assert.deepEqual(transport.calls.map(([name]) => name), ["getInjectionStatus", "patchStatus", "getPatchRecordings"]);
});

const injectionRejections: Array<[string, Record<string, unknown>]> = [
  ["pre-existing transaction", { transaction_id: "old-transaction" }],
  ["wrong mode", { mode: "add" }], ["wrong language", { language: "fr" }],
  ["empty write", { changed_files: 0 }], ["missing time", { applied_at: null }],
  ["old time", { applied_at: "2000-01-01T00:00:00Z" }],
  ["invalid time", { applied_at: "invalid" }], ["future time", { applied_at: "2999-01-01T00:00:00Z" }],
];
for (const [label, change] of injectionRejections) {
  test(`actual adapter blocks uncertain injection with ${label}`, async () => {
    resetTransport();
    transport.responses.getInjectionStatus = { game_root: project.path, pending: null, applied: [appliedInjection(change)] };
    const receipt = uncertain("inject");
    const result = await fastWorkflowAdapter.reconcile("inject", receipt, contextFor(receipt, "inject"));
    assert.equal(result.outcome, "blocked");
    assert.ok(!transport.calls.some(([name]) => name === "inject" || name === "patchPack"));
  });
}

test("actual adapter blocks pending or ambiguous transactions, unknown status and uncertain Replace", async () => {
  for (const variant of ["pending", "statusPending", "unknown", "multiple", "replace", "noBaseline"]) {
    resetTransport();
    const receipt = uncertain("inject");
    transport.responses.getInjectionStatus = { game_root: project.path, pending: variant === "pending" ? { transaction_id: "pending" } : null, applied: variant === "multiple" ? [appliedInjection(), appliedInjection({ transaction_id: "another" })] : [appliedInjection()] };
    if (variant === "statusPending") transport.responses.patchStatus = { status: "not_patched", injection_pending: true };
    if (variant === "unknown") transport.responses.patchStatus = { status: "unknown", injection_pending: false };
    if (variant === "replace") {
      receipt.options.injectMode = "replace";
      receipt.options.outputDir = "C:/translated-copy";
      receipt.optionsHash = fastWorkflowOptionsHash(receipt.options);
    }
    if (variant === "noBaseline") stage(receipt, "inject").result!.injectionIds = undefined;
    const result = await fastWorkflowAdapter.reconcile("inject", receipt, contextFor(receipt, "inject"));
    assert.equal(result.outcome, "blocked", variant);
    assert.ok(!transport.calls.some(([name]) => name === "inject" || name === "patchPack"));
  }
});

test("actual adapter only retries when durable preflight evidence proves no write was dispatched", async () => {
  for (const id of ["inject", "pack"] as const) {
    resetTransport();
    const receipt = uncertain(id);
    stage(receipt, id).result!.writeStarted = false;
    const result = await fastWorkflowAdapter.reconcile(id, receipt, contextFor(receipt, id));
    assert.equal(result.outcome, "retry");
    assert.deepEqual(transport.calls, []);
  }
});

test("actual adapter verifies the exact uncertain pack destination; ZIP status never substitutes for artifact proof", async () => {
  resetTransport();
  const receipt = uncertain("pack");
  const result = await fastWorkflowAdapter.reconcile("pack", receipt, contextFor(receipt, "pack"));
  assert.equal(result.outcome, "done");
  assert.equal(result.result?.patchId, "packed-id");
  assert.deepEqual(transport.calls.map(([name]) => name), ["getInjectionStatus", "patchStatus", "getPatchRecordings", "patchVerify"]);
  assert.deepEqual(transport.calls[transport.calls.length - 1], ["patchVerify", { game_path: project.path, zip_path: options.patchOutputPath }]);
  assert.ok(!transport.calls.some(([name]) => name === "patchPack"));
});

for (const [label, change] of [
  ["old archive", { created_at: "2000-01-01T00:00:00Z" }], ["invalid archive time", { created_at: "invalid" }],
  ["future archive", { created_at: "2999-01-01T00:00:00Z" }], ["wrong engine", { engine: "html-game" }],
  ["wrong language", { language: "fr" }], ["empty archive", { files: [] }], ["missing patch identity", { patch_id: "" }],
] as const) {
  test(`actual adapter blocks ${label} instead of overwriting an uncertain pack`, async () => {
    resetTransport();
    const verified = transport.responses.patchVerify as any;
    verified.manifest = { ...verified.manifest, ...change };
    const receipt = uncertain("pack");
    const result = await fastWorkflowAdapter.reconcile("pack", receipt, contextFor(receipt, "pack"));
    assert.equal(result.outcome, "blocked");
    assert.ok(!transport.calls.some(([name]) => name === "patchPack"));
  });
}

test("actual adapter uncertain pack read failure retains receipt and does not write again", async () => {
  resetTransport();
  transport.responses.patchVerify = () => { throw new Error("ZIP not found"); };
  const { result } = await run(uncertain("pack"), fastWorkflowAdapter);
  assert.equal(result.status, "failed");
  assert.equal(result.error, "ZIP not found");
  assert.equal(hasUncertainWorkflowWrite(result), true);
  assert.ok(!transport.calls.some(([name]) => name === "patchPack"));
});

test("actual adapter strict pivot checkpoints the new DB then opens it against the original game", async () => {
  resetTransport();
  const receipt = completedPrefix("pivot");
  const result = await fastWorkflowAdapter.execute("pivot", receipt, contextFor(receipt, "pivot"));
  assert.deepEqual(transport.calls, [
    ["runPivot", options.pivotOutputPath, true],
    ["openProjectDb", options.pivotOutputPath, project.path, project.format_id],
  ]);
  assert.equal(stage(receipt, "pivot").result?.pivotDatabasePath, options.pivotOutputPath);
  assert.equal((result as FastWorkflowStageResult).project?.database_path, options.pivotOutputPath);
  assert.equal(transport.projectState.project?.path, project.path);
});

test("actual adapter resumes saved pivot creation without creating or translating English again", async () => {
  resetTransport();
  const receipt = completedPrefix("pivot");
  stage(receipt, "pivot").result = resultFor("pivot");
  const result = await fastWorkflowAdapter.execute("pivot", receipt, contextFor(receipt, "pivot"));
  assert.equal((result as FastWorkflowStageResult).pivotDatabasePath, options.pivotOutputPath);
  assert.ok(transport.calls.every(([name]) => name === "openProjectDb"));
  assert.ok(transport.calls.length >= 1);
});

test("actual adapter provider completion checks pending rows before permitting strict pivot", async () => {
  resetTransport();
  transport.responses.getStats = { total: 4, pending: 1, error: 0 };
  const receipt = fresh();
  const controller = new AbortController();
  const executing = fastWorkflowAdapter.execute("translate", receipt, contextFor(receipt, "translate", controller));
  const subscription = await waitForSubscription();
  subscription.handlers.onStarted({ total: 4 });
  subscription.handlers.onBatchCompleted({ completed: 4, total: 4 });
  subscription.handlers.onCompleted({ total_translated: 4, remaining: 0 });
  await assert.rejects(executing, /fastWorkflow.failedTranslation/);
  assert.equal(stage(receipt, "translate").result?.providerCompleted, true);
  assert.ok(!transport.calls.some(([name]) => name === "runPivot"));
});

test("actual adapter translation cancellation keeps ownership until terminal confirmation", async () => {
  resetTransport();
  const receipt = fresh();
  const controller = new AbortController();
  let finished = false;
  const executing = fastWorkflowAdapter.execute("translate", receipt, contextFor(receipt, "translate", controller));
  executing.then(() => { finished = true; }, () => { finished = true; });
  const subscription = await waitForSubscription();
  assert.equal(stage(receipt, "translate").result?.jobId, "transport-job");
  controller.abort();
  await Promise.resolve();
  assert.equal(finished, false);
  assert.deepEqual(transport.calls.find(([name]) => name === "cancelTranslation"), ["cancelTranslation", "transport-job"]);
  subscription.handlers.onFailed({ error: "cancelled" });
  await assert.rejects(executing, /fastWorkflow.cancelled/);
  assert.equal(transport.unsubscribed, 1);
  assert.equal(stage(receipt, "translate").result?.jobId, undefined, "terminal cancellation permits a later pending-only retry");
});

test("actual adapter noncancellable injection completes after Cancel and the runner omits packing", async () => {
  resetTransport();
  const controller = new AbortController();
  const entered = deferred<void>();
  const finish = deferred<unknown>();
  const report = transport.responses.inject;
  transport.responses.inject = () => { entered.resolve(); return finish.promise; };
  const executing = run(directReceipt("inject"), fastWorkflowAdapter, { controller });
  await entered.promise;
  controller.abort();
  finish.resolve(report);
  const { result } = await executing;
  assert.equal(result.status, "cancelled");
  assert.equal(stage(result, "inject").status, "done");
  assert.equal(stage(result, "inject").result?.injectedRoot, project.path);
  assert.ok(!transport.calls.some(([name]) => name === "patchPack"));
});

test("actual validation stage preserves advisory counts and displays them after completion", async () => {
  resetTransport();
  transport.responses.validate = {
    validation: {
      issues_found: 2, by_kind: { IdenticalToSource: 1, InconsistentTranslation: 1 },
      issues: [{ kind: "IdenticalToSource" }, { kind: { InconsistentTranslation: { variants: 2 } } }],
    }, fonts: [], font_issues: [],
  };
  const receipt = directReceipt("validate");
  const result = await fastWorkflowAdapter.execute("validate", receipt, contextFor(receipt, "validate"));
  assert.deepEqual(result, { validationIssues: 2 });
  Object.assign(stage(receipt, "validate"), { status: "done", result });
  for (const locale of ["en", "es"] as const) {
    setLocale(locale);
    const html = renderToStaticMarkup(createElement(FastWorkflowView, { receipt, isRunning: false }));
    assert.ok(html.includes(t("fastWorkflow.validationAdvisory", { count: 2 })));
  }
});

test("actual validation blocks structural, unknown and inconsistent validation summaries before injection", async () => {
  for (const validation of [
    { issues_found: 1, by_kind: { MissingPlaceholder: 1 }, issues: [{ kind: { MissingPlaceholder: { placeholder: "{name}" } } }] },
    { issues_found: 1, by_kind: { NewUnknownKind: 1 }, issues: [] },
    { issues_found: 1, by_kind: {}, issues: [] },
    { issues_found: 1, by_kind: { IdenticalToSource: 2 }, issues: [] },
  ]) {
    resetTransport();
    transport.responses.validate = { validation, fonts: [], font_issues: [] };
    const { result } = await run(directReceipt("validate"), fastWorkflowAdapter);
    assert.equal(result.error, "fastWorkflow.validationFailed");
    assert.equal(stage(result, "validate").status, "failed");
    assert.ok(!transport.calls.some(([name]) => name === "inject"));
  }
});

test("actual validation blocks both font diagnostics and missing glyph coverage", async () => {
  for (const report of [
    { fonts: [], font_issues: [{ font_path: "font.ttf", message: "Font unreadable" }] },
    { fonts: [{ has_full_coverage: false }], font_issues: [] },
  ]) {
    resetTransport();
    transport.responses.validate = { validation: { issues_found: 0, by_kind: {}, issues: [] }, ...report };
    const receipt = directReceipt("validate");
    await assert.rejects(fastWorkflowAdapter.execute("validate", receipt, contextFor(receipt, "validate")), /fastWorkflow.validationFailed/);
    assert.equal(stage(receipt, "validate").result?.validationIssues, 1);
    assert.ok(!transport.calls.some(([name]) => name === "inject"));
  }
});
