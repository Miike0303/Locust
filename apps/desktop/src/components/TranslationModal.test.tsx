import assert from "node:assert/strict";
import { after, beforeEach, test } from "node:test";
import { registerHooks } from "node:module";
import { setLocale, t } from "../lib/i18n";
import { buildTranslationStartParams } from "../lib/translationDefaults";

const project = { name: "Fixture", path: "game", database_path: "game/project.db", format_id: "rpgmaker_mv" };
const h = {
  t, locale: "en", project: { ...project }, editor: { isTranslating: false, selectedEntryId: "selected", jobSnapshot: null as any },
  states: [] as any[], cursor: 0, changed: false, effects: [] as (() => void)[],
  calls: [] as any[], handlers: null as any, unsubscribed: 0,
  state(initial: any) {
    const index = this.cursor++;
    if (!(index in this.states)) this.states[index] = typeof initial === "function" ? initial() : initial;
    return [this.states[index], (value: any) => {
      const next = typeof value === "function" ? value(this.states[index]) : value;
      if (!Object.is(next, this.states[index])) this.changed = true;
      this.states[index] = next;
    }];
  },
  effect(fn: () => void, deps: unknown[]) {
    const index = this.cursor++;
    const previous = this.states[index] as unknown[] | undefined;
    if (!previous || deps.some((value, i) => !Object.is(value, previous[i]))) {
      this.states[index] = deps;
      this.effects.push(fn);
    }
  },
};
(globalThis as any).__translationModal = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/components/TranslationModal.tsx")) {
      const stubs: Record<string, string> = {
        react: "export const useState=initial=>h.state(initial);export const useRef=initial=>h.state(()=>({current:initial}))[0];export const useEffect=(fn,deps)=>h.effect(fn,deps);",
        "@tanstack/react-query": "export const useQuery=options=>options.queryKey[0]==='providers'?{data:[{id:'google',name:'Google',is_free:true,requires_api_key:false}]}:options.queryKey[0]==='config'?{data:{default_provider:'google'},isFetched:true,isError:false}:{data:[{file_path:'menus.json'},{file_path:'intro_日本語.json'}]};",
        "react-router-dom": "export const useNavigate=()=>()=>{};",
        "../lib/api": "export const getProviders=()=>{};export const getConfig=()=>{};export const getFileStats=()=>{};export const checkProviderHealth=async()=>({ok:true,message:'ok'});",
        "../lib/useTranslationSettings": "export const useTranslationSettings=()=>({fallbackIds:[],setFallbackIds:()=>{},fallbackPick:'',setFallbackPick:()=>{},addFallback:()=>{},removeFallback:()=>{},providerReadiness:{ready:true}});",
        "../lib/useTranslationEstimate": "export const useTranslationEstimate=()=>({pending:false});",
        "./TranslationEstimate": "export default ()=>null;",
        "../lib/translationStart": "export const startSingleTranslation=async params=>{h.calls.push(params);h.editor.isTranslating=true;h.editor.jobSnapshot=null;return {job_id:'job-'+h.calls.length};};",
        "../lib/translationJobSession": "export const clearTranslationSnapshotIfIdle=()=>{};export const reconnectTranslationJob=()=>{};export const requestTranslationCancel=async()=>{};export const setTranslationModalOpen=()=>{};",
        "../lib/ws": "export const subscribeToJob=(_id,handlers)=>{h.handlers=handlers;return ()=>{h.unsubscribed++;};};",
        "../stores/editorStore": "export const useEditorStore=()=>h.editor;",
        "../stores/projectStore": "export const useProjectStore=()=>({project:h.project});useProjectStore.getState=()=>({project:h.project});",
        "../lib/projectScope": "export const draftProjectKey=p=>JSON.stringify([p.database_path,p.path,p.format_id]);",
        "../stores/logStore": "export const addLog=()=>{};",
        "../stores/toastStore": "export const addToast=()=>{};",
        "../lib/modalA11y": "export const useModalA11y=()=>({dialogRef:{current:null},dialogProps:{role:'dialog'},titleProps:{id:'title'}});export const MODAL_BACKDROP_CLASS='modal';export const modalPanelClass=x=>x;",
        "../lib/i18n": "export const useT=()=>h.t;export const useLocale=()=>({locale:h.locale});",
      };
      if (stubs[specifier]) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__translationModal;" + stubs[specifier])}` };
    }
    return nextResolve(specifier, context);
  },
});
const { default: TranslationModal } = await import("./TranslationModal");
after(() => { hooks.deregister(); Reflect.deleteProperty(globalThis, "__translationModal"); setLocale("en"); });
beforeEach(() => {
  h.states = []; h.cursor = 0; h.effects = []; h.calls = []; h.handlers = null; h.unsubscribed = 0;
  h.project = { ...project }; h.editor = { isTranslating: false, selectedEntryId: "selected", jobSnapshot: null };
  h.locale = "en"; setLocale("en");
});

function render(extra: Record<string, any> = {}): any {
  for (let pass = 0; pass < 30; pass++) {
    h.cursor = 0; h.changed = false; h.effects = [];
    const tree = TranslationModal({ open: true, onClose() {}, totalPending: 12, onComplete() {}, ...extra });
    const effects = h.effects; h.effects = [];
    effects.forEach(effect => effect());
    if (!h.changed) return tree;
  }
  throw new Error("modal did not settle");
}
function nodes(tree: any): any[] {
  if (tree == null || typeof tree !== "object") return [];
  if (Array.isArray(tree)) return tree.flatMap(nodes);
  return [tree, ...nodes(tree.props?.children)];
}
function text(tree: any): string {
  if (tree == null || typeof tree === "boolean") return "";
  if (typeof tree === "string" || typeof tree === "number") return String(tree);
  if (Array.isArray(tree)) return tree.map(text).join("");
  return text(tree.props?.children);
}
function find(tree: any, predicate: (node: any) => boolean): any {
  const node = nodes(tree).find(predicate); assert.ok(node, "control exists"); return node;
}
function radio(tree: any, key: "all" | "selected" | "files") {
  const label = find(tree, node => node.type === "label" && text(node).includes(t(`translate.scope.${key}`)));
  return find(label, node => node.type === "input");
}
function start(tree: any) { return find(tree, node => node.type === "button" && text(node) === t("translate.start")); }

for (const locale of ["en", "es"] as const) {
  test(`scope controls default to all pending and localize in ${locale}`, () => {
    h.locale = locale; setLocale(locale);
    const tree = render();
    assert.equal(radio(tree, "all").props.checked, true);
    for (const key of ["scope", "scope.selected", "scope.files", "priority", "priorityHint"]) assert.ok(text(tree).includes(t(`translate.${key}` as any)));
    assert.equal(start(tree).props.disabled, false);
  });
}

test("selected rows submit the captured IDs without file scope", async () => {
  const selectedIds = ["a", "b"];
  let tree = render({ selectedIds }); radio(tree, "selected").props.onChange();
  tree = render({ selectedIds });
  await start(tree).props.onClick();
  selectedIds.push("later-selection");
  assert.deepEqual(h.calls[0].options.entry_ids, ["a", "b"]);
  assert.equal(h.calls[0].options.file_paths, undefined);
});

test("file multi-select and priority preserve exact paths and order", async () => {
  let tree = render(); radio(tree, "files").props.onChange(); tree = render();
  assert.equal(start(tree).props.disabled, true);
  const select = find(tree, node => node.props?.id === "translation-scope-files");
  assert.equal(select.props.multiple, true);
  select.props.onChange({ target: { selectedOptions: [{ value: "menus.json" }, { value: "intro_日本語.json" }] } });
  find(tree, node => node.props?.id === "translation-priority").props.onChange({ target: { value: "intro_日本語.json\nmenus.json\nintro_日本語.json" } });
  tree = render(); await start(tree).props.onClick();
  assert.deepEqual(h.calls[0].options.file_paths, ["menus.json", "intro_日本語.json"]);
  assert.deepEqual(h.calls[0].options.file_priority, ["intro_日本語.json", "menus.json"]);
  assert.equal(h.calls[0].options.entry_ids, undefined);
});

test("no editor selection hides selected rows and clearing one never widens scope", () => {
  let tree = render(); radio(tree, "selected").props.onChange(); tree = render();
  h.editor.selectedEntryId = ""; tree = render();
  assert.ok(!text(tree).includes(t("translate.scope.selected")));
  assert.equal(start(tree).props.disabled, true);
});

test("retry failed only uses the receipt and prior options despite changed editor selection", async () => {
  let tree = render(); radio(tree, "selected").props.onChange(); tree = render();
  await start(tree).props.onClick();
  const first = h.calls[0];
  h.handlers.onCompleted({ failed_ids: ["failed-2"] });
  h.editor.isTranslating = false;
  h.editor.jobSnapshot = { done: true, cancelled: false, error: null, completed: 1, total: 2, batchFailures: { count: 1, lastReason: "503" } };
  h.editor.selectedEntryId = "unrelated";
  tree = render();
  const retry = find(tree, node => node.type === "button" && text(node) === t("translate.retryFailed", { count: 1 }));
  await retry.props.onClick(); await Promise.resolve();
  assert.equal(h.calls.length, 2);
  assert.deepEqual(h.calls[1], { ...first, options: { ...first.options, file_paths: undefined, entry_ids: ["failed-2"] } });
});

test("project changes discard the old receipt and restore all pending", async () => {
  await start(render()).props.onClick();
  h.handlers.onCompleted({ failed_ids: ["old"] });
  h.editor.isTranslating = false;
  h.editor.jobSnapshot = { done: true, cancelled: false, error: null };
  h.project = { ...project, database_path: "other/project.db" };
  const tree = render();
  assert.ok(!text(tree).includes(t("translate.retryFailed", { count: 1 })));
  assert.ok(h.unsubscribed > 0);
});

test("a stale start handler cannot submit one project's selected rows to another project", async () => {
  let tree = render(); radio(tree, "selected").props.onChange(); tree = render();
  const submit = start(tree).props.onClick;
  h.project = { ...project, database_path: "other/project.db" };
  await submit();
  assert.equal(h.calls.length, 0);
});

test("a stale retry handler cannot submit the previous project's failed IDs", async () => {
  await start(render()).props.onClick();
  h.handlers.onCompleted({ failed_ids: ["failed"] });
  h.editor.isTranslating = false;
  h.editor.jobSnapshot = { done: true, error: null };
  const retry = find(render(), node => node.type === "button" && text(node) === t("translate.retryFailed", { count: 1 }));
  h.project = { ...project, database_path: "other/project.db" };
  await retry.props.onClick();
  assert.equal(h.calls.length, 1);
});

test("terminal failures retain a scoped retry receipt and cancellation clears it", async () => {
  await start(render()).props.onClick();
  h.handlers.onFailed({ error: "cost limit exceeded", failed_ids: ["failed"] });
  h.editor.isTranslating = false;
  h.editor.jobSnapshot = { done: false, error: "cost limit exceeded" };
  const retry = find(render(), node => node.type === "button" && text(node) === t("translate.retryFailed", { count: 1 }));
  await retry.props.onClick(); await Promise.resolve();
  assert.equal(h.calls.length, 2);
  assert.deepEqual(h.calls[1].options.entry_ids, ["failed"]);
  h.handlers.onFailed({ error: "cancelled", failed_ids: ["failed"] });
  h.editor.isTranslating = false;
  h.editor.jobSnapshot = { cancelled: true, error: null };
  assert.ok(!text(render()).includes(t("translate.retryFailed", { count: 1 })));
});

test("obsolete terminal frames cannot close a later job's receipt subscription", async () => {
  await start(render()).props.onClick();
  const obsolete = h.handlers;
  obsolete.onCompleted({ failed_ids: ["failed"] });
  h.editor.isTranslating = false;
  h.editor.jobSnapshot = { done: true, error: null };
  const retry = find(render(), node => node.type === "button" && text(node) === t("translate.retryFailed", { count: 1 }));
  await retry.props.onClick(); await Promise.resolve();
  const unsubscribed = h.unsubscribed;
  obsolete.onFailed({ error: "late terminal" });
  assert.equal(h.unsubscribed, unsubscribed);
  h.handlers.onCompleted({ failed_ids: ["still-failed", "another"] });
  h.editor.isTranslating = false;
  h.editor.jobSnapshot = { done: true, error: null };
  assert.ok(text(render()).includes(t("translate.retryFailed", { count: 2 })));
});

test("translation builder distinguishes omitted and explicitly empty scope and copies inputs", () => {
  const form = { providerId: "google", fallbackIds: [], sourceLang: "ja", targetLang: "en", batchSize: undefined, maxConcurrent: undefined, costLimit: "", gameContext: "" };
  assert.equal(buildTranslationStartParams(form).options.entry_ids, undefined);
  const ids: string[] = []; const files = [" a_日本語.json "];
  const params = buildTranslationStartParams({ ...form, entryIds: ids, filePaths: files });
  ids.push("new"); files.push("other");
  assert.deepEqual(params.options.entry_ids, []);
  assert.deepEqual(params.options.file_paths, [" a_日本語.json "]);
});
