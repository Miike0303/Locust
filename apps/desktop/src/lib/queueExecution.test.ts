import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";
import { registerHooks } from "node:module";
import { setLocale, t } from "./i18n/index.ts";
import * as policy from "./translationJob.ts";
import type { TranslationStartParams } from "./api.ts";

// Keep the actual Zustand stores and session; replace only network and UI sinks.
const h = {
  calls: [] as string[], toasts: [] as any[], logs: [] as any[], handlers: null as any,
  onOpen: () => {}, onWait: () => {},
  start: async () => ({ job_id: "job-1" }),
};
(globalThis as any).__queueExecution = h;
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/components/TranslationModal.tsx")) {
      const stubs: Record<string, string> = {
        react: `export const useState=x=>[typeof x==='function'?x():x,()=>{}]; export const useEffect=()=>{}; export const useRef=x=>({current:x});`,
        "@tanstack/react-query": `export const useQuery=()=>({});`,
        "react-router-dom": `export const useNavigate=()=>()=>{};`,
        "../lib/i18n": `export const useT=()=>globalThis.__queueExecution.t; export const useLocale=()=>({locale:'en'});`,
        "../stores/editorStore": `export const useEditorStore=Object.assign(()=>globalThis.__queueExecution.editor.getState(),globalThis.__queueExecution.editor);`,
        "../lib/modalA11y": `export const useModalA11y=()=>({}); export const MODAL_BACKDROP_CLASS=''; export const modalPanelClass=x=>x;`,
      };
      if (stubs[specifier]) return { url: `data:text/javascript,${encodeURIComponent(stubs[specifier])}`, shortCircuit: true };
    }
    return nextResolve(specifier, context);
  },
  load(url, context, nextLoad) {
    const modules: Record<string, string> = {
      "/src/lib/api.ts": `const h=globalThis.__queueExecution;
        export const getProviders=()=>[]; export const getConfig=()=>({}); export const checkProviderHealth=()=>({});
        export async function preflightProjectOpen(){return {kind:'extract'};}
        export async function resumeProject(){throw new Error('plain fixture must not resume');}
        export async function openProject(path){h.calls.push('open');h.onOpen();return {project_path:path,project_name:'Fixture',format_id:'test',total_strings:3,supported_modes:[],database_path:'fixture.db',extraction_warnings:[]};}
        export async function startTranslation(){h.calls.push('start');return h.start();}
        export async function cancelTranslation(){h.calls.push('cancel');}
        export async function validate(){h.calls.push('validate');return {validation:{issues_found:0}};}`,
      "/src/lib/ws.ts": `const h=globalThis.__queueExecution;
        export const JOB_STREAM_LOST_MESSAGE='stream lost';
        export async function waitForJob(id,options){h.calls.push('wait');options.onProgress(1,3,0.1,true);await h.onWait();}
        export function subscribeToJob(id,handlers){h.handlers=handlers;return()=>{};}`,
      "/src/stores/logStore.ts": `export function addLog(...args){globalThis.__queueExecution.logs.push(args);}`,
      "/src/stores/toastStore.ts": `export function addToast(...args){globalThis.__queueExecution.toasts.push(args);}`,
    };
    const path = url.replace(/\\/g, "/");
    const key = Object.keys(modules).find(suffix => path.endsWith(suffix));
    return key ? { format: "module", source: modules[key], shortCircuit: true } : nextLoad(url, context);
  },
});
const { useQueueStore: queue } = await import("../stores/queueStore.ts");
const { useEditorStore: editor } = await import("../stores/editorStore.ts");
const session = await import("./translationJobSession.ts");
Object.assign(h, { editor, t });
const params: TranslationStartParams = {
  provider_id: "mock",
  options: {
    source_lang: "en", target_lang: "es", batch_size: 3, max_concurrent: 1,
    cost_limit_usd: null, game_context: null, use_glossary: true, use_memory: true, skip_approved: true,
  },
};
const opts = { projectName: "Single", providerLabel: "Mock" };
const queueProgress = {
  owner: { kind: "queue", itemId: "queue-item" }, projectName: "Queued",
  completed: 2, total: 10, costSoFar: 0, startedAt: 123, queuePosition: 1, queueTotal: 2,
} as const;

beforeEach(() => {
  if (session.subscribedTranslationJobId()) h.handlers.onFailed({ error: "cleanup" });
  queue.setState({ items: [], isRunning: false, cancelRequested: false, globalProgress: null, translationParams: params });
  editor.setState({ isTranslating: false, jobId: null, jobSnapshot: null });
  h.calls = []; h.toasts = []; h.logs = []; h.handlers = null;
  h.onOpen = () => {}; h.onWait = () => {}; h.start = async () => ({ job_id: "job-1" });
  session.setTranslationModalOpen(true);
  setLocale("en");
});

test("pure start policies reject either active owner, including a start still awaiting its job ID", () => {
  for (const queueRunning of [false, true]) {
    for (const singleJobRunning of [false, true]) {
      const state = { queueRunning, singleJobRunning };
      assert.equal(policy.canStartQueue(state), !queueRunning && !singleJobRunning);
      assert.equal(policy.canStartSingleJob(state), !queueRunning && !singleJobRunning);
    }
  }
});

for (const locale of ["en", "es"] as const) {
  test(`cancelled queue item stays cancelled, preserves pending items and emits only cancellation notices (${locale})`, async () => {
    setLocale(locale);
    queue.getState().addItem("/fixture/first");
    queue.getState().addItem("/fixture/second");
    h.onWait = () => queue.getState().cancelQueue();
    await queue.getState().startQueue();
    assert.deepEqual(queue.getState().items.map(i => i.status), ["cancelled", "pending"]);
    assert.equal(queue.getState().items[0].error, null);
    assert.equal(h.calls.filter(c => c === "open").length, 1);
    assert.ok(!h.calls.includes("validate"));
    assert.ok(h.toasts.some(([level, message]) => level === "info" && message === t("queue.toast.itemCancelled", { name: "Fixture" })));
    assert.ok(h.logs.some(([level, message]) => level === "info" && message === t("activity.queue.itemCancelled", { name: "Fixture" })));
    assert.ok(!h.toasts.some(([level]) => level === "success"));
    assert.ok(!h.logs.some(([, message]) => message === t("activity.queue.completed", { name: "Fixture", count: 3 })));
    queue.getState().clearCompleted();
    assert.deepEqual(queue.getState().items.map(i => i.status), ["cancelled", "pending"]);
    assert.equal(t("queue.status.cancelled"), locale === "en" ? "Cancelled" : "Cancelado");
  });

  test(`queue refuses a live single job without touching items or progress (${locale})`, async () => {
    setLocale(locale);
    queue.getState().addItem("/fixture/first");
    editor.setState({ isTranslating: true, jobId: "single" });
    const progress = { ...queueProgress, owner: { kind: "single", jobId: "single" } } as const;
    queue.getState().setGlobalProgress(progress);
    const before = queue.getState();
    await queue.getState().startQueue();
    assert.equal(queue.getState().items, before.items);
    assert.equal(queue.getState().globalProgress, progress);
    assert.equal(queue.getState().isRunning, false);
    assert.deepEqual(h.calls, []);
    assert.deepEqual(h.toasts, [["info", t("queue.toast.singleJobRunning")]]);
    assert.notEqual(h.toasts[0][1], "queue.toast.singleJobRunning");
  });

  test(`single start refuses a running queue without invoking the backend (${locale})`, async () => {
    const { startSingleTranslation } = await import("./translationStart.ts");
    setLocale(locale);
    queue.setState({ isRunning: true, globalProgress: queueProgress });
    const before = editor.getState();
    assert.equal(await startSingleTranslation(params, opts), null);
    assert.equal(editor.getState(), before);
    assert.equal(queue.getState().globalProgress, queueProgress);
    assert.deepEqual(h.calls, []);
    assert.deepEqual(h.toasts, [["info", t("translate.toast.queueRunning")]]);
    assert.notEqual(h.toasts[0][1], "translate.toast.queueRunning");
  });
}

test("a completed queue item is still cleared normally", async () => {
  queue.getState().addItem("/fixture/first");
  await queue.getState().startQueue();
  assert.equal(queue.getState().items[0].status, "done");
  assert.ok(h.calls.includes("validate"));
  queue.getState().clearCompleted();
  assert.deepEqual(queue.getState().items, []);
});

test("duplicate queue start does not replace progress or open another project", async () => {
  queue.getState().addItem("/fixture/first");
  queue.setState({ isRunning: true, globalProgress: queueProgress });
  const before = queue.getState();
  await queue.getState().startQueue();
  assert.equal(queue.getState(), before);
  assert.deepEqual(h.calls, []);
});

test("single start reserves exclusivity before its API promise resolves and releases it on rejection", async () => {
  const { startSingleTranslation } = await import("./translationStart.ts");
  let rejectStart!: (reason: Error) => void;
  h.start = () => new Promise((_, reject) => { rejectStart = reject; });
  const starting = startSingleTranslation(params, opts);
  assert.equal(editor.getState().isTranslating, true);
  assert.equal(editor.getState().jobId, null);
  queue.getState().addItem("/fixture/first");
  await queue.getState().startQueue();
  assert.equal(await startSingleTranslation(params, opts), null);
  assert.deepEqual(h.calls, ["start"]);
  assert.equal(queue.getState().items[0].status, "pending");
  rejectStart(new Error("start rejected"));
  await assert.rejects(starting, /start rejected/);
  assert.equal(editor.getState().isTranslating, false);
  h.start = async () => ({ job_id: "job-1" });
  assert.deepEqual(await startSingleTranslation(params, opts), { job_id: "job-1" });
  assert.equal(session.subscribedTranslationJobId(), "job-1");
});

test("single completion clears its own progress", () => {
  session.attachTranslationJob({ jobId: "single", ...opts });
  h.handlers.onStarted({ total: 3 });
  assert.deepEqual(queue.getState().globalProgress?.owner, { kind: "single", jobId: "single" });
  h.handlers.onBatchCompleted({ completed: 1, total: 3, cost_so_far: 0.1 });
  assert.equal(queue.getState().globalProgress?.completed, 1);
  h.handlers.onCompleted({ total_translated: 3, total_cost: 0.1 });
  assert.equal(queue.getState().globalProgress, null);
});

for (const terminal of ["onCompleted", "onFailed", "onClosed"] as const) {
  test(`single ${terminal} cannot clear queue progress`, () => {
    session.attachTranslationJob({ jobId: "single", ...opts });
    h.handlers.onStarted({ total: 3 });
    queue.setState({ isRunning: true, globalProgress: queueProgress });
    h.handlers[terminal]({ total_translated: 3, total_cost: 0, error: "cancelled" });
    assert.equal(queue.getState().globalProgress, queueProgress);
    assert.equal(queue.getState().isRunning, true);
  });
}

test("late single progress events cannot overwrite a running queue", () => {
  session.attachTranslationJob({ jobId: "single", ...opts });
  queue.setState({ isRunning: true, globalProgress: queueProgress });
  h.handlers.onStarted({ total: 3 });
  assert.equal(queue.getState().globalProgress, queueProgress);
  h.handlers.onBatchCompleted({ completed: 1, total: 3, cost_so_far: 0.1 });
  assert.equal(queue.getState().globalProgress, queueProgress);
});

for (const stage of ["open", "wait rejection"] as const) {
  test(`queue cancellation during ${stage} emits an item cancellation notice`, async () => {
    queue.getState().addItem("/fixture/first");
    queue.getState().addItem("/fixture/second");
    if (stage === "open") h.onOpen = () => queue.getState().cancelQueue();
    else h.onWait = () => { queue.getState().cancelQueue(); throw new Error("cancelled"); };
    await queue.getState().startQueue();
    assert.deepEqual(queue.getState().items.map(i => i.status), ["cancelled", "pending"]);
    assert.ok(h.toasts.some(([level, message]) => level === "info" && message === t("queue.toast.itemCancelled", { name: "Fixture" })));
    assert.ok(h.logs.some(([level, message]) => level === "info" && message === t("activity.queue.itemCancelled", { name: "Fixture" })));
    assert.ok(!h.toasts.some(([level]) => level === "success" || level === "error"));
    assert.ok(!h.calls.includes("validate"));
    if (stage === "open") assert.ok(!h.calls.includes("start"));
  });
}

test("queue reserves exclusivity while project opening is still pending", async () => {
  const { startSingleTranslation } = await import("./translationStart.ts");
  queue.getState().addItem("/fixture/first");
  const running = queue.getState().startQueue();
  assert.equal(queue.getState().isRunning, true);
  assert.equal(queue.getState().items[0].status, "extracting");
  assert.deepEqual(queue.getState().globalProgress?.owner, { kind: "queue", itemId: queue.getState().items[0].id });
  const single = await startSingleTranslation(params, opts);
  await running;
  assert.equal(single, null);
  assert.equal(h.calls.filter(c => c === "start").length, 1);
});

test("single completion preserves progress owned by a different single job", () => {
  session.attachTranslationJob({ jobId: "old-single", ...opts });
  const progress = { ...queueProgress, owner: { kind: "single", jobId: "new-single" } } as const;
  queue.getState().setGlobalProgress(progress);
  h.handlers.onCompleted({ total_translated: 3, total_cost: 0.1 });
  assert.equal(queue.getState().globalProgress, progress);
});

for (const locale of ["en", "es"] as const) {
  test(`Translate dialog start button refuses a running queue (${locale})`, async () => {
    const { default: TranslationModal } = await import("../components/TranslationModal.tsx");
    setLocale(locale);
    queue.setState({ isRunning: true, globalProgress: queueProgress });
    const before = editor.getState();
    const tree = TranslationModal({ open: true, totalPending: 3, onClose() {}, onComplete() {} });
    function findStart(node: any): any {
      if (!node || typeof node !== "object") return undefined;
      if (Array.isArray(node)) return node.map(findStart).find(Boolean);
      if (node.type === "button" && node.props.children === t("translate.start")) return node;
      return findStart(node.props?.children);
    }
    const start = findStart(tree);
    assert.ok(start, "the configure dialog renders a start button");
    await start.props.onClick();
    assert.deepEqual(h.calls, []);
    assert.deepEqual(h.toasts, [["info", t("translate.toast.queueRunning")]]);
    assert.equal(editor.getState(), before);
    assert.equal(queue.getState().globalProgress, queueProgress);
  });
}
