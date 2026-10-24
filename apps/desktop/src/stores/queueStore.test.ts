import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { registerHooks } from "node:module";
import { focusManager, onlineManager, QueryObserver } from "@tanstack/react-query";
import { projectScopedQueryEnabled } from "../lib/projectScope.ts";
import { queryClient } from "../lib/queryClient.ts";
import { draftProjectKey } from "./draftStore.ts";
import {
  getProjectOpenChoice, resolveProjectOpenChoice,
  type ProjectOpenChoice, type ProjectOpenChoiceRequest,
} from "../lib/openProjectFlow.ts";
import type { ProjectOpenPreflight, TranslationStartParams } from "../lib/api.ts";

const h = {
  calls: [] as string[], toasts: [] as unknown[][],
  logs: [] as unknown[][], jobError: null as string | null,
  batchErrors: [] as string[], translated: 1, progressTotal: 100, onJob: () => {},
  openedPaths: [] as string[], onOpen: async (_path: string) => {},
  preflight: { kind: "resume_available", database_path: "/fixture/game.locust.db", project_path: "/fixture/game", format_id: "renpy" } as ProjectOpenPreflight,
};
(globalThis as any).__resumeQueue = h;
registerHooks({
  load(url, context, nextLoad) {
    const stubs: Record<string, string> = {
      "/src/lib/api.ts": `const h=globalThis.__resumeQueue;
        const result={project_path:'/fixture/game',project_name:'Fixture',format_id:'renpy',total_strings:1,supported_modes:['replace'],database_path:'/fixture/game.locust.db',extraction_warnings:['Saved warning'],added:0,updated:0,stale_source_reset:0,removed:0,preserved_translations:0};
        export async function preflightProjectOpen(){h.calls.push('preflight');return h.preflight;}
        export async function openProject(path){h.calls.push('extract');h.openedPaths.push(path);await h.onOpen(path);return {...result,project_path:path,database_path:path+'.locust.db'};}
        export async function resumeProject(){h.calls.push('resume');return result;}
        export async function startTranslation(){h.calls.push('translate');return {job_id:'job'};}
        export async function cancelTranslation(){h.calls.push('cancelTranslation');}
        export async function getWsUrl(){return 'ws://fixture/translation';}
        export async function validate(){h.calls.push('validate');return {validation:{issues_found:0}};}`,
      "/src/stores/logStore.ts": `export function addLog(...args){globalThis.__resumeQueue.logs.push(args);}`,
      "/src/stores/toastStore.ts": `export function addToast(...args){globalThis.__resumeQueue.toasts.push(args);}`,
    };
    const path = url.replace(/\\/g, "/");
    const key = Object.keys(stubs).find(key => path.endsWith(key));
    return key ? { format: "module", source: stubs[key], shortCircuit: true } : nextLoad(url, context);
  },
});
// Exercise real waitForJob and WS dispatch so dropped batch events cannot hide
// behind a mock that already knows the expected queue result.
class JobSocket {
  closed = false;
  onmessage: ((event: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  constructor() {
    queueMicrotask(() => {
      for (const error of h.batchErrors) this.emit({ type: "batch_failed", error });
      this.emit({ type: "batch_completed", completed: 99, total: h.progressTotal, cost_so_far: 1 });
      h.onJob();
      this.emit(h.jobError !== null
        ? { type: "failed", error: h.jobError }
        : { type: "completed", total_translated: h.translated, total_cost: 0.25, cost_is_complete: true, duration_secs: 1 });
    });
  }
  emit(data: unknown) { this.onmessage?.({ data: JSON.stringify(data) }); }
  close() { this.closed = true; this.onclose?.(); }
}
(globalThis as any).WebSocket = JobSocket;
const params: TranslationStartParams = {
  provider_id: "mock", options: {
    source_lang: "ja", target_lang: "en", batch_size: 1, max_concurrent: 1,
    cost_limit_usd: null, game_context: null, use_glossary: false, use_memory: false, skip_approved: true,
  },
};
const storageDescriptor = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
const startupItem = {
  id: "interrupted", projectName: "Game", projectPath: "/game", formatId: null, status: "translating",
  error: null, validationIssues: null, validationError: null,
  progress: { completed: 3, total: 10, costSoFar: 0.5, costIsComplete: false, startedAt: 123 },
};
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem(key: string) { return key === "locust.queue.v1" ? JSON.stringify({
    version: 1, items: [startupItem], translationParams: params, history: [],
    isRunning: true, isPanelOpen: true, cancelRequested: true, globalProgress: startupItem.progress,
  }) : null; }, setItem() {}, removeItem() {},
} });
const { useQueueStore: queue } = await import("./queueStore.ts");
if (storageDescriptor) Object.defineProperty(globalThis, "localStorage", storageDescriptor);
else Reflect.deleteProperty(globalThis, "localStorage");
const { useProjectStore: project } = await import("./projectStore.ts");
const { useEditorStore: editor } = await import("./editorStore.ts");
const { setLocale, t } = await import("../lib/i18n/index.ts");
const originalProject = { path: "/other", format_id: "renpy", name: "Other" };
beforeEach(() => {
  const pending = getProjectOpenChoice();
  if (pending) resolveProjectOpenChoice(pending, "cancel");
  queue.setState({ items: [], history: [], isRunning: false, cancelRequested: false, globalProgress: null, translationParams: params });
  editor.setState({ isTranslating: false });
  project.getState().setProject(originalProject);
  h.calls = []; h.toasts = []; h.logs = []; h.jobError = null; h.openedPaths = [];
  h.batchErrors = []; h.translated = 1; h.progressTotal = 100; h.onJob = () => {};
  h.onOpen = async () => {};
  h.preflight = { kind: "resume_available", database_path: "/fixture/game.locust.db", project_path: "/fixture/game", format_id: "renpy" };
  setLocale("en");
});

async function pendingChoice(): Promise<ProjectOpenChoiceRequest> {
  // Yield to the real dynamic import, not a network or browser fixture.
  for (let i = 0; i < 100 && queue.getState().isRunning && !getProjectOpenChoice(); i++) {
    await new Promise<void>(resolve => setImmediate(resolve));
  }
  const request = getProjectOpenChoice();
  assert.ok(request, "queue must wait for a choice instead of extracting/translating");
  assert.ok(!h.calls.includes("translate"));
  return request;
}

for (const choice of ["resume", "refresh", "cancel"] as const satisfies readonly ProjectOpenChoice[]) {
  test(`queue ${choice} waits for the shared choice and cancel never translates`, async () => {
    queue.getState().addItem("/fixture/game");
    const running = queue.getState().startQueue();
    const request = await pendingChoice();
    assert.deepEqual(h.calls, ["preflight"]);
    resolveProjectOpenChoice(request, choice);
    await running;
    if (choice === "cancel") {
      assert.deepEqual(h.calls, ["preflight"]);
      assert.equal(queue.getState().items[0].status, "cancelled");
      assert.equal(project.getState().project, originalProject);
      assert.ok(!h.toasts.some(([level]) => level === "success"));
    } else {
      assert.deepEqual(h.calls, ["preflight", choice === "resume" ? "resume" : "extract", "translate", "validate"]);
      assert.equal(queue.getState().items[0].status, "done");
      assert.equal(project.getState().project?.database_path, "/fixture/game.locust.db");
      assert.deepEqual(project.getState().project?.extraction_warnings, ["Saved warning"]);
    }
    assert.equal(queue.getState().isRunning, false);
    assert.equal(queue.getState().globalProgress, null);
  });
}

test("queue cancellation while choosing closes the choice without opening either project", async () => {
  queue.getState().addItem("/fixture/game");
  queue.getState().addItem("/fixture/next");
  const running = queue.getState().startQueue();
  await pendingChoice();
  queue.getState().cancelQueue();
  await running;
  assert.equal(getProjectOpenChoice(), null);
  assert.deepEqual(h.calls, ["preflight"]);
  assert.deepEqual(queue.getState().items.map(i => i.status), ["cancelled", "pending"]);
  assert.equal(project.getState().project, originalProject);
});

test("cancelled choice aborts only that queue item and is not reported as all completed", async () => {
  queue.getState().addItem("/fixture/game");
  queue.getState().addItem("/fixture/next");
  const running = queue.getState().startQueue();
  resolveProjectOpenChoice(await pendingChoice(), "cancel");
  // First choice is consumed before the second preflight is allowed to proceed.
  const next = await pendingChoice();
  resolveProjectOpenChoice(next, "resume");
  await running;
  assert.deepEqual(queue.getState().items.map(i => i.status), ["cancelled", "done"]);
  assert.equal(h.calls.filter(c => c === "translate").length, 1);
  assert.ok(!h.toasts.some(([, message]) => message === t("queue.toast.allDone")));
});

test("plain queued folder still extracts and translates without a choice", async () => {
  h.preflight = { kind: "extract" };
  queue.getState().addItem("/fixture/game");
  await queue.getState().startQueue();
  assert.equal(getProjectOpenChoice(), null);
  assert.deepEqual(h.calls, ["preflight", "extract", "translate", "validate"]);
  assert.equal(queue.getState().items[0].status, "done");
});

for (const locale of ["en", "es"] as const) {
  test(`queue row localizes terminal provider failure and logs raw detail once in ${locale}`, async () => {
    setLocale(locale);
    h.preflight = { kind: "extract" };
    const raw = "provider error: Claude returned status 403 Forbidden: opaque response  ";
    h.jobError = raw;
    queue.getState().addItem("/fixture/game");
    await queue.getState().startQueue();
    const item = queue.getState().items[0];
    assert.equal(item.status, "error");
    assert.equal(item.error, locale === "es"
      ? "Claude rechazó la clave API. Revísela en Ajustes → Proveedores."
      : "Claude rejected the API key. Check it in Settings → Providers.");
    assert.deepEqual(h.logs.filter(([, , detail]) => detail === raw), [
      ["error", t("activity.queue.itemFailed", { name: "game" }), raw, "queue"],
    ]);
    assert.ok(!h.logs.some(([, message]) => String(message).includes(raw)));
    assert.ok(!h.calls.includes("validate"));
    assert.equal(queue.getState().isRunning, false);
    assert.equal(queue.getState().globalProgress, null);
  });
}

test("queue keeps unknown diagnostics and stream-loss localization", async () => {
  setLocale("es");
  h.preflight = { kind: "extract" };
  for (const raw of ["unknown provider failure", "provider error: OpenAI malformed response: first\nsecond", "ws.jobStreamLost"]) {
    queue.setState({ items: [] });
    h.logs = []; h.jobError = raw;
    queue.getState().addItem("/fixture/game");
    await queue.getState().startQueue();
    assert.equal(queue.getState().items[0].error, raw === "ws.jobStreamLost" ? t("ws.jobStreamLost") : raw);
    assert.equal(h.logs.filter(([, , detail]) => detail === raw).length, 1);
  }
});

for (const locale of ["en", "es"] as const) {
  test(`queue fallback recovery reaching the progress total validates and completes in ${locale}`, async () => {
    setLocale(locale);
    h.preflight = { kind: "extract" };
    h.batchErrors = ["provider error: OpenAI returned status 401 Unauthorized"];
    for (const translated of [100, 101]) {
      h.translated = translated;
      h.calls = []; h.toasts = []; h.logs = [];
      queue.getState().addItem("/fixture/game");
      await queue.getState().startQueue();
      const item = queue.getState().items[0];
      assert.equal(item.status, "done");
      assert.equal(item.error, null);
      assert.equal(item.progress.completed, translated);
      assert.equal(item.progress.total, 100);
      assert.equal(item.validationIssues, 0);
      assert.ok(h.calls.includes("validate"));
      assert.deepEqual(h.toasts, [
        ["success", t("queue.toast.itemDone", { name: "Fixture" })],
        ["success", t("queue.toast.allDone")],
      ]);
      assert.equal(h.logs.filter(([, , detail]) => detail === h.batchErrors[0]).length, 1);
      assert.equal(queue.getState().isRunning, false);
      assert.equal(queue.getState().globalProgress, null);
      queue.getState().clearCompleted();
      assert.equal(queue.getState().items.length, 0);
    }
  });

  test(`queue batch failures with an unknown total retain warning or failure outcomes in ${locale}`, async () => {
    setLocale(locale);
    h.preflight = { kind: "extract" };
    h.batchErrors = ["provider unavailable"];
    h.progressTotal = 0;
    for (const translated of [2, 0]) {
      h.translated = translated;
      h.calls = []; h.toasts = [];
      queue.getState().addItem("/fixture/game");
      await queue.getState().startQueue();
      const item = queue.getState().items[0];
      assert.equal(item.status, "error");
      assert.equal(item.progress.total, 0);
      assert.equal(item.progress.completed, translated);
      assert.ok(!h.calls.includes("validate"));
      assert.equal(h.toasts[h.toasts.length - 1][0], translated > 0 ? "warning" : "error");
      queue.getState().removeItem(item.id);
    }
  });

  for (const translated of [2, 0]) {
    test(`queue ${translated ? "partial" : "total"} batch failure has a non-success row and notices in ${locale}`, async () => {
      setLocale(locale);
      h.preflight = { kind: "extract" };
      h.translated = translated;
      h.batchErrors = [
        "provider error: OpenAI returned status 401 Unauthorized: first opaque body",
        "provider error: Claude returned status 403 Forbidden: last opaque body  ",
      ];
      queue.getState().addItem("/fixture/game");
      await queue.getState().startQueue();
      const item = queue.getState().items[0];
      const reason = locale === "es"
        ? "Claude rechazó la clave API. Revísela en Ajustes → Proveedores."
        : "Claude rejected the API key. Check it in Settings → Providers.";
      const message = translated > 0
        ? t("queue.completedWithErrors", { name: "Fixture", translated, count: 2 })
        : t("queue.translationFailed", { name: "Fixture", count: 2 });
      const level = translated > 0 ? "warning" : "error";
      assert.equal(item.status, "error", "reuse the existing non-success row");
      assert.ok(item.error?.includes(message));
      assert.ok(item.error?.includes(reason));
      assert.ok(item.error?.includes(t("translate.pendingRetryHint")));
      assert.equal(item.progress.completed, translated, "terminal count overrides interim progress");
      assert.equal(item.progress.costSoFar, 0.25);
      assert.ok(h.toasts.some(([severity, text]) => severity === level && text === message));
      assert.ok(h.logs.some(([severity, text]) => severity === level && text === message));
      assert.ok(!h.toasts.some(([severity]) => severity === "success"));
      assert.equal(h.toasts[h.toasts.length - 1][0], level);
      for (const raw of h.batchErrors) {
        assert.equal(h.logs.filter(([, , detail]) => detail === raw).length, 1);
        assert.ok(!h.logs.some(([, summary]) => String(summary).includes(raw)));
      }
      assert.ok(!h.calls.includes("validate"), "validation must not overwrite the translation outcome");
      assert.equal(queue.getState().isRunning, false);
      assert.equal(queue.getState().globalProgress, null);
      queue.getState().clearCompleted();
      assert.equal(queue.getState().items.length, 1, "incomplete work stays visible");
    });
  }
}

test("queue batch failures do not leak into the next item or next run", async () => {
  h.preflight = { kind: "extract" };
  h.batchErrors = ["provider error: OpenAI returned status 401 Unauthorized"];
  h.onJob = () => { h.batchErrors = []; };
  queue.getState().addItem("/fixture/game");
  queue.getState().addItem("/fixture/next");
  await queue.getState().startQueue();
  assert.deepEqual(queue.getState().items.map(i => i.status), ["error", "done"]);
  assert.equal(h.toasts[h.toasts.length - 1][0], "warning");
  queue.getState().addItem("/fixture/last");
  await queue.getState().startQueue();
  assert.equal(queue.getState().items[2].status, "done");
  assert.equal(h.toasts[h.toasts.length - 1][0], "success");
});

test("queue cancellation after batch failure keeps the item cancelled and the next item pending", async () => {
  h.preflight = { kind: "extract" };
  h.batchErrors = ["provider error: OpenAI returned status 401 Unauthorized"];
  h.onJob = () => queue.getState().cancelQueue();
  queue.getState().addItem("/fixture/game");
  queue.getState().addItem("/fixture/next");
  await queue.getState().startQueue();
  assert.deepEqual(queue.getState().items.map(i => i.status), ["cancelled", "pending"]);
  assert.equal(queue.getState().items[0].error, null);
  assert.equal(h.calls.filter(c => c === "translate").length, 1);
  assert.ok(!h.calls.includes("validate"));
  assert.ok(h.toasts.every(([level]) => level === "info"));
});

for (const action of ["retryItem", "retryFailed"] as const) {
  test(`${action} resets only failed/cancelled work and does nothing during a run`, () => {
    for (const path of ["/error", "/cancelled", "/done", "/pending", "/translating"]) queue.getState().addItem(path);
    queue.setState(s => ({ items: s.items.map((item, i) => ({ ...item,
      status: (["error", "cancelled", "done", "pending", "translating"] as const)[i],
      error: "old error", validationIssues: 4, validationError: "old validation",
      progress: { completed: 2, total: 8, costSoFar: 0.3, costIsComplete: true, startedAt: 123 },
    })) }));
    const before = queue.getState().items;
    const retry = () => action === "retryItem"
      ? before.forEach(item => queue.getState().retryItem(item.id)) : queue.getState().retryFailed();
    queue.setState({ isRunning: true }); retry();
    assert.equal(queue.getState().items, before);
    queue.setState({ isRunning: false }); retry();
    const after = queue.getState().items;
    for (const item of after.slice(0, 2)) {
      assert.equal(item.status, "pending");
      assert.equal(item.error, null); assert.equal(item.validationIssues, null); assert.equal(item.validationError, null);
      assert.deepEqual(item.progress, { completed: 0, total: 0, costSoFar: 0, costIsComplete: false, startedAt: null });
    }
    assert.deepEqual(after.slice(2), before.slice(2));
    assert.equal(queue.getState().translationParams, params);
    const unchanged = queue.getState().items;
    queue.getState().retryItem("missing");
    assert.equal(queue.getState().items, unchanged);
    assert.deepEqual(queue.getState().history, []);
  });
}

for (const outcome of ["done", "error", "cancelled"] as const) {
  test(`queue records ${outcome} once with the terminal snapshot and retries via startQueue`, async () => {
    h.preflight = { kind: "extract" };
    if (outcome === "error") h.jobError = "failed";
    if (outcome === "cancelled") h.onJob = () => queue.getState().cancelQueue();
    queue.getState().addItem("/fixture/game");
    await queue.getState().startQueue();
    const item = queue.getState().items[0];
    const [entry] = queue.getState().history;
    assert.equal(queue.getState().history.length, 1);
    assert.deepEqual(entry, { id: entry.id, projectName: item.projectName, projectPath: item.projectPath,
      status: outcome, finishedAt: entry.finishedAt, completed: item.progress.completed,
      total: item.progress.total, costSoFar: item.progress.costSoFar,
      costIsComplete: item.progress.costIsComplete === true, validationIssues: item.validationIssues });
    assert.equal(new Date(entry.finishedAt).toISOString(), entry.finishedAt);
    await queue.getState().startQueue();
    assert.equal(queue.getState().history.length, 1, "terminal items do not run again automatically");
    if (outcome !== "done") {
      h.jobError = null; h.onJob = () => {};
      queue.getState().retryItem(item.id);
      await queue.getState().startQueue();
      assert.equal(queue.getState().items[0].status, "done");
      assert.deepEqual(queue.getState().history.map(e => e.status), ["done", outcome]);
      assert.notEqual(queue.getState().history[0].id, entry.id);
      assert.deepEqual(queue.getState().history[1], entry, "retry preserves the previous attempt");
    }
    queue.getState().clearHistory();
    assert.deepEqual(queue.getState().history, []);
    assert.equal(queue.getState().items.length, 1);
  });
}

test("queue history retains only the latest 20 attempts", async () => {
  h.preflight = { kind: "extract" };
  for (let i = 0; i < 22; i++) queue.getState().addItem(`/fixture/${i}`);
  await queue.getState().startQueue();
  assert.equal(queue.getState().history.length, 20);
  assert.deepEqual(queue.getState().history.map(e => e.projectPath), Array.from({ length: 20 }, (_, i) => `/fixture/${21 - i}`));
});

test("queue startup restores durable work while all runtime state stays idle", () => {
  const initial = queue.getInitialState();
  assert.deepEqual(initial.items, [{ ...startupItem, status: "error", error: "queue.interrupted" }]);
  assert.deepEqual(initial.translationParams, params);
  assert.deepEqual(initial.history, []);
  assert.equal(initial.isRunning, false); assert.equal(initial.isPanelOpen, false);
  assert.equal(initial.cancelRequested, false); assert.equal(initial.globalProgress, null);
});

test("queue persists every durable change including run history, but skips runtime-only updates", async () => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  const writes: any[] = [];
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
    getItem() { return null; }, removeItem() {},
    setItem(key: string, value: string) { assert.equal(key, "locust.queue.v1"); writes.push(JSON.parse(value)); },
  } });
  try {
    const snapshot = () => writes[writes.length - 1];
    queue.getState().addItem("/fixture/game");
    assert.deepEqual(snapshot().items, queue.getState().items);
    queue.getState().setParams({ ...params, provider_id: "other" });
    assert.equal(snapshot().translationParams.provider_id, "other");
    const count = writes.length;
    queue.getState().setPanelOpen(true); queue.getState().setGlobalProgress(null);
    assert.equal(writes.length, count);
    h.preflight = { kind: "extract" };
    await queue.getState().startQueue();
    assert.equal(snapshot().history.length, 1);
    assert.equal(snapshot().history[0].status, "done");
    queue.getState().clearCompleted(); assert.deepEqual(snapshot().items, []);
    queue.getState().clearHistory(); assert.deepEqual(snapshot().history, []);
    assert.deepEqual(Object.keys(snapshot()).sort(), ["history", "items", "translationParams", "version"]);
  } finally {
    if (descriptor) Object.defineProperty(globalThis, "localStorage", descriptor);
    else Reflect.deleteProperty(globalThis, "localStorage");
  }
});

afterEach(() => queryClient.clear());

const jobProject = { path: "/fixture/game", database_path: "/fixture/game.locust.db", format_id: "renpy", name: "Fixture" };
function projectDataKeys(scope: string) {
  return [
    ["strings", { status: "pending" }, scope], ["strings", { offset: 50 }, scope],
    ["string", "row", scope], ["stats", scope], ["file-stats", scope],
    ["current-project", scope], ["translation-runs", scope], ["injection-status", scope],
    ["validation-issues", scope, "syntax", false, 0], ["validation-summary", scope],
    ["string-history", scope, "row"],
  ];
}

for (const outcome of ["success", "failure", "cancellation"] as const) {
  test(`queue ${outcome} invalidates only the settled job's project data`, async () => {
    h.preflight = { kind: "extract" };
    const own = projectDataKeys(draftProjectKey(jobProject));
    // Same game folder, separate database: matching by path would corrupt this cache.
    const other = projectDataKeys(draftProjectKey({ ...jobProject, database_path: "/fixture/other.locust.db" }));
    h.onJob = () => {
      for (const key of [...own, ...other]) queryClient.setQueryData(key, "cached");
      if (outcome === "cancellation") queue.getState().cancelQueue();
    };
    if (outcome === "failure") h.jobError = "partial write before failure";
    queue.getState().addItem(jobProject.path);
    await queue.getState().startQueue();
    assert.equal(queue.getState().items[0].status, outcome === "success" ? "done" : outcome === "failure" ? "error" : "cancelled");
    for (const key of own) assert.equal(queryClient.getQueryState(key)?.isInvalidated, true, JSON.stringify(key));
    for (const key of other) assert.equal(queryClient.getQueryState(key)?.isInvalidated, false, JSON.stringify(key));
  });
}

test("queue marks a non-open job project stale without fetching its active queries", async () => {
  h.preflight = { kind: "extract" };
  const key = ["stats", draftProjectKey(jobProject)];
  let reads = 0;
  let unsubscribe = () => {};
  h.onJob = () => {
    project.setState({ project: { ...jobProject, database_path: "/fixture/other.locust.db" } });
    queryClient.setQueryData(key, "cached");
    const observer = new QueryObserver(queryClient, { queryKey: key, staleTime: Infinity,
      queryFn: async () => { reads++; return "wrong live project"; } });
    unsubscribe = observer.subscribe(() => {});
  };
  try {
    queue.getState().addItem(jobProject.path);
    await queue.getState().startQueue();
    assert.equal(queryClient.getQueryState(key)?.isInvalidated, true);
    assert.equal(reads, 0);
    assert.equal(queryClient.getQueryData(key), "cached");
  } finally { unsubscribe(); }
});

for (const outcome of ["success", "failure"] as const) {
  test(`queue awaits the open project's active refetch before opening a different project (${outcome})`, async () => {
    h.preflight = { kind: "extract" };
    const key = ["stats", draftProjectKey(jobProject)];
    let release = () => {};
    let unsubscribe = () => {};
    let reads = 0;
    h.onJob = () => {
      h.onJob = () => {};
      queryClient.setQueryData(key, "cached");
      const observer = new QueryObserver(queryClient, { queryKey: key, staleTime: Infinity,
        queryFn: async () => {
          reads++;
          await new Promise<void>(resolve => { release = resolve; });
          assert.equal(project.getState().project?.path, jobProject.path);
          return "refreshed";
        } });
      unsubscribe = observer.subscribe(() => {});
    };
    if (outcome === "failure") h.jobError = "partial write before failure";
    queue.getState().addItem(jobProject.path);
    queue.getState().addItem("/fixture/next");
    const running = queue.getState().startQueue();
    try {
      for (let i = 0; i < 100 && !reads && queue.getState().isRunning; i++) {
        await new Promise<void>(resolve => setImmediate(resolve));
      }
      assert.equal(reads, 1, "settlement starts the active refetch");
      assert.deepEqual(h.openedPaths, [jobProject.path], "the backend must still serve the settled project");
      assert.equal(queue.getState().isRunning, true);
    } finally { release(); await running; unsubscribe(); }
    assert.deepEqual(h.openedPaths, [jobProject.path, "/fixture/next"]);
  });
}


test("queue cancels A's paused refresh before B opens and reconnects", async () => {
  h.preflight = { kind: "extract" };
  const nextProject = { ...jobProject, path: "/fixture/next", database_path: "/fixture/next.locust.db" };
  const families = ["validation-summary", "file-stats", "string-history"];
  const keys = (scope: string) => families.map(family => [family, scope]);
  const own = keys(draftProjectKey(jobProject));
  const next = keys(draftProjectKey(nextProject));
  const reads: string[] = [];
  const contaminated: unknown[] = [];
  let paused = false;
  let unsubscribeA: (() => void)[] = [];
  const unsubscribeB: (() => void)[] = [];
  const stopTracking = queryClient.getQueryCache().subscribe(event => {
    if (event.query.queryKey[1] !== draftProjectKey(jobProject)) return;
    if (event.query.state.fetchStatus === "paused") paused = true;
    if (event.query.state.data === nextProject.path) contaminated.push(event.query.queryKey);
  });
  queryClient.mount();
  h.onJob = () => {
    if (h.openedPaths.length === 1) {
      for (const key of own) {
        queryClient.setQueryData(key, jobProject.path);
        const observer = new QueryObserver(queryClient, { queryKey: key, staleTime: Infinity,
          queryFn: async () => {
            const backendPath = h.openedPaths[h.openedPaths.length - 1];
            reads.push(`A:${backendPath}`);
            return backendPath;
          } });
        unsubscribeA.push(observer.subscribe(() => {}));
      }
      onlineManager.setOnline(false);
    } else {
      // Model the view detaching A's observers after the backend opens B.
      unsubscribeA.forEach(unsubscribe => unsubscribe());
      unsubscribeA = [];
      for (const key of next) {
        queryClient.setQueryData(key, "old B data");
        const observer = new QueryObserver(queryClient, { queryKey: key, staleTime: Infinity,
          queryFn: async () => {
            const backendPath = h.openedPaths[h.openedPaths.length - 1];
            reads.push(`B:${backendPath}`);
            return backendPath;
          } });
        unsubscribeB.push(observer.subscribe(() => {}));
      }
      onlineManager.setOnline(true);
    }
  };
  try {
    queue.getState().addItem(jobProject.path);
    queue.getState().addItem(nextProject.path);
    await queue.getState().startQueue();
    await new Promise<void>(resolve => setImmediate(resolve));
    assert.equal(paused, true, "A's settlement refresh must pause while offline");
    assert.deepEqual(h.openedPaths, [jobProject.path, nextProject.path]);
    assert.deepEqual(queue.getState().items.map(item => item.status), ["done", "done"]);
    assert.deepEqual(contaminated, [], "A's cache must never receive B's data");
    assert.ok(reads.every(read => read === `B:${nextProject.path}`), JSON.stringify(reads));
    assert.equal(reads.length, families.length, "B's scoped queries must fetch B's data");
    for (const key of own) {
      assert.equal(queryClient.getQueryData(key), jobProject.path);
      assert.equal(queryClient.getQueryState(key)?.fetchStatus, "idle");
      assert.equal(queryClient.getQueryState(key)?.isInvalidated, true);
    }
    for (const key of next) assert.equal(queryClient.getQueryData(key), nextProject.path);
  } finally {
    stopTracking();
    [...unsubscribeA, ...unsubscribeB].forEach(unsubscribe => unsubscribe());
    queryClient.clear();
    queryClient.unmount();
    onlineManager.setOnline(true);
  }
});


test("live scoped observers cannot read A while B open is pending, and reopening A refetches", async () => {
  h.preflight = { kind: "extract" };
  const nextProject = { ...jobProject, path: "/fixture/next", database_path: "/fixture/next.locust.db" };
  const aKeys = projectDataKeys(draftProjectKey(jobProject));
  const bKeys = projectDataKeys(draftProjectKey(nextProject));
  const reads: string[] = [];
  const observers: (() => void)[] = [];
  let releaseOpen = () => {};
  let releaseRead = () => {};
  let openingB = false;
  let paused = false;
  const observe = (key: unknown[], label: string, initial: string) => {
    queryClient.setQueryData(key, initial);
    const scope = String(key[key[0] === "strings" || key[0] === "string" ? 2 : 1]);
    const options = { queryKey: key, staleTime: Infinity, enabled: projectScopedQueryEnabled(scope),
      queryFn: async () => {
        const backendPath = h.openedPaths[h.openedPaths.length - 1];
        reads.push(`${label}:${backendPath}`);
        if (label === "A" && reads.length === 1) {
          await new Promise<void>(resolve => { releaseRead = resolve; });
        }
        return backendPath;
      } };
    const observer = new QueryObserver(queryClient, options);
    observers.push(observer.subscribe(() => {}));
    observers.push(project.subscribe(() => observer.setOptions({ ...options, enabled: projectScopedQueryEnabled(scope) })));
    return observer;
  };
  const stopTracking = queryClient.getQueryCache().subscribe(event => {
    if (event.query.state.fetchStatus === "paused") paused = true;
  });
  h.onJob = () => {
    if (h.openedPaths.length !== 1) return;
    const active = aKeys.map(key => observe(key, "A", jobProject.path));
    void active[0].refetch();
    onlineManager.setOnline(false);
  };
  h.onOpen = async path => {
    if (path !== nextProject.path) return;
    assert.equal(project.getState().projectTransitionInProgress, true);
    assert.equal(project.getState().confirmedOpenProjectKey, draftProjectKey(jobProject));
    openingB = true;
    await new Promise<void>(resolve => { releaseOpen = resolve; });
  };
  queryClient.mount();
  queue.getState().addItem(jobProject.path);
  queue.getState().addItem(nextProject.path);
  const running = queue.getState().startQueue();
  try {
    for (let i = 0; i < 100 && !openingB; i++) await new Promise<void>(resolve => setImmediate(resolve));
    assert.equal(openingB, true);
    assert.equal(paused, true, "A's settlement refresh paused offline");
    bKeys.forEach(key => observe(key, "B", "old B"));
    await queryClient.invalidateQueries({ predicate: q => bKeys.some(key => JSON.stringify(key) === JSON.stringify(q.queryKey)), refetchType: "none" });
    const beforeReconnect = reads.slice();
    onlineManager.setOnline(true);
    focusManager.setFocused(false);
    focusManager.setFocused(true);
    await new Promise<void>(resolve => setImmediate(resolve));
    await queryClient.invalidateQueries({ predicate: q => aKeys.some(key => JSON.stringify(key) === JSON.stringify(q.queryKey)) });
    assert.deepEqual(reads, beforeReconnect, "reconnect during B's RPC must issue no scoped reads");
    for (const key of aKeys) assert.equal(queryClient.getQueryData(key), jobProject.path);
    releaseRead(); releaseOpen();
    await running;
    assert.equal(project.getState().projectTransitionInProgress, false);
    assert.equal(project.getState().confirmedOpenProjectKey, draftProjectKey(nextProject));
    assert.ok(reads.some(read => read === `B:${nextProject.path}`));
    for (const key of aKeys) assert.equal(queryClient.getQueryData(key), jobProject.path);
    for (const key of bKeys) assert.equal(queryClient.getQueryData(key), nextProject.path);
    const beforeReopen = reads.length;
    queue.setState({ items: [] });
    queue.getState().addItem(jobProject.path);
    h.onJob = () => {};
    await queue.getState().startQueue();
    assert.ok(reads.slice(beforeReopen).some(read => read === `A:${jobProject.path}`));
    assert.ok(reads.slice(beforeReopen).every(read => read === `A:${jobProject.path}`));
    for (const key of aKeys) assert.equal(queryClient.getQueryData(key), jobProject.path);
  } finally {
    observers.forEach(stop => stop()); stopTracking();
    releaseRead(); releaseOpen(); onlineManager.setOnline(true);
    await running;
    queryClient.unmount();
    focusManager.setFocused(undefined);
  }
});
