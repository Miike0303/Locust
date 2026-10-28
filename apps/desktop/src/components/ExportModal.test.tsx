import assert from "node:assert/strict";
import { after, beforeEach, test } from "node:test";
import { registerHooks } from "node:module";
import { setLocale, t } from "../lib/i18n";
import * as a11y from "../lib/modalA11y";
import type { FileStats, ProjectInfo, StringStatus } from "../lib/api";

const FILE = "C:\\fixture\\scenarios\\Scene 1_日本語.rpy";
const OTHER = "scenarios/other.rpy";
const PADDED = "  scenarios/padded.rpy  ";
const STATUSES: StringStatus[] = ["pending", "translated", "reviewed", "approved", "error"];
const facets = { file_paths: [FILE, OTHER, PADDED], tags: [] };
const files: FileStats[] = [
  { file_path: FILE, total: 8, pending: 1, translated: 3, reviewed: 2, approved: 1, error: 1 },
  { file_path: OTHER, total: 5, pending: 0, translated: 4, reviewed: 1, approved: 0, error: 0 },
  { file_path: PADDED, total: 2, pending: 0, translated: 0, reviewed: 0, approved: 0, error: 2 },
];
const project: ProjectInfo = { path: "C:\\fixture", name: "Fixture", format_id: "renpy", database_path: "fixture/source.db" };
const ready = <T,>(data: T) => ({ data, isPending: false, isFetching: false, isError: false });
const h = {
  t, a11y, project: project as ProjectInfo | null, scopeAllowed: true,
  states: [] as any[], cursor: 0, effects: [] as (() => void)[], changed: false,
  facets: ready(facets), files: ready(files), queries: [] as any[],
  calls: [] as { command: string; args: any }[], requests: [] as URL[],
  toasts: [] as any[], logs: [] as any[], downloads: [] as any[], revoked: [] as string[],
  closed: 0, imported: 0, output: "assignment.po" as string | null, failure: "",
  setTauri: (_value: boolean) => {},
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
(globalThis as any).__exportModal = h;
const hooks = registerHooks({
  load(url, context, nextLoad) {
    if (url.replace(/\\/g, "/").endsWith("/src/lib/runtime.ts")) return {
      format: "module", shortCircuit: true,
      source: "export let IS_TAURI=true;export const isTauri=()=>IS_TAURI;globalThis.__exportModal.setTauri=value=>{IS_TAURI=value;};",
    };
    return nextLoad(url, context);
  },
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/components/ExportModal.tsx")) {
      const stubs: Record<string, string> = {
        react: "export const useState=initial=>h.state(initial);export const useRef=initial=>({current:initial});export const useEffect=(fn,deps)=>h.effect(fn,deps);export const useId=()=> 'export-scope';",
        "@tanstack/react-query": "export const useQuery=options=>{h.queries.push(options);return options.queryKey[0]==='file-stats'?h.files:h.facets;};",
        "../lib/projectScope": "export const draftProjectKey=p=>JSON.stringify([p.database_path??null,p.path,p.format_id]);export const useProjectScopedQueryEnabled=(_key,enabled)=>()=>enabled&&h.scopeAllowed;export const projectScopedQueryFn=(_key,fn)=>fn;",
        "../lib/i18n": "export const useT=()=>h.t;",
        "../stores/projectStore": "export const useProjectStore=()=>({project:h.project});",
        "../stores/toastStore": "export const addToast=(...args)=>h.toasts.push(args);",
        "../stores/logStore": "export const addLog=(...args)=>h.logs.push(args);",
        "../lib/modalA11y": "export const useModalA11y=()=>({dialogRef:{current:null},dialogProps:h.a11y.buildModalDialogProps({titleId:'title'}),titleProps:h.a11y.buildModalTitleProps('title')});export const MODAL_BACKDROP_CLASS=h.a11y.MODAL_BACKDROP_CLASS;export const MODAL_BODY_CLASS=h.a11y.MODAL_BODY_CLASS;export const MODAL_FOOTER_CLASS=h.a11y.MODAL_FOOTER_CLASS;export const modalPanelClass=h.a11y.modalPanelClass;",
      };
      if (stubs[specifier]) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__exportModal;" + stubs[specifier])}` };
    }
    return nextResolve(specifier, context);
  },
});
const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
const documentDescriptor = Object.getOwnPropertyDescriptor(globalThis, "document");
const originalFetch = globalThis.fetch;
const originalCreateUrl = URL.createObjectURL;
const originalRevokeUrl = URL.revokeObjectURL;
Object.defineProperty(globalThis, "window", { configurable: true, value: {
  __TAURI_INTERNALS__: { invoke: async (command: string, args: any) => {
    h.calls.push({ command, args });
    if (command === "get_server_port") return 7842;
    if (command === "get_string_facets") return facets;
    if (command === "plugin:dialog|save") return h.output;
    assert.equal(command, "export_translations");
    if (h.failure) throw h.failure;
    return { path: args.path, format: args.format, lang: args.lang, entries: 3, bytes: 20 };
  } },
} });
Object.defineProperty(globalThis, "document", { configurable: true, value: {
  documentElement: { lang: "en" },
  createElement: (tag: string) => {
    assert.equal(tag, "a");
    const anchor = { href: "", download: "", click() { h.downloads.push({ href: this.href, download: this.download }); } };
    return anchor;
  },
} });
URL.createObjectURL = () => "blob:assignment";
URL.revokeObjectURL = url => { h.revoked.push(url); };
globalThis.fetch = async input => {
  const url = new URL(String(input));
  h.requests.push(url);
  if (url.pathname === "/api/project/current") return new Response("null");
  if (url.pathname === "/api/stats/files") return new Response(JSON.stringify({ files }));
  if (url.pathname === "/api/strings/facets") return new Response(JSON.stringify(facets));
  assert.match(url.pathname, /^\/api\/export\/(po|xliff)$/);
  return h.failure ? new Response(h.failure, { status: 400 }) : new Response("assignment contents");
};
const { default: ExportModal } = await import("./ExportModal");
const api = await import("../lib/api");
// Resolve the real HTTP client's base through the native port seam before HTTP tests.
await api.getCurrentProject();
beforeEach(() => {
  h.states = []; h.effects = []; h.queries = []; h.calls = []; h.requests = [];
  h.toasts = []; h.logs = []; h.downloads = []; h.revoked = []; h.closed = 0; h.imported = 0;
  h.output = "assignment.po"; h.failure = ""; h.scopeAllowed = true; h.project = project;
  h.facets = ready(facets); h.files = ready(files); h.setTauri(true); setLocale("en");
});
after(() => {
  hooks.deregister(); globalThis.fetch = originalFetch;
  URL.createObjectURL = originalCreateUrl; URL.revokeObjectURL = originalRevokeUrl;
  if (windowDescriptor) Object.defineProperty(globalThis, "window", windowDescriptor);
  else Reflect.deleteProperty(globalThis, "window");
  if (documentDescriptor) Object.defineProperty(globalThis, "document", documentDescriptor);
  else Reflect.deleteProperty(globalThis, "document");
  Reflect.deleteProperty(globalThis, "__exportModal"); setLocale("en");
});
function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(nodes);
  return [node, ...nodes(node.props?.children)];
}
function text(node: any): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(text).join("");
  return node && typeof node === "object" ? text(node.props?.children) : "";
}
function render(open = true): any {
  let tree: any;
  for (let i = 0; i < 4; i++) {
    h.cursor = 0; h.changed = false;
    tree = ExportModal({ open, onClose() { h.closed++; }, onImported() { h.imported++; } });
    for (const effect of h.effects.splice(0)) effect();
    if (!h.changed) return tree;
  }
  throw new Error("Hook harness did not settle");
}
function select(id: string, value: string) {
  const control = nodes(render()).find(node => node.type === "select" && node.props.id === `export-scope-${id}`);
  assert.ok(control, id);
  control.props.onChange({ target: { value } });
}
function selectStatuses(selected: StringStatus[]) {
  for (const status of STATUSES) {
    const label = nodes(render()).find(node => node.type === "label" && text(node) === t(`filter.${status}`));
    const control = nodes(label).find(node => node.type === "input");
    assert.ok(control);
    control.props.onChange({ target: { checked: selected.includes(status) } });
  }
}
function primary() {
  return nodes(render()).find(node => node.type === "button" && node.props.className?.includes("bg-accent "));
}
async function clickExport() {
  const button = primary();
  assert.equal(button.props.disabled, false);
  button.props.onClick();
  await new Promise(resolve => setImmediate(resolve));
}

for (const locale of ["en", "es"] as const) {
  test(`${locale}: default scope, relative labels, exact values and associated controls`, () => {
    setLocale(locale);
    const tree = render();
    assert.ok(text(tree).includes(locale === "en" ? "15 strings will be exported" : "Se exportarán 15 cadenas"));
    assert.equal(primary().props.disabled, false);
    const controls = nodes(tree);
    const checkboxes = controls.filter(node => node.type === "input" && node.props.type === "checkbox");
    assert.equal(checkboxes.length, 5);
    assert.ok(checkboxes.every(node => node.props.checked));
    const file = controls.find(node => node.type === "select" && node.props.id === "export-scope-file");
    assert.equal(file.props.value, "");
    assert.deepEqual(nodes(file).filter(node => node.type === "option").map(node => [node.props.value, text(node)]), [
      ["", t("export.scope.allFiles")], [FILE, "scenarios/Scene 1_日本語.rpy"], [OTHER, OTHER], [PADDED, PADDED],
    ]);
    for (const control of controls.filter(node => node.type === "select")) {
      assert.ok(controls.some(node => node.type === "label" && node.props.htmlFor === control.props.id));
    }
    assert.equal(nodes(tree).find(node => node.props?.role === "dialog").props["aria-labelledby"], "title");
    assert.equal(controls.find(node => node.type === "fieldset").props["aria-describedby"], "export-scope-count");
    assert.equal(controls.find(node => node.props?.id === "export-scope-count").props.role, "status");
    assert.deepEqual(controls.filter(node => node.type === "select" || node.type === "input" && node.props.type === "checkbox")
      .map(node => node.props.id || node.props.type), ["export-scope-format", "export-scope-file", ...Array(5).fill("checkbox"), "export-scope-lang"]);
  });

  test(`${locale}: file/status changes update the caption; zero scope blocks export`, () => {
    setLocale(locale); render(); select("file", FILE); selectStatuses(["translated", "reviewed"]);
    assert.ok(text(render()).includes(t("export.scope.count", { count: 5 })));
    select("file", OTHER); selectStatuses(["pending"]);
    assert.ok(text(render()).includes(t("export.scope.count", { count: 0 })));
    assert.ok(text(render()).includes(locale === "en" ? "Nothing matches this scope" : "Nada coincide con este alcance"));
    assert.equal(primary().props.disabled, true);
    primary().props.onClick();
    assert.equal(h.calls.length, 0);
    selectStatuses([]);
    assert.equal(primary().props.disabled, true);
    selectStatuses(["error"]); select("file", PADDED);
    assert.ok(text(render()).includes(t("export.scope.count", { count: 2 })));
    assert.equal(primary().props.disabled, false);
  });
}

for (const native of [true, false]) {
  for (const format of ["po", "xliff"] as const) {
    test(`${native ? "Tauri" : "browser"} ${format}: export sends the exact file and selected statuses`, async () => {
      h.setTauri(native); render(); select("format", format); select("file", FILE); selectStatuses(["translated", "reviewed"]);
      await clickExport();
      if (native) {
        const call = h.calls.find(call => call.command === "export_translations");
        assert.deepEqual(call?.args, { format, lang: "es", path: "assignment.po", file: FILE, status: ["translated", "reviewed"] });
        assert.equal(h.downloads.length, 0);
      } else {
        const url = h.requests.find(url => url.pathname === `/api/export/${format}`)!;
        assert.equal(url.searchParams.get("file"), FILE);
        assert.deepEqual(url.searchParams.getAll("status"), ["translated", "reviewed"]);
        assert.deepEqual(h.downloads, [{ href: "blob:assignment", download: `translation_es.${format}` }]);
        assert.deepEqual(h.revoked, ["blob:assignment"]);
      }
      assert.equal(h.closed, 1);
      assert.equal(h.toasts[0][0], "success");
    });
  }

  test(`${native ? "Tauri" : "browser"}: real facets and aggregate count APIs feed the preview queries`, async () => {
    h.setTauri(native); render();
    const queries = h.queries.slice(-2);
    assert.deepEqual(await queries[0].queryFn(), facets);
    assert.deepEqual(await queries[1].queryFn(), files);
    assert.ok(h.requests.some(url => url.pathname === "/api/stats/files"));
    assert.ok(!h.requests.some(url => url.pathname === "/api/strings"));
  });

  test(`${native ? "Tauri" : "browser"}: export errors keep the dialog open and restore the action`, async () => {
    h.setTauri(native); h.failure = "no entries to export";
    await clickExport();
    assert.equal(h.closed, 0);
    assert.equal(h.toasts[0][0], "error");
    assert.equal(h.downloads.length, 0);
    assert.equal(primary().props.disabled, false);
  });
}

test("omitted and explicitly empty status lists keep distinct HTTP and Tauri payloads", async () => {
  h.setTauri(false);
  await api.exportPo("es"); await api.exportXliff("es");
  assert.ok(h.requests.every(url => url.search === "?lang=es"));
  await api.exportPo("es", { status: [] });
  assert.equal(h.requests[h.requests.length - 1]?.search, "?lang=es&status=");
  h.setTauri(true);
  await api.exportTranslations("po", "es", "assignment.po");
  assert.deepEqual(h.calls[h.calls.length - 1]?.args, { format: "po", lang: "es", path: "assignment.po" });
  await api.exportTranslations("xliff", "es", "assignment.xliff", { status: [] });
  assert.deepEqual(h.calls[h.calls.length - 1]?.args.status, []);
});

test("pending, refreshing and failed scope data disable export without claiming an empty selection", () => {
  for (const query of ["facets", "files"] as const) {
    h[query].isPending = true;
    assert.equal(primary().props.disabled, true);
    assert.ok(text(render()).includes(t("export.scope.loading")));
    assert.ok(!text(render()).includes(t("export.scope.empty")));
    h[query].isPending = false; h[query].isError = true;
    assert.equal(primary().props.disabled, true);
    assert.ok(text(render()).includes(t("export.scope.loadFailed")));
    h[query].isError = false;
  }
  h.files.isFetching = true;
  assert.equal(primary().props.disabled, true);
});

test("closing, changing database and importing gate the preview and reset the scope", () => {
  render(); select("file", FILE); selectStatuses(["translated"]);
  assert.equal(render(false), null);
  assert.ok(h.queries.slice(-2).every(query => !query.enabled()));
  const reopened = render();
  assert.equal(nodes(reopened).find(node => node.props?.id === "export-scope-file").props.value, "");
  assert.ok(nodes(reopened).filter(node => node.type === "input" && node.props.type === "checkbox").every(node => node.props.checked));
  const oldKey = h.queries[h.queries.length - 1].queryKey;
  select("file", FILE); h.project = { ...project, database_path: "fixture/other.db" };
  assert.equal(nodes(render()).find(node => node.props?.id === "export-scope-file").props.value, "");
  assert.notDeepEqual(h.queries[h.queries.length - 1].queryKey, oldKey);
  const importTab = nodes(render()).find(node => node.type === "button" && text(node) === t("export.import"));
  importTab.props.onClick(); h.files.isPending = true;
  const importing = render();
  assert.ok(!nodes(importing).some(node => node.type === "fieldset"));
  assert.equal(primary().props.disabled, false);
  assert.ok(h.queries.slice(-2).every(query => !query.enabled()));
  h.project = null;
  assert.equal(render(), null);
});

test("native save cancellation writes nothing and keeps the dialog available", async () => {
  h.output = null;
  await clickExport();
  assert.ok(!h.calls.some(call => call.command === "export_translations"));
  assert.equal(h.closed, 0);
  assert.equal(primary().props.disabled, false);
});

test("an unconfirmed project cannot start an export", () => {
  h.scopeAllowed = false;
  assert.equal(primary().props.disabled, true);
  primary().props.onClick();
  assert.equal(h.calls.length, 0);
});
