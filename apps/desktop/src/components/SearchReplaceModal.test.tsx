import assert from "node:assert/strict";
import { after, test } from "node:test";
import { registerHooks } from "node:module";
import * as a11y from "../lib/modalA11y.ts";
import { setLocale, t } from "../lib/i18n";
import type { StringEntry } from "../lib/api";

const row: StringEntry = { id: "row", source: "Source", translation: "old text", status: "translated", tags: [], metadata: {}, created_at: "2026-01-01", file_path: "story.html", context: null, provider_used: null, char_limit: null, translated_at: null, reviewed_at: null };
const h = {
  t, a11y, snapshot: null as any, estimatePending: false, states: [] as any[], cursor: 0, closed: 0, done: 0,
  setTauri: (_value: boolean) => {},
  reads: [] as any[], writes: [] as any[], toasts: [] as [string, string][], logs: [] as any[],
  entries: [row], total: 1,
  report: { requested: 1, applied: 1, skipped: 0, conflicts: [] as string[] },
  read(filter: any) { h.reads.push(filter); return { entries: h.entries, total: h.total }; },
  write(body: any) { h.writes.push(body); return h.report; },
};
(globalThis as any).__bulkReplace = h;
const hooks = registerHooks({
  load(url, context, nextLoad) {
    if (url.endsWith("/src/lib/runtime.ts")) return {
      format: "module", shortCircuit: true,
      source: "export let IS_TAURI=true;export const isTauri=()=>IS_TAURI;globalThis.__bulkReplace.setTauri=value=>{IS_TAURI=value;};",
    };
    return nextLoad(url, context);
  },
  resolve(specifier, context, nextResolve) {
    if (["/components/SearchReplaceModal.tsx", "/components/TranslationModal.tsx"].some(path => context.parentURL?.endsWith(path))) {
      const stubs: Record<string, string> = {
        react: "export const useState=initial=>{const i=h.cursor++;if(!(i in h.states))h.states[i]=initial;return [h.states[i],value=>{h.states[i]=value;}];};export const useEffect=()=>{};export const useRef=initial=>({current:initial});",
        "../lib/i18n": "export const useT=()=>h.t;export const useLocale=()=>({locale:'en'});",
        "@tanstack/react-query": "export const useQuery=({queryKey})=>({data:queryKey[0]==='providers'?[]:{},isFetched:true});",
        "react-router-dom": "export const useNavigate=()=>()=>{};",
        "../stores/editorStore": "export const useEditorStore=()=>({isTranslating:!!h.snapshot,jobSnapshot:h.snapshot});",
        "../stores/projectStore": "export const useProjectStore={getState:()=>({project:null})};",
        "../lib/useTranslationSettings": "export const useTranslationSettings=()=>({fallbackIds:[],fallbackPick:'',providerReadiness:{ready:true}});",
        "../lib/useTranslationEstimate": "export const useTranslationEstimate=()=>({pending:h.estimatePending});",
        "../lib/translationStart": "export const startSingleTranslation=async()=>null;",
        "../lib/translationJobSession": "export const clearTranslationSnapshotIfIdle=()=>{};export const reconnectTranslationJob=()=>{};export const requestTranslationCancel=()=>{};export const setTranslationModalOpen=()=>{};",
        "../stores/toastStore": "export const addToast=(...args)=>h.toasts.push(args);",
        "../stores/logStore": "export const addLog=(...args)=>h.logs.push(args);",
        "../lib/modalA11y": "export const useModalA11y=()=>({dialogRef:{current:null},dialogProps:h.a11y.buildModalDialogProps({titleId:'title'}),titleProps:h.a11y.buildModalTitleProps('title')});export const MODAL_BACKDROP_CLASS=h.a11y.MODAL_BACKDROP_CLASS;export const MODAL_FOOTER_CLASS=h.a11y.MODAL_FOOTER_CLASS;export const modalPanelClass=h.a11y.modalPanelClass;",
      };
      if (stubs[specifier]) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__bulkReplace;" + stubs[specifier])}` };
    }
    return nextResolve(specifier, context);
  },
});
const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
const originalFetch = globalThis.fetch;
Object.defineProperty(globalThis, "window", { configurable: true, value: {
  __TAURI_INTERNALS__: { invoke: async (command: string, args: any) => {
    if (command === "get_server_port") return 7842;
    if (command === "get_strings") return h.read(args.filter);
    if (command === "batch_patch_strings") return h.write(args.data);
    throw new Error(`Unexpected command: ${command}`);
  } },
} });
globalThis.fetch = async (input, init) => {
  const url = new URL(String(input));
  if (url.pathname === "/api/project/current") return new Response("null");
  if (url.pathname === "/api/strings/batch") {
    assert.equal(init?.method, "POST");
    return new Response(JSON.stringify(h.write(JSON.parse(String(init?.body)))));
  }
  assert.equal(url.pathname, "/api/strings");
  return new Response(JSON.stringify(h.read(Object.fromEntries(url.searchParams))));
};
const { default: SearchReplaceModal } = await import("./SearchReplaceModal");
const { default: TranslationModal } = await import("./TranslationModal");
// Resolve the real client's HTTP base through its desktop port seam first.
await (await import("../lib/api")).getCurrentProject();
after(() => {
  hooks.deregister();
  globalThis.fetch = originalFetch;
  if (windowDescriptor) Object.defineProperty(globalThis, "window", windowDescriptor);
  else Reflect.deleteProperty(globalThis, "window");
  Reflect.deleteProperty(globalThis, "__bulkReplace");
  setLocale("en");
});
function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(nodes);
  return [node, ...nodes(node.props?.children)];
}
function render() {
  h.cursor = 0;
  return SearchReplaceModal({ open: true, onClose() { h.closed++; }, onDone() { h.done++; } });
}
function reset(locale: "en" | "es" = "en") {
  h.states = []; h.reads = []; h.writes = []; h.toasts = []; h.logs = []; h.closed = 0; h.done = 0;
  h.entries = [row]; h.total = 1;
  h.report = { requested: 1, applied: 1, skipped: 0, conflicts: [] };
  h.setTauri(true);
  setLocale(locale);
  const inputs = nodes(render()).filter(n => n.type === "input" && n.props.type !== "checkbox");
  inputs[0].props.onChange({ target: { value: "old" } });
  inputs[1].props.onChange({ target: { value: "new" } });
}
async function click(key: "replace.previewBtn" | "replace.replaceAll") {
  const button = nodes(render()).find(n => n.type === "button" && n.props.children === t(key));
  assert.ok(button);
  assert.equal(button.props.disabled, false);
  await button.props.onClick();
  await new Promise(resolve => setImmediate(resolve));
}

for (const tauri of [true, false]) {
  test(`${tauri ? "Tauri" : "HTTP"}: sends each read translation as the batch baseline`, async () => {
    reset(); h.setTauri(tauri);
    h.entries = [row, { ...row, id: "second", translation: "old old" }, { ...row, id: "null", translation: null }];
    h.total = 3; h.report = { requested: 2, applied: 2, skipped: 0, conflicts: [] };
    await click("replace.replaceAll");
    assert.deepEqual(h.writes, [{ provider: "search-replace", updates: [
      { id: "row", translation: "new text", expected_translation: "old text" },
      { id: "second", translation: "new new", expected_translation: "old old" },
    ] }]);
    assert.equal(h.done, 1);
    assert.equal(h.toasts[0][0], "success");
  });
}

const conflictMessages = {
  en: "Applied 1 of 2 translation(s). 1 row(s) changed meanwhile and were left untouched. Run the preview again before retrying.",
  es: "Se aplicaron 1 de 2 traducción(es). 1 fila(s) cambiaron mientras tanto y se dejaron intactas. Ejecute de nuevo la vista previa antes de reintentar.",
};
for (const locale of ["en", "es"] as const) {
  test(`${locale}: conflicts show applied count and recovery warning without closing the modal`, async () => {
    reset(locale);
    h.entries = [row, { ...row, id: "changed", translation: "old old" }]; h.total = 2;
    h.report = { requested: 2, applied: 1, skipped: 1, conflicts: ["changed"] };
    await click("replace.replaceAll");
    assert.deepEqual(h.toasts, [["warning", conflictMessages[locale]]]);
    assert.equal(h.done, 1);
    assert.equal(h.closed, 0, "keep preview available for recovery");
    assert.ok(!JSON.stringify(h.logs).includes("3 hits"), "skipped occurrences are not applied");
  });
}

for (const action of ["replace.previewBtn", "replace.replaceAll"] as const) {
  test(`${action}: refuses an incomplete 50001-row result without claiming success`, async () => {
    reset(); h.total = 50_001;
    await click(action);
    assert.equal(h.writes.length, 0, "must refuse before writing a partial batch");
    assert.equal(h.done, 0);
    assert.equal(h.closed, 0);
    assert.deepEqual(h.toasts, [["warning", "Loaded 1 of 50001 matching rows. No changes were made. Narrow the search and run the preview again."]]);
    assert.equal(h.logs.length, 0);
  });
}

test("all-conflict batches report zero applied, retain recovery, and never show success", async () => {
  reset(); h.report = { requested: 1, applied: 0, skipped: 1, conflicts: ["row"] };
  await click("replace.replaceAll");
  assert.equal(h.toasts[0][0], "warning");
  assert.match(h.toasts[0][1], /Applied 0 of 1/);
  assert.equal(h.closed, 0);
});

test("missing rows use a partial warning without claiming all attempted occurrences were replaced", async () => {
  reset(); h.report = { requested: 1, applied: 0, skipped: 1, conflicts: [] };
  await click("replace.replaceAll");
  assert.deepEqual(h.toasts, [["warning", "Applied 0 of 1 translation(s); 1 skipped. Run the preview again before retrying."]]);
  assert.equal(h.closed, 0);
});

function assertClasses(node: any, expected: string[]) {
  assert.ok(node, "expected modal region to render");
  const classes = new Set(node.props.className?.split(/\s+/));
  for (const token of expected) assert.ok(classes.has(token), `missing ${token}`);
}
function boundedRegions(tree: any) {
  const panel = nodes(tree).find(n => n.props?.role === "dialog");
  assertClasses(panel, ["max-h-[calc(100vh-2rem)]", "my-4", "flex", "flex-col"]);
  const regions = panel.props.children;
  assert.ok(Array.isArray(regions), "header, body, and footer must be siblings");
  assert.equal(regions.length, 3);
  const [header, body, footer] = regions;
  assert.equal(header.type, "header");
  assertClasses(header, ["shrink-0", "border-b", "border-border"]);
  assert.ok(nodes(header).some(n => n.type === "h2" && n.props.id === "title"));
  assert.ok(nodes(header).some(n => n.type === "button"));
  assertClasses(body, ["flex-1", "min-h-0", "overflow-y-auto", "p-6"]);
  assert.equal(footer.type, "footer");
  assertClasses(footer, ["shrink-0", "border-t", "border-border", "bg-surface"]);
  assert.ok(!footer.props.className.includes("overflow-y-auto"));
  return { body, footer };
}
function assertFooterButton(footer: any, label: string) {
  const button = nodes(footer).find(n => n.type === "button" && n.props.children === label);
  assert.ok(button, `${label} must render in the fixed footer`);
  return button;
}

test("Search/Replace bounds its frame, scrolls the preview body, and keeps all actions in the footer", async () => {
  reset();
  await click("replace.previewBtn");
  const { body, footer } = boundedRegions(render());
  const preview = nodes(body).find(n => n.type === "ul");
  assertClasses(preview, ["overflow-y-auto"]);
  for (const key of ["common.cancel", "replace.previewBtn", "replace.replaceAll"] as const) {
    assertFooterButton(footer, t(key));
  }
});

for (const pending of [false, true]) {
  test(`Translation bounds its frame and keeps Start in the footer (estimate pending: ${pending})`, () => {
    reset(); h.states = []; h.cursor = 0; h.snapshot = null; h.estimatePending = pending;
    const tree = TranslationModal({ open: true, onClose() {}, totalPending: 1, onComplete() {} });
    const { body, footer } = boundedRegions(tree);
    assert.ok(nodes(body).some(n => n.props?.estimate?.pending === pending), "estimate stays in the scrolling body");
    assert.equal(assertFooterButton(footer, t("translate.start")).props.disabled, pending);
  });
}

for (const snapshot of [{ completed: 1, total: 2 }, { done: true, completed: 2, total: 2 }]) {
  test(`Translation keeps progress actions in the footer (done: ${!!("done" in snapshot)})`, () => {
    reset(); h.states = []; h.cursor = 0; h.snapshot = snapshot;
    const tree = TranslationModal({ open: true, onClose() {}, totalPending: 1, onComplete() {}, onReview() {} });
    const { footer } = boundedRegions(tree);
    if ("done" in snapshot) {
      assertFooterButton(footer, t("common.close"));
      assertFooterButton(footer, t("translate.review"));
    } else {
      assertFooterButton(footer, t("translate.cancel"));
    }
  });
}
