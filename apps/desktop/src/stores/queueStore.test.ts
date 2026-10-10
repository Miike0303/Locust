import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";
import { registerHooks } from "node:module";
import {
  getProjectOpenChoice, resolveProjectOpenChoice,
  type ProjectOpenChoice, type ProjectOpenChoiceRequest,
} from "../lib/openProjectFlow.ts";
import type { ProjectOpenPreflight, TranslationStartParams } from "../lib/api.ts";

const h = {
  calls: [] as string[], toasts: [] as unknown[][],
  logs: [] as unknown[][], jobError: null as string | null,
  batchErrors: [] as string[], translated: 1, progressTotal: 100, onJob: () => {},
  preflight: { kind: "resume_available", database_path: "/fixture/game.locust.db", project_path: "/fixture/game", format_id: "renpy" } as ProjectOpenPreflight,
};
(globalThis as any).__resumeQueue = h;
registerHooks({
  load(url, context, nextLoad) {
    const stubs: Record<string, string> = {
      "/src/lib/api.ts": `const h=globalThis.__resumeQueue;
        const result={project_path:'/fixture/game',project_name:'Fixture',format_id:'renpy',total_strings:1,supported_modes:['replace'],database_path:'/fixture/game.locust.db',extraction_warnings:['Saved warning'],added:0,updated:0,stale_source_reset:0,removed:0,preserved_translations:0};
        export async function preflightProjectOpen(){h.calls.push('preflight');return h.preflight;}
        export async function openProject(){h.calls.push('extract');return result;}
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
const { useQueueStore: queue } = await import("./queueStore.ts");
const { useProjectStore: project } = await import("./projectStore.ts");
const { useEditorStore: editor } = await import("./editorStore.ts");
const { setLocale, t } = await import("../lib/i18n/index.ts");
const params: TranslationStartParams = {
  provider_id: "mock", options: {
    source_lang: "ja", target_lang: "en", batch_size: 1, max_concurrent: 1,
    cost_limit_usd: null, game_context: null, use_glossary: false, use_memory: false, skip_approved: true,
  },
};
const originalProject = { path: "/other", format_id: "renpy", name: "Other" };
beforeEach(() => {
  const pending = getProjectOpenChoice();
  if (pending) resolveProjectOpenChoice(pending, "cancel");
  queue.setState({ items: [], isRunning: false, cancelRequested: false, globalProgress: null, translationParams: params });
  editor.setState({ isTranslating: false });
  project.setState({ project: originalProject });
  h.calls = []; h.toasts = []; h.logs = []; h.jobError = null;
  h.batchErrors = []; h.translated = 1; h.progressTotal = 100; h.onJob = () => {};
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
