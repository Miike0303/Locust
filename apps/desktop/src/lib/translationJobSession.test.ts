import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { test } from "node:test";
import { setLocale, t } from "./i18n/index.ts";

// Exercise the real persistent session across terminal/cancel races. Only its
// network and UI sinks are replaced; lifecycle decisions stay in production code.
const h = {
  snapshot: null as any, jobId: null as string | null, translating: false,
  handlers: null as any, toasts: [] as any[], logs: [] as any[], unsubscribed: 0,
  cancellations: [] as string[],
  subscriptions: [] as { jobId: string; handlers: any }[],
};
(globalThis as any).__sessionHarness = h;
registerHooks({
  load(url, context, nextLoad) {
    const p = url.replace(/\\/g, "/");
    const modules: Record<string, string> = {
      "/src/lib/api.ts": `export async function cancelTranslation(id){globalThis.__sessionHarness.cancellations.push(id);}
        export async function startTranslation(){throw new Error('unexpected translation start');}
        export async function getProviders(){return [];}
        export async function getConfig(){return {};}
        export async function checkProviderHealth(){return {ok:true};}`,
      "/src/lib/ws.ts": `export const JOB_STREAM_LOST_MESSAGE='stream lost'; export function subscribeToJob(id,handlers){const h=globalThis.__sessionHarness;h.handlers=handlers;h.subscriptions.push({jobId:id,handlers});return()=>{h.unsubscribed++;};}`,
      "/src/stores/editorStore.ts": `const h=globalThis.__sessionHarness; export const useEditorStore=Object.assign(()=>({isTranslating:h.translating,jobSnapshot:h.snapshot}),{getState:()=>({jobId:h.jobId,isTranslating:h.translating,jobSnapshot:h.snapshot,setJob:id=>{h.jobId=id;},setTranslating:v=>{h.translating=v;},setJobSnapshot:v=>{h.snapshot=v;},patchJobSnapshot:v=>{h.snapshot={...h.snapshot,...v};}})});`,
      "/src/stores/queueStore.ts": `export const useQueueStore={getState:()=>({setGlobalProgress:()=>{},globalProgress:null})};`,
      "/src/stores/logStore.ts": `export function addLog(...args){globalThis.__sessionHarness.logs.push(args);}`,
      "/src/stores/toastStore.ts": `export function addToast(...args){globalThis.__sessionHarness.toasts.push(args);}`,
    };
    const key = Object.keys(modules).find(suffix => p.endsWith(suffix));
    return key ? { format: "module", source: modules[key], shortCircuit: true } : nextLoad(url, context);
  },
});
const session = await import("./translationJobSession.ts");
const { createElement } = await import("react");
const { renderToStaticMarkup } = await import("react-dom/server");
const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
const { MemoryRouter } = await import("react-router-dom");
const { default: TranslationModal } = await import("../components/TranslationModal.tsx");
function renderModal() {
  return renderToStaticMarkup(createElement(QueryClientProvider, { client: new QueryClient() },
    createElement(MemoryRouter, null,
      createElement(TranslationModal, { open: true, totalPending: 5, onClose() {}, onComplete() {}, onReview() {} }))));
}
session.setTranslationModalOpen(true);
const attach = (jobId: string) => session.attachTranslationJob({jobId,projectName:"Fixture",providerLabel:"Mock"});
for (const locale of ["en", "es"] as const) {
  test(`completion without batch failures preserves success, including an empty job in ${locale}`, () => {
    setLocale(locale);
    for (const translated of [3, 0]) {
      attach(`clean-${locale}-${translated}`);
      h.logs = []; h.toasts = [];
      h.handlers.onCompleted({ type: "completed", total_translated: translated, total_cost: 0, cost_is_complete: true });
      assert.equal(h.snapshot.done, true);
      assert.equal(h.snapshot.error, null);
      assert.equal(h.snapshot.batchFailures?.count ?? 0, 0, "a new job must not inherit failures");
      assert.ok(renderModal().includes(t("translate.complete")));
      assert.deepEqual(h.toasts, [["success", t("translate.toast.completeObservedCost", { count: translated, cost: "$0.0000" })]]);
      assert.deepEqual(h.logs, [["info", t("activity.translation.completed", { count: translated, cost: "$0.0000" }), undefined, "translation"]]);
    }
  });

  test(`fallback recovery reaching the latest progress total completes normally in ${locale}`, () => {
    setLocale(locale);
    for (const progressSource of ["started", "batch_completed"]) {
      for (const translated of [5, 6]) {
        attach(`recovered-${locale}-${progressSource}-${translated}`);
        h.logs = []; h.toasts = [];
        h.handlers.onStarted({ total: progressSource === "started" ? 5 : 10 });
        const raw = "provider error: OpenAI returned status 401 Unauthorized";
        h.handlers.onBatchFailed({ type: "batch_failed", error: raw });
        h.handlers.onProviderSwitched({ provider_name: "Claude", remaining_pending: 5 });
        if (progressSource === "batch_completed") {
          h.handlers.onBatchCompleted({ completed: 4, total: 5, cost_so_far: 0 });
        }
        h.toasts = [];
        h.handlers.onCompleted({ type: "completed", total_translated: translated, total_cost: 0, cost_is_complete: true });
        assert.deepEqual(h.toasts, [["success", t("translate.toast.completeObservedCost", { count: translated, cost: "$0.0000" })]]);
        assert.equal(h.snapshot.done, true);
        assert.equal(h.snapshot.error, null);
        assert.equal(h.snapshot.completed, translated);
        assert.equal(h.snapshot.batchFailures?.count ?? 0, 0);
        assert.equal(h.translating, false);
        assert.equal(h.jobId, null);
        const modal = renderModal();
        assert.ok(modal.includes(t("translate.complete")));
        assert.ok(!modal.includes(t("translate.completedWithErrors")));
        assert.ok(!modal.includes(t("translate.pendingRetryHint")));
        assert.deepEqual(h.logs[h.logs.length - 1], ["info", t("activity.translation.completed", { count: translated, cost: "$0.0000" }), undefined, "translation"]);
        assert.equal(h.logs.filter(([, , detail]) => detail === raw).length, 1);
      }
    }
  });

  test(`batch failures with an unknown total retain warning or failure outcomes in ${locale}`, () => {
    setLocale(locale);
    for (const translated of [2, 0]) {
      attach(`unknown-total-${locale}-${translated}`);
      h.logs = []; h.toasts = [];
      h.handlers.onBatchFailed({ type: "batch_failed", error: "provider unavailable" });
      h.handlers.onCompleted({ type: "completed", total_translated: translated, total_cost: 0, cost_is_complete: true });
      assert.equal(h.snapshot.total, 0);
      assert.equal(h.snapshot.done, translated > 0);
      assert.equal(h.snapshot.error, translated > 0 ? null : "provider unavailable");
      assert.equal(h.snapshot.batchFailures.count, 1);
      assert.equal(h.toasts.length, 1);
      assert.equal(h.toasts[0][0], translated > 0 ? "warning" : "error");
      assert.ok(renderModal().includes(t(translated > 0 ? "translate.completedWithErrors" : "translate.failed")));
    }
  });

  for (const translated of [2, 0]) {
    test(`${translated ? "partial" : "total"} batch failure uses terminal translated count and keeps raw detail once in ${locale}`, () => {
      setLocale(locale);
      attach(`batch-result-${locale}-${translated}`);
      h.logs = []; h.toasts = [];
      h.handlers.onStarted({ total: 5 });
      const raws = [
        "provider error: OpenAI returned status 401 Unauthorized: first opaque body",
        "provider error: Claude returned status 403 Forbidden: last opaque body  ",
      ];
      h.handlers.onBatchFailed({ type: "batch_failed", error: raws[0] });
      h.handlers.onProviderSwitched({ provider_name: "Claude", remaining_pending: 5 });
      h.handlers.onStarted({ total: 5 });
      h.handlers.onBatchFailed({ type: "batch_failed", error: raws[1] });
      const reason = locale === "es"
        ? "Claude rechazó la clave API. Revísela en Ajustes → Proveedores."
        : "Claude rejected the API key. Check it in Settings → Providers.";
      assert.deepEqual(h.snapshot.batchFailures, { count: 2, lastReason: reason });
      assert.equal(h.snapshot.error, null, "recoverable batch failures must let the job continue");
      // Interim progress can differ across fallback passes; only completed is authoritative.
      h.handlers.onBatchCompleted({ completed: 5, total: 5, cost_so_far: 0 });
      h.toasts = [];
      h.handlers.onCompleted({ type: "completed", total_translated: translated, total_cost: 0, cost_is_complete: true });
      assert.equal(h.snapshot.done, translated > 0);
      assert.equal(h.snapshot.error, translated > 0 ? null : reason);
      assert.equal(h.snapshot.completed, translated);
      assert.equal(h.snapshot.cancelling, false);
      assert.equal(h.translating, false);
      assert.equal(h.jobId, null);
      const modal = renderModal();
      assert.ok(modal.includes(t(translated > 0 ? "translate.completedWithErrors" : "translate.failed")));
      assert.ok(!modal.includes(t("translate.complete")), "failed work must never show the success heading");
      assert.ok(modal.includes(reason));
      assert.ok(modal.includes(translated > 0 ? "bg-warning-muted" : "bg-danger-muted"));
      assert.ok(modal.includes(translated > 0 ? "text-warning" : "text-danger"));
      if (translated > 0) {
        assert.ok(modal.includes(t("translate.completedWithErrorsSummary", { translated, count: 2 })));
        assert.ok(modal.includes(t("translate.pendingRetryHint")));
        assert.ok(modal.includes(t("translate.review")));
      }
      const message = translated > 0
        ? t("translate.completedWithErrorsSummary", { translated, count: 2 })
        : t("translate.toast.failed", { error: reason });
      assert.deepEqual(h.toasts, [[translated > 0 ? "warning" : "error", message]]);
      assert.equal(h.logs[h.logs.length - 1][0], translated > 0 ? "warning" : "error");
      for (const raw of raws) {
        assert.equal(h.logs.filter(([, , detail]) => detail === raw).length, 1);
        assert.ok(!h.logs.some(([, summary]) => summary.includes(raw)));
      }
      const logCount = h.logs.length;
      h.handlers.onCompleted({ type: "completed", total_translated: translated, total_cost: 0 });
      h.handlers.onBatchFailed({ error: raws[1] });
      h.handlers.onClosed();
      assert.equal(h.logs.length, logCount);
      assert.equal(h.toasts.length, 1);
    });
  }
}
attach("cancel-replayed");
h.handlers.onFailed({type:"failed",entry_id:null,error:"cancelled"});
assert.equal(h.snapshot.cancelled,true);
assert.equal(h.snapshot.error,null);
assert.equal(h.translating,false);
assert.equal(h.jobId,null);
assert.equal(h.toasts[h.toasts.length - 1][0],"info");
assert.equal(h.toasts[h.toasts.length - 1][1],t("translate.toast.cancelled"));
const count=h.toasts.length;
h.handlers.onClosed();
assert.equal(h.toasts.length,count,"socket close cannot finish cancellation twice");

attach("real-failure");
h.handlers.onFailed({type:"failed",entry_id:null,error:"authentication failed"});
assert.equal(h.snapshot.cancelled,false);
assert.equal(h.snapshot.error,"authentication failed");
assert.equal(h.toasts[h.toasts.length - 1][0],"error");

attach("completion-wins");
await session.requestTranslationCancel();
assert.equal(h.snapshot.cancelling,true);
assert.equal(h.cancellations[h.cancellations.length - 1],"completion-wins");
h.handlers.onCompleted({type:"completed",total_translated:3,total_cost:0,cost_is_complete:true});
assert.equal(h.snapshot.done,true);
assert.equal(h.snapshot.cancelled,false);
assert.equal(h.snapshot.cancelling,false);
assert.equal(h.toasts[h.toasts.length - 1][0],"success");

attach("old-server-close");
await session.requestTranslationCancel();
h.handlers.onClosed();
assert.equal(h.snapshot.cancelled,true);
assert.equal(h.snapshot.error,null);
assert.equal(session.subscribedTranslationJobId(),null);

// The same live session must read the current locale when events arrive.
// Raw backend diagnostics, severity, and source stay untouched.
setLocale("es");
attach("locale-switch");
h.handlers.onProviderSwitched({provider_name:"Provider X",remaining_pending:1});
assert.deepEqual(h.logs[h.logs.length - 1], ["info", "Se cambió al proveedor Provider X (1 cadena pendiente)", undefined, "translation"]);
h.handlers.onBatchFailed({error:"raw backend failure: /game/script.rpy"});
assert.deepEqual(h.logs[h.logs.length - 1], ["warning", "Falló un lote de traducción; se continuará con los siguientes", "raw backend failure: /game/script.rpy", "translation"]);
setLocale("en");
h.handlers.onProviderSwitched({provider_name:"Provider X",remaining_pending:2});
assert.deepEqual(h.logs[h.logs.length - 1], ["info", "Switched to provider Provider X (2 strings still pending)", undefined, "translation"]);
setLocale("es");
h.handlers.onFailed({type:"failed",entry_id:null,error:"raw backend failure"});
assert.deepEqual(h.logs[h.logs.length - 1], ["error", "La traducción falló", "raw backend failure", "translation"]);
setLocale("en");
for (const locale of ["en", "es"] as const) {
  test(`terminal provider failure localizes snapshot and toast, logging raw detail once in ${locale}`, () => {
    setLocale(locale === "en" ? "es" : "en");
    attach(`provider-failed-${locale}`);
    setLocale(locale);
    h.logs = []; h.toasts = [];
    const raw = String.raw`provider error: OpenAI returned status 401 Unauthorized: <html>opaque $& {error}</html>  `;
    const reason = locale === "es"
      ? "OpenAI rechazó la clave API. Revísela en Ajustes → Proveedores."
      : "OpenAI rejected the API key. Check it in Settings → Providers.";
    h.handlers.onFailed({ type: "failed", error: raw });
    assert.equal(h.snapshot.error, reason);
    assert.equal(h.snapshot.cancelling, false);
    assert.equal(h.snapshot.cancelled, false);
    assert.equal(h.translating, false);
    assert.equal(h.jobId, null);
    assert.deepEqual(h.toasts, [["error", `${locale === "es" ? "La traducción falló" : "Translation failed"}: ${reason}`]]);
    assert.deepEqual(h.logs, [["error", t("activity.translation.failed"), raw, "translation"]]);
    h.handlers.onFailed({ type: "failed", error: raw });
    h.handlers.onBatchFailed({ type: "batch_failed", error: raw });
    h.handlers.onClosed();
    assert.equal(h.logs.length, 1);
    assert.equal(h.toasts.length, 1);
    setLocale("en");
  });

  test(`batch provider failure adds localized guidance, keeps raw detail once and continues in ${locale}`, () => {
    attach(`provider-batch-${locale}`);
    setLocale(locale);
    h.logs = []; h.toasts = [];
    const raw = "provider error: DeepL returned status 429 Too Many Requests: opaque response  ";
    const reason = locale === "es"
      ? "El proveedor está limitando las solicitudes. Espere y vuelva a intentarlo, o reduzca el tamaño del lote en Ajustes → Valores predeterminados de traducción."
      : "The provider is limiting requests. Wait and retry, or lower the batch size in Settings → Translation Defaults.";
    h.handlers.onBatchFailed({ type: "batch_failed", error: raw });
    assert.deepEqual(h.logs, [["warning", `${t("activity.translation.batchFailed")}: ${reason}`, raw, "translation"]]);
    assert.equal(h.snapshot.error, null);
    assert.equal(h.translating, true);
    assert.equal(h.toasts.length, 0);
    h.handlers.onCompleted({ type: "completed", total_translated: 1, total_cost: 0, cost_is_complete: true });
    assert.equal(h.snapshot.done, true);
    assert.equal(h.logs.filter(([, , detail]) => detail === raw).length, 1);
    setLocale("en");
  });
}

test("unknown and multiline batch failures keep the diagnostic only in raw detail", () => {
  setLocale("es");
  attach("unknown-batches");
  for (const raw of ["unknown provider failure", "provider error: OpenAI malformed response: first\nsecond\r\n  "]) {
    h.logs = [];
    h.handlers.onBatchFailed({ type: "batch_failed", error: raw });
    assert.deepEqual(h.logs, [["warning", t("activity.translation.batchFailed"), raw, "translation"]]);
  }
  h.handlers.onCompleted({ type: "completed", total_translated: 0, total_cost: 0, cost_is_complete: true });
  setLocale("en");
});
function enabledButton(markup: string, label: string): void {
  const button = [...markup.matchAll(/<button\b([^>]*)>(.*?)<\/button>/gs)]
    .find(([, , text]) => text === label);
  assert.ok(button, `expected a ${label} button`);
  assert.ok(!button[1].includes("disabled="), `${label} must stay enabled`);
}

for (const locale of ["en", "es"] as const) {
  test(`unexpected close preserves the cancellable job and connection status in ${locale}`, async (ctx) => {
    ctx.mock.timers.enable({ apis: ["setTimeout"] });
    setLocale(locale);
    attach(`disconnected-cancel-${locale}`);
    h.logs = []; h.toasts = []; h.cancellations = [];
    h.handlers.onClosed();
    assert.equal(h.jobId, `disconnected-cancel-${locale}`);
    assert.equal(session.subscribedTranslationJobId(), h.jobId);
    assert.equal(h.translating, true);
    assert.equal(h.snapshot.disconnected, true);
    assert.equal(h.snapshot.error, null);
    const modal = renderModal();
    assert.ok(modal.includes(locale === "es" ? "Conexión perdida" : "Connection lost"));
    enabledButton(modal, t("translate.cancel"));
    assert.deepEqual(h.toasts, [], "a transport failure is not a failed translation");
    await session.requestTranslationCancel();
    assert.deepEqual(h.cancellations, [`disconnected-cancel-${locale}`]);
    assert.equal(h.snapshot.cancelling, true);
    // A disconnected cancellation must also subscribe for its terminal replay.
    h.handlers.onFailed({ type: "failed", error: "cancelled" });
    assert.equal(h.snapshot.cancelled, true);
    assert.equal(h.snapshot.disconnected, false);
    assert.equal(h.jobId, null);
  });

  test(`exhausted backoff keeps Cancel and Reconnect available in ${locale}`, async (ctx) => {
    ctx.mock.timers.enable({ apis: ["setTimeout"] });
    setLocale(locale);
    h.subscriptions = [];
    attach(`retry-exhausted-${locale}`);
    h.handlers.onClosed();
    assert.equal(h.snapshot.disconnected, true);
    for (const [index, delay] of [1000, 2000, 4000].entries()) {
      ctx.mock.timers.tick(delay - 1);
      assert.equal(h.subscriptions.length, index + 1);
      ctx.mock.timers.tick(1);
      assert.equal(h.subscriptions.length, index + 2);
      h.handlers.onClosed();
      h.handlers.onClosed(); // Error followed by close must not consume two retries.
    }
    ctx.mock.timers.tick(60_000);
    assert.equal(h.subscriptions.length, 4, "initial subscription plus three retries");
    assert.ok(h.subscriptions.every(({ jobId }) => jobId === `retry-exhausted-${locale}`));
    assert.equal(h.snapshot.disconnected, true);
    assert.equal(h.snapshot.error, null);
    assert.equal(h.translating, true);
    session.setTranslationModalOpen(false);
    session.clearTranslationSnapshotIfIdle();
    attach(`retry-exhausted-${locale}`);
    session.setTranslationModalOpen(true);
    assert.equal(h.subscriptions.length, 4, "reopening must preserve the disconnected session");
    const modal = renderModal();
    enabledButton(modal, t("translate.cancel"));
    enabledButton(modal, locale === "es" ? "Reconectar" : "Reconnect");
    session.reconnectTranslationJob();
    assert.equal(h.subscriptions.length, 5);
    session.reconnectTranslationJob();
    assert.equal(h.subscriptions.length, 5, "manual reconnect must not overlap subscriptions");
    h.handlers.onOpen();
    assert.equal(h.snapshot.disconnected, false);
    h.handlers.onFailed({ type: "failed", error: "cancelled" });
    const subscriptions = h.subscriptions.length;
    ctx.mock.timers.tick(60_000);
    assert.equal(h.subscriptions.length, subscriptions);
  });
}

test("replay preserves progress and counts repeated batch failures once per occurrence", (ctx) => {
  ctx.mock.timers.enable({ apis: ["setTimeout"] });
  setLocale("en");
  h.subscriptions = []; h.logs = []; h.toasts = [];
  attach("replay-counts");
  const started = { type: "started", total: 5 };
  const failure = { type: "batch_failed", error: "provider unavailable" };
  const string = { type: "string_translated", entry_id: "entry-1", translation: "First" };
  const progress = { type: "batch_completed", completed: 1, total: 5, cost_so_far: 0.01 };
  const original = h.handlers;
  original.onStarted(started);
  original.onStringTranslated(string);
  original.onBatchFailed(failure);
  original.onBatchFailed(failure); // Two distinct failures can have identical payloads.
  original.onBatchCompleted(progress);
  original.onClosed();
  ctx.mock.timers.tick(1000);
  assert.equal(h.subscriptions.length, 2);
  const replay = h.handlers;
  replay.onStarted(started);
  replay.onStringTranslated(string);
  replay.onBatchFailed(failure);
  replay.onBatchFailed(failure);
  replay.onBatchCompleted(progress);
  assert.equal(h.snapshot.disconnected, false);
  assert.equal(h.snapshot.completed, 1);
  assert.equal(h.snapshot.batchFailures.count, 2);
  assert.equal(h.logs.filter(([, , detail]) => detail === failure.error).length, 2);
  original.onFailed({ type: "failed", error: "stale socket" });
  original.onClosed();
  assert.equal(h.jobId, "replay-counts");
  replay.onBatchFailed(failure);
  replay.onStringTranslated({ ...string, entry_id: "entry-2", translation: "Second" });
  replay.onBatchCompleted({ ...progress, completed: 2, cost_so_far: 0.02 });
  const completed = { type: "completed", total_translated: 2, total_cost: 0.02, cost_is_complete: true };
  replay.onCompleted(completed);
  replay.onCompleted(completed);
  replay.onStringTranslated(string);
  replay.onBatchFailed(failure);
  replay.onClosed();
  ctx.mock.timers.tick(60_000);
  assert.equal(h.snapshot.completed, 2);
  assert.equal(h.snapshot.lastTranslated, "Second");
  assert.equal(h.snapshot.batchFailures.count, 3);
  assert.equal(h.snapshot.costSoFar, 0.02);
  assert.equal(h.snapshot.done, true);
  assert.equal(h.snapshot.disconnected, false);
  assert.equal(h.jobId, null);
  assert.equal(h.translating, false);
  assert.deepEqual(h.toasts, [["warning", t("translate.completedWithErrorsSummary", { translated: 2, count: 3 })]]);
  assert.equal(h.subscriptions.length, 2);
});

test("job not found on resubscribe is terminal and stops retries", (ctx) => {
  ctx.mock.timers.enable({ apis: ["setTimeout"] });
  h.subscriptions = []; h.toasts = [];
  attach("expired-job");
  h.handlers.onClosed();
  ctx.mock.timers.tick(1000);
  assert.equal(h.subscriptions.length, 2);
  h.handlers.onFailed({ type: "failed", error: "job not found" });
  h.handlers.onClosed();
  ctx.mock.timers.tick(60_000);
  assert.equal(h.snapshot.error, t("api.error.jobNotFound"));
  assert.equal(h.snapshot.disconnected, false);
  assert.equal(h.jobId, null);
  assert.equal(session.subscribedTranslationJobId(), null);
  assert.equal(h.translating, false);
  assert.equal(h.toasts.length, 1);
  assert.equal(h.toasts[0][0], "error");
  assert.equal(h.subscriptions.length, 2);
});

test("replacing a disconnected job cancels its retry and ignores its late events", (ctx) => {
  ctx.mock.timers.enable({ apis: ["setTimeout"] });
  h.subscriptions = [];
  attach("old-disconnected");
  const old = h.handlers;
  old.onClosed();
  assert.equal(h.snapshot.disconnected, true);
  attach("replacement");
  old.onCompleted({ type: "completed", total_translated: 99, total_cost: 1 });
  old.onStarted({ type: "started", total: 99 });
  old.onClosed();
  ctx.mock.timers.tick(60_000);
  assert.equal(h.subscriptions.length, 2);
  assert.equal(h.jobId, "replacement");
  assert.equal(h.snapshot.completed, 0);
  h.handlers.onFailed({ type: "failed", error: "cancelled" });
});

test("replaying old events does not replenish automatic retries; new progress does", (ctx) => {
  ctx.mock.timers.enable({ apis: ["setTimeout"] });
  h.subscriptions = [];
  attach("interrupted-replay");
  const started = { type: "started", total: 5 };
  h.handlers.onStarted(started);
  h.handlers.onClosed();
  for (const delay of [1000, 2000, 4000]) {
    ctx.mock.timers.tick(delay);
    h.handlers.onOpen();
    h.handlers.onStarted(started);
    h.handlers.onClosed();
  }
  ctx.mock.timers.tick(60_000);
  assert.equal(h.subscriptions.length, 4);
  assert.equal(h.snapshot.disconnected, true);
  session.reconnectTranslationJob();
  h.handlers.onStarted(started);
  h.handlers.onClosed();
  ctx.mock.timers.tick(1000);
  h.handlers.onStarted(started);
  h.handlers.onBatchCompleted({ type: "batch_completed", completed: 1, total: 5, cost_so_far: 0 });
  h.handlers.onClosed();
  const count = h.subscriptions.length;
  ctx.mock.timers.tick(999);
  assert.equal(h.subscriptions.length, count);
  ctx.mock.timers.tick(1);
  assert.equal(h.subscriptions.length, count + 1, "fresh progress restarts backoff at one second");
  h.handlers.onFailed({ type: "failed", error: "cancelled" });
});

test("a terminal event arriving during backoff ends the job and clears the pending retry", (ctx) => {
  ctx.mock.timers.enable({ apis: ["setTimeout"] });
  h.subscriptions = []; h.toasts = [];
  attach("terminal-during-backoff");
  h.handlers.onStarted({ type: "started", total: 2 });
  h.handlers.onClosed();
  h.handlers.onStringTranslated({ type: "string_translated", translation: "late progress" });
  assert.equal(h.snapshot.lastTranslated, "");
  h.handlers.onCompleted({ type: "completed", total_translated: 2, total_cost: 0, cost_is_complete: true });
  assert.equal(h.snapshot.done, true);
  assert.equal(h.snapshot.disconnected, false);
  assert.equal(h.jobId, null);
  ctx.mock.timers.tick(60_000);
  assert.equal(h.subscriptions.length, 1);
  assert.equal(h.toasts.length, 1);
});
console.log("translationJobSession.test.ts: ok");
