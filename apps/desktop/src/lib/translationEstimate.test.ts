import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import { setLocale, t } from "./i18n/index.ts";
import { buildTranslationStartParams } from "./translationDefaults.ts";
import type { TranslationEstimate, TranslationStartParams } from "./api.ts";

// Exercise the public hook and rendered controls with deferred API promises.
// Slots survive rerenders; effects run after render, so synchronous invalidation
// is tested independently of effect cleanup (unlike a static markup-only test).
const h = { slots: [] as any[], cursor: 0, effects: [] as (() => void)[], cleanups: [] as (() => void)[],
  calls: [] as any[], project: { database_path: "a.db", path: "/a", format_id: "renpy" } as any,
  running: false, translating: false, requests: [] as any[], starts: [] as any[], queue: {} as any, config: {} as any, t };
(globalThis as any).__estimateTest = h;
const mocks: Record<string, string> = {
      react: `const h=globalThis.__estimateTest;
        export const useState=initial=>{const i=h.cursor++;if(!(i in h.slots))h.slots[i]=typeof initial==='function'?initial():initial;return [h.slots[i],v=>{h.slots[i]=typeof v==='function'?v(h.slots[i]):v;}];};
        export const useRef=initial=>{const i=h.cursor++;return h.slots[i]??={current:initial};};
        export const useEffect=(fn,deps)=>{const i=h.cursor++;const old=h.slots[i];if(!old||deps.some((d,j)=>!Object.is(d,old[j]))) {h.slots[i]=deps;h.effects.push(()=>{h.cleanups[i]?.();h.cleanups[i]=fn();});}};
        export const useSyncExternalStore=(_,snapshot)=>snapshot();
        export const useMemo=(fn,deps)=>{const i=h.cursor++;const old=h.slots[i];if(!old||deps.some((d,j)=>!Object.is(d,old.deps[j])))h.slots[i]={deps,value:fn()};return h.slots[i].value;};`,
      "./api": `export const estimateTranslation=params=>{const h=globalThis.__estimateTest;h.calls.push(params);return new Promise((resolve,reject)=>h.requests.push({resolve,reject}));};
        export const getConfig=()=>({});export const getProviders=()=>[];export const getFormats=()=>[];export const checkProviderHealth=()=>({});`,
      "../stores/projectStore": `export const useProjectStore={subscribe:()=>()=>{},getState:()=>({project:globalThis.__estimateTest.project})};`,
      "../stores/editorStore": `const snapshot=()=>({isTranslating:globalThis.__estimateTest.translating,jobSnapshot:null});export const useEditorStore=Object.assign(snapshot,{subscribe:()=>()=>{},getState:snapshot});`,
      "../stores/queueStore": `const snapshot=()=>({...globalThis.__estimateTest.queue,isRunning:globalThis.__estimateTest.running});export const useQueueStore=Object.assign(snapshot,{subscribe:()=>()=>{},getState:snapshot});`,
      "@tanstack/react-query": `export const useQuery=({queryKey})=>({data:queryKey[0]==='config'?globalThis.__estimateTest.config:queryKey[0]==='providers'?[{id:'mock',name:'Mock',available:true}]:[],isFetched:true});`,
      "react-router-dom": `export const useNavigate=()=>()=>{};`,
      "../lib/i18n": `export const useT=()=>globalThis.__estimateTest.t;export const useLocale=()=>({locale:'en'});`,
      "../lib/modalA11y": `export const useModalA11y=()=>({});export const MODAL_BACKDROP_CLASS='';export const MODAL_HEADER_CLASS='';export const MODAL_BODY_CLASS='';export const MODAL_FOOTER_CLASS='';export const modalPanelClass=x=>x;`,
      "../lib/translationStart": `export const startSingleTranslation=async params=>{globalThis.__estimateTest.starts.push(params);return null;};`,
      "../lib/translationJobSession": `export const setTranslationModalOpen=()=>{};export const clearTranslationSnapshotIfIdle=()=>{};export const reconnectTranslationJob=()=>{};export const requestTranslationCancel=()=>{};`,
};
const hooks = registerHooks({ resolve(specifier, context, nextResolve) {
  const parent = context.parentURL ?? "";
  if (["/lib/useTranslationEstimate.ts", "/components/TranslationModal.tsx", "/components/QueuePanel.tsx", "/lib/useTranslationSettings.ts"].some(path => parent.endsWith(path))) {
    const source = mocks[specifier === "../lib/api" ? "./api" : specifier];
    if (source) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(source)}` };
  }
  return nextResolve(specifier, context);
} });
const { useTranslationEstimate } = await import("./useTranslationEstimate.ts");
const { default: Estimate } = await import("../components/TranslationEstimate.tsx");
let enabled: boolean, config: any, params: TranslationStartParams, invalid: boolean;
function render(flush = true) {
  h.cursor = 0;
  const value = useTranslationEstimate({ enabled, config, buildParams: () => {
    if (invalid) throw Error("invalid form"); return params;
  } });
  if (flush) h.effects.splice(0).forEach(fn => fn());
  return value;
}
function markup(queue = false) { return renderToStaticMarkup(createElement(Estimate, { estimate: render(), queue })); }
const response: TranslationEstimate = { provider_id: "mock", effective_model: "Actual backend model",
  context_window: "18446744073709551615", approximate_input_allowance: "13835058055282163712",
  max_batch_tokens: "0", batch_size: 3, entry_count: 5, source_bytes: 123,
  cost_known: true, estimated_cost_usd: 0, approximate: true };
async function finish(index = 0, value = response) { h.requests[index].resolve(value); await Promise.resolve(); await Promise.resolve(); }
beforeEach(() => {
  h.cleanups.forEach(fn => fn?.()); h.slots = []; h.effects = []; h.cleanups = []; h.cursor = 0;
  h.calls = []; h.requests = []; h.running = false; h.translating = false; h.starts = [];
  h.config = {}; h.queue = { items: [{ id: 'queued', projectName: 'Not the open game', projectPath: '/never-open', status: 'pending', progress: {} }],
    isPanelOpen: true, translationParams: null, history: [], setParams: (p: any) => h.starts.push(p), startQueue() {} };
  h.project = { database_path: "a.db", path: "/a", format_id: "renpy" };
  enabled = true; invalid = false; config = { model_overrides: {} };
  params = buildTranslationStartParams({ providerId: "mock", fallbackIds: ["other"], sourceLang: "en", targetLang: "es",
    batchSize: 3, maxConcurrent: 2, costLimit: "1.5", gameContext: " Setting " });
  setLocale("en");
});
after(() => { hooks.deregister(); Reflect.deleteProperty(globalThis, "__estimateTest"); setLocale("en"); });

test("only an explicit action requests; exact start options, no fallback or project-opening payload", async () => {
  assert.equal(render().status, "notRequested"); render(); markup(true);
  assert.deepEqual(h.calls, []);
  const pending = render().refresh(); render().refresh();
  assert.equal(render().pending, true);
  assert.deepEqual(h.calls, [{ provider_id: params.provider_id, options: params.options }]);
  await finish(); await pending;
  assert.equal(render().pending, false); assert.equal(render().status, "ready");
  render(); assert.equal(h.calls.length, 1);
});
for (const locale of ["en", "es"] as const) {
  test(`${locale}: distinct no-project, not-requested, loading, unavailable, unknown, zero and known cost`, async () => {
    setLocale(locale);
    h.project = null; assert.ok(markup().includes(t("estimate.noProject"))); await render().refresh();
    assert.equal(h.calls.length, 0);
    h.project = { database_path: "a.db", path: "/a", format_id: "renpy" };
    assert.ok(markup().includes(t("estimate.notRequested")));
    const pending = render().refresh(); assert.ok(markup().includes(t("estimate.loading")));
    await finish(); await pending;
    const html = markup(true);
    for (const text of ["$0.0000", response.effective_model!, response.context_window!, response.approximate_input_allowance!,
      t("estimate.queueScope"), t("estimate.disclaimer"), t("estimate.scope")]) assert.ok(html.includes(text), text);
    assert.ok(!html.includes("18446744073709552000"));
    const unknown = render().refresh(); await finish(1, { ...response, cost_known: false, estimated_cost_usd: null,
      effective_model: null, context_window: null, approximate_input_allowance: null, max_batch_tokens: null }); await unknown;
    assert.ok(markup().includes(t("estimate.unknownCost"))); assert.ok(!markup().includes("$0.0000"));
    assert.ok(markup().includes(t("estimate.unspecified")));
    const known = render().refresh(); await finish(2, { ...response, estimated_cost_usd: 2.5 }); await known;
    assert.ok(markup().includes("$2.5000"));
    const failed = render().refresh(); h.requests[3].reject(Error("busy")); await failed;
    assert.ok(markup().includes(t("estimate.unavailable"))); assert.equal(render().pending, false);
  });
}
const changes: Record<string, () => void> = {
  provider: () => { params = { ...params, provider_id: params.provider_id + "x" }; },
  options: () => { params = { ...params, options: { ...params.options, use_memory: !params.options.use_memory } }; },
  database: () => { h.project = { ...h.project, database_path: h.project.database_path + "x" }; },
  game: () => { h.project = { ...h.project, path: h.project.path + "x" }; },
  format: () => { h.project = { ...h.project, format_id: h.project.format_id + "x" }; },
  policy: () => { config.model_overrides.mock = { Model: { context_window: (config.model_overrides.mock?.Model.context_window ?? 0) + 4096 } }; },
  config: () => { config = { ...config, providers: { mock: { model: (config.providers?.mock.model ?? "") + "x" } } }; },
  hidden: () => { enabled = false; render(false); enabled = true; },
  queue: () => { h.running = true; render(false); h.running = false; },
  single: () => { h.translating = true; render(false); h.translating = false; },
};
for (const [name, change] of Object.entries(changes)) {
  test(`${name}: invalidate synchronously; obsolete success/error/finalizer cannot replace or unblock newer request`, async () => {
    const first = render().refresh(); change();
    assert.equal(render(false).status, "notRequested"); assert.equal(render(false).pending, false);
    const current = render().refresh();
    await finish(0); await first;
    assert.equal(render().status, "loading"); assert.equal(render().pending, true);
    await finish(1); await current;
    assert.equal(render().status, "ready");
    change(); assert.equal(render(false).status, "notRequested", "no frame showing old success");
    const obsolete = render().refresh(); change(); render(); const newest = render().refresh();
    h.requests[2].reject(Error("old failure")); await obsolete;
    assert.equal(render().status, "loading"); assert.equal(render().pending, true);
    await finish(3); await newest; assert.equal(render().status, "ready");
  });
}
test("hidden/activity/unmount refuse requests and invalidate pending completion; errors can be retried", async () => {
  const old = render().refresh(); h.cleanups.forEach(fn => fn?.()); await finish(); await old;
  assert.notEqual(render().status, "ready");
  for (const key of ["hidden", "queue", "single"]) {
    enabled = key !== "hidden"; h.running = key === "queue"; h.translating = key === "single";
    await render().refresh(); assert.equal(h.calls.length, 1);
  }
});
test("invalid builder is unavailable without throwing, requesting or manufacturing zero cost", async () => {
  invalid = true; assert.equal(render().status, "unavailable"); await render().refresh();
  assert.deepEqual(h.calls, []); assert.equal(render().pending, false); assert.ok(!markup().includes("$0.0000"));
});
test("fallback selection is excluded from estimate input/identity; start payload remains intact", async () => {
  const pending = render().refresh(); await finish(); await pending;
  params = { ...params, fallback_provider_ids: ["different"] };
  assert.equal(render().status, "ready"); assert.deepEqual(params.fallback_provider_ids, ["different"]);
});

const { default: Modal } = await import("../components/TranslationModal.tsx");
const { default: Queue } = await import("../components/QueuePanel.tsx");
function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  return Array.isArray(node) ? node.flatMap(nodes) : [node, ...nodes(node.props?.children)];
}
function form(kind: "modal" | "queue", open = true) {
  h.cursor = 0; h.queue.isPanelOpen = open;
  const tree = kind === "modal" ? Modal({ open, totalPending: 5, onClose() {}, onComplete() {} }) : Queue();
  h.effects.splice(0).forEach(fn => fn()); return tree;
}
function control(kind: "modal" | "queue", field: string) {
  const id = `${kind === "modal" ? "translation" : "queue"}-${field}`;
  const node = nodes(form(kind)).find(n => n.props?.id === id); assert.ok(node, id); return node;
}
function start(kind: "modal" | "queue") {
  const node = nodes(form(kind)).find(n => n.type === "button" && (kind === "modal"
    ? n.props.children === t("translate.start") : n.props.onClick?.name === "handleStart"));
  assert.ok(node); return node;
}
function estimate(kind: "modal" | "queue") {
  const node = nodes(form(kind)).find(n => n.type === Estimate); assert.ok(node); return node.props.estimate;
}
for (const kind of ["modal", "queue"] as const) {
  test(`${kind}: exact start/estimate wire options, pending Start release, context and modal flags`, async () => {
    form(kind); form(kind); assert.equal(h.calls.length, 0);
    for (const [field, value] of [["provider", "mock"], ["source", "ja"], ["target", "en"], ["gameContext", " Local context "],
      ["batchSize", "7"], [kind === "modal" ? "parallelRequests" : "maxConcurrent", "4"], ["costLimit", "2.5"]]) {
      control(kind, field).props.onChange({ target: { value } });
    }
    if (kind === "modal") {
      const switches = nodes(form(kind)).filter(n => n.type === "input" && n.props.type === "checkbox");
      assert.equal(switches.length, 2); switches.forEach(n => n.props.onChange({ target: { checked: false } }));
    }
    const options = { source_lang: "ja", target_lang: "en", game_context: "Local context", batch_size: 7, max_concurrent: 4,
      cost_limit_usd: 2.5, use_glossary: kind !== "modal", use_memory: kind !== "modal", skip_approved: true };
    const pending = estimate(kind).refresh();
    assert.equal(start(kind).props.disabled, true); await start(kind).props.onClick(); assert.deepEqual(h.starts, []);
    assert.deepEqual(h.calls, [{ provider_id: "mock", options }]);
    h.requests[0].reject(Error("busy")); await pending;
    assert.equal(start(kind).props.disabled, false); await start(kind).props.onClick();
    assert.deepEqual(h.starts, [{ provider_id: "mock", options }]);
    assert.equal(h.project.path, "/a", "no queued project was opened");
    const view = nodes(form(kind)).find(n => n.type === Estimate);
    assert.equal(view.props.queue ?? false, kind === "queue");
    control(kind, "costLimit").props.onChange({ target: { value: "invalid" } });
    assert.equal(estimate(kind).status, "unavailable"); assert.equal(start(kind).props.disabled, false);
  });
}
test("modal resets per-open defaults; queue retains persisted settings across hide/reopen", () => {
  const storage = new Map<string, string>();
  const old = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
    getItem: (k: string) => storage.get(k) ?? null, setItem: (k: string, v: string) => storage.set(k, v),
  } });
  try {
    h.config = { default_source_lang: "ja", default_target_lang: "de" };
    form("modal"); form("modal"); control("modal", "target").props.onChange({ target: { value: "fr" } });
    form("modal", false); form("modal"); assert.equal(control("modal", "target").props.value, "de");
    h.cleanups.forEach(fn => fn?.()); h.slots = []; h.cleanups = []; h.effects = [];
    h.queue.translationParams = { ...params, options: { ...params.options, target_lang: "pt", game_context: "Persisted context" } };
    form("queue"); form("queue"); assert.equal(control("queue", "target").props.value, "pt");
    assert.equal(control("queue", "gameContext").props.value, "Persisted context");
    control("queue", "target").props.onChange({ target: { value: "fr" } });
    form("queue", false); form("queue"); assert.equal(control("queue", "target").props.value, "fr");
    assert.deepEqual(h.calls, []);
  } finally {
    if (old) Object.defineProperty(globalThis, "localStorage", old); else Reflect.deleteProperty(globalThis, "localStorage");
  }
});
