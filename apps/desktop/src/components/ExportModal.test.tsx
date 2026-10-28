import assert from "node:assert/strict";
import { after, beforeEach, test } from "node:test";
import { registerHooks } from "node:module";
import { setLocale, t } from "../lib/i18n";
import * as a11y from "../lib/modalA11y";
import type { FileStats, ProjectInfo, StringStatus, ImportPreview } from "../lib/api";

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
const importPreview = (keep: boolean): ImportPreview => ({
  fill: 1, replace: keep ? 0 : 1, unchanged: 2, unknown_ids: 3, kept_existing: keep ? 1 : 0,
  stale_sources: 4, imported: keep ? 1 : 2, skipped: keep ? 10 : 9,
  replacements: keep ? [] : [{ id: "reviewed", file: FILE, previous_text: "Old reviewed text", previous_status: "reviewed", new_text: "Reviewer replacement" }],
  truncated: !keep, plan_token: keep ? "keep-plan" : "replace-plan",
});
const h = {
  t, a11y, project: project as ProjectInfo | null, scopeAllowed: true,
  states: [] as any[], cursor: 0, effects: [] as (() => void)[], changed: false,
  facets: ready(facets), files: ready(files), source: ready({ source_lang: "ja" }), queries: [] as any[],
  calls: [] as { command: string; args: any }[], requests: [] as URL[],
  bodies: [] as { path: string; body: any }[],
  previewHandler: null as null | ((keep: boolean) => Promise<ImportPreview>),
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
        react: "export const useState=initial=>h.state(initial);export const useRef=initial=>h.state(()=>({current:initial}))[0];export const useEffect=(fn,deps)=>h.effect(fn,deps);export const useId=()=> 'export-scope';",
        "@tanstack/react-query": "export const useQuery=options=>{h.queries.push(options);return options.queryKey[0]==='export-source-language'?h.source:options.queryKey[0]==='file-stats'?h.files:h.facets;};",
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
    if (command === "plugin:dialog|open") return h.output;
    if (command === "preview_import_translations") {
      if (h.failure) throw h.failure;
      return h.previewHandler ? h.previewHandler(args.keepExisting) : importPreview(args.keepExisting);
    }
    if (command === "apply_import_preview") {
      if (h.failure) throw h.failure;
      return { path: args.path, format: args.format, imported: 1, skipped: 10, stale_sources: 4 };
    }
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
globalThis.fetch = async (input, init) => {
  const url = new URL(String(input));
  h.requests.push(url);
  if (url.pathname === "/api/project/current") return new Response("null");
  if (url.pathname === "/api/stats/files") return new Response(JSON.stringify({ files }));
  if (url.pathname === "/api/strings/facets") return new Response(JSON.stringify(facets));
  if (url.pathname === "/api/export/source-language") return new Response(JSON.stringify(h.source.data));
  if (/^\/api\/import\/(po|xliff|tmx)\/(preview|apply)$/.test(url.pathname)) {
    const body = JSON.parse(String(init?.body));
    h.bodies.push({ path: url.pathname, body });
    if (h.failure) return new Response(h.failure, { status: h.failure.includes("stale") ? 409 : 400 });
    return new Response(JSON.stringify(url.pathname.endsWith("/preview")
      ? h.previewHandler ? await h.previewHandler(body.keep_existing) : importPreview(body.keep_existing)
      : { imported: 1, skipped: 10, stale_sources: 4 }));
  }
  assert.match(url.pathname, /^\/api\/export\/(po|xliff|tmx)$/);
  return h.failure ? new Response(h.failure, { status: 400 }) : new Response("assignment contents");
};
const { default: ExportModal } = await import("./ExportModal");
const api = await import("../lib/api");
// Resolve the real HTTP client's base through the native port seam before HTTP tests.
await api.getCurrentProject();
beforeEach(() => {
  h.states = []; h.effects = []; h.queries = []; h.calls = []; h.requests = [];
  h.bodies = []; h.previewHandler = null;
  h.toasts = []; h.logs = []; h.downloads = []; h.revoked = []; h.closed = 0; h.imported = 0;
  h.output = "assignment.po"; h.failure = ""; h.scopeAllowed = true; h.project = project;
  h.facets = ready(facets); h.files = ready(files); h.source = ready({ source_lang: "ja" }); h.setTauri(true); setLocale("en");
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
  for (const format of ["po", "xliff", "tmx"] as const) {
    test(`${native ? "Tauri" : "browser"} ${format}: export sends the exact file and selected statuses`, async () => {
      h.setTauri(native); render(); select("format", format); select("file", FILE); selectStatuses(["translated", "reviewed"]);
      await clickExport();
      if (native) {
        const call = h.calls.find(call => call.command === "export_translations");
        assert.deepEqual(call?.args, { format, lang: "es", path: "assignment.po", file: FILE, status: ["translated", "reviewed"], ...(format === "tmx" ? { sourceLang: "ja" } : {}) });
        const dialog = h.calls.find(call => call.command === "plugin:dialog|save");
        assert.equal(dialog?.args.options.defaultPath, `translation_es.${format}`);
        if (format === "tmx") assert.deepEqual(dialog?.args.options.filters, [{ name: "TMX", extensions: ["tmx"] }]);
        assert.equal(h.downloads.length, 0);
      } else {
        const url = h.requests.find(url => url.pathname === `/api/export/${format}`)!;
        assert.equal(url.searchParams.get("file"), FILE);
        assert.deepEqual(url.searchParams.getAll("status"), ["translated", "reviewed"]);
        assert.equal(url.searchParams.get("lang"), "es");
        assert.equal(url.searchParams.get("source_lang"), format === "tmx" ? "ja" : null);
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

for (const locale of ["en", "es"] as const) {
  test(`${locale}: TMX alone shows associated source and target selects and requires distinct languages`, async () => {
    setLocale(locale);
    for (const format of ["po", "xliff"]) {
      select("format", format);
      assert.ok(!nodes(render()).some(node => node.props?.id === "export-scope-source-lang"));
      assert.equal(primary().props.disabled, false);
    }
    select("format", "tmx");
    const tree = render();
    const source = nodes(tree).find(node => node.props?.id === "export-scope-source-lang");
    assert.equal(source.props.value, "ja");
    assert.ok(nodes(tree).some(node => node.type === "label" && node.props.htmlFor === source.props.id
      && text(node) === (locale === "en" ? "Source language" : "Idioma de origen")));
    assert.equal(nodes(tree).find(node => node.props?.id === "export-scope-lang").props.value, "es");
    select("source-lang", "es");
    assert.equal(primary().props.disabled, true);
    assert.ok(text(render()).includes(t("export.distinctLanguages")));
    select("source-lang", "en-US");
    await clickExport();
    assert.equal(h.calls.find(call => call.command === "export_translations")?.args.sourceLang, "en-US");
    await click("export.import");
    assert.ok(!nodes(render()).some(node => node.props?.id === "export-scope-source-lang"));
  });
}

for (const native of [true, false]) {
  test(`${native ? "Tauri" : "browser"}: TMX uses the project language query and reports codec refusal without a download`, async () => {
    h.setTauri(native); render();
    let query = [...h.queries].reverse().find(query => query.queryKey[0] === "export-source-language");
    assert.equal(query.enabled(), false);
    select("format", "tmx"); render();
    query = [...h.queries].reverse().find(query => query.queryKey[0] === "export-source-language");
    assert.equal(query.enabled(), true);
    assert.deepEqual(await query.queryFn(), { source_lang: "ja" });
    assert.equal([...h.requests].reverse().find(url => url.pathname === "/api/export/source-language")?.search, "?lang=es");
    h.failure = "TMX profile error: document exceeds 32 MiB";
    await clickExport();
    assert.equal(h.closed, 0);
    assert.equal(h.downloads.length, 0);
    assert.equal(h.toasts[0][0], "error");
    assert.equal(primary().props.disabled, false);
  });

  test(`${native ? "Tauri" : "browser"}: API requires an explicit TMX source before issuing requests`, async () => {
    h.setTauri(native);
    await assert.rejects(api.exportTranslations("tmx", "es", "catalog.tmx"), /source_lang/);
    assert.equal(h.requests.length, 0);
    assert.equal(h.calls.length, 0);
    assert.equal(h.downloads.length, 0);
  });
}

test("TMX source defaults arrive asynchronously, preserve manual choices and reset with the project or dialog", () => {
  h.source = { ...ready({ source_lang: "" }), isPending: true };
  select("format", "tmx");
  assert.equal(primary().props.disabled, true);
  select("source-lang", "en");
  h.source = ready({ source_lang: "ja" });
  assert.equal(nodes(render()).find(node => node.props?.id === "export-scope-source-lang").props.value, "en");
  h.project = { ...project, database_path: "fixture/new.db" };
  h.source = ready({ source_lang: "pt-PT" });
  let source = nodes(render()).find(node => node.props?.id === "export-scope-source-lang");
  assert.equal(source.props.value, "pt-PT");
  assert.ok(nodes(source).some(node => node.type === "option" && node.props.value === "pt-PT"));
  select("source-lang", "de"); render(false); render();
  source = nodes(render()).find(node => node.props?.id === "export-scope-source-lang");
  assert.equal(source.props.value, "pt-PT");
  for (const source_lang of ["auto", "und", "", "bad_tag"]) {
    h.source = ready({ source_lang });
    assert.equal(primary().props.disabled, true);
    assert.equal(nodes(render()).find(node => node.props?.id === "export-scope-source-lang").props.value, "");
  }
  h.source.isError = true;
  select("source-lang", "ja");
  assert.equal(primary().props.disabled, false);
  select("format", "po");
  assert.equal(primary().props.disabled, false);
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
  assert.ok(!nodes(importing).some(node => node.type === "fieldset" && text(node).includes(t("export.scope.title"))));
  assert.equal(primary().props.disabled, true);
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

const flush = () => new Promise(resolve => setImmediate(resolve));
function button(label: string) {
  const control = nodes(render()).find(node => node.type === "button" && text(node) === t(label as Parameters<typeof t>[0]));
  assert.ok(control, label);
  return control;
}
async function click(label: string) {
  const control = button(label);
  assert.equal(control.props.disabled ?? false, false, label);
  control.props.onClick();
  await flush();
}
function radio(keep: boolean) {
  const label = nodes(render()).find(node => node.type === "label" && text(node) === t(keep ? "export.preview.keep" : "export.preview.replace"));
  assert.ok(label);
  return nodes(label).find(node => node.type === "input");
}
async function chooseImport(native: boolean, format = "po") {
  h.setTauri(native);
  await click("export.import");
  select("format", format);
  render();
  if (native) await click("export.chooseFile");
  else {
    const input = nodes(render()).find(node => node.type === "input" && node.props.type === "file");
    input.props.onChange({ target: { files: [{ name: `assignment.${format}`, arrayBuffer: async () => new TextEncoder().encode("reviewer catalog").buffer }], value: "chosen" } });
    await flush();
  }
}

for (const locale of ["en", "es"] as const) {
  for (const native of [true, false]) {
    for (const format of ["po", "xliff", "tmx"]) {
      test(`${locale} ${native ? "Tauri" : "browser"} ${format}: select, protected preview, replacement preview and token apply`, async () => {
        setLocale(locale);
        await chooseImport(native, format);
        assert.equal(radio(true).props.checked, true);
        assert.equal(button("export.preview.apply").props.disabled, true);
        assert.equal(h.calls.filter(call => call.command.includes("import")).length, 0);
        assert.equal(h.bodies.length, 0);
        await click("export.preview.action");
        assert.equal(h.imported, 0);
        assert.equal(h.closed, 0);
        assert.ok(text(render()).includes(t("export.preview.kept", { count: 1 })));
        assert.ok(text(render()).includes(t("export.preview.staleSources", { count: 4 })));
        const live = nodes(render()).find(node => node.props?.["aria-live"] === "polite");
        assert.equal(live.props["aria-atomic"], "true");
        assert.ok(text(live).includes(t("export.preview.summary", { fill: 1, replace: 0, unchanged: 2, unknown: 3 })));
        assert.equal(button("export.preview.apply").props.disabled, false);
        radio(false).props.onChange();
        assert.equal(button("export.preview.apply").props.disabled, true);
        await flush();
        const tree = render();
        assert.ok(text(tree).includes("Old reviewed text"));
        assert.ok(text(tree).includes("Reviewer replacement"));
        assert.ok(text(tree).includes("scenarios/Scene 1_日本語.rpy"));
        assert.ok(text(tree).includes(t("export.preview.truncated", { count: 1 })));
        assert.ok(!text(tree).includes(t("export.preview.kept", { count: 1 })));
        assert.ok(nodes(tree).some(node => node.props?.status === "reviewed"));
        assert.ok(nodes(tree).some(node => node.type === "thead" && node.props.className.includes("sticky")));
        assert.ok(nodes(tree).some(node => node.props?.role === "region" && node.props.tabIndex === 0 && node.props.className.includes("max-h-64")));
        await click("export.preview.apply");
        assert.equal(h.imported, 1);
        assert.equal(h.closed, 1);
        if (native) {
          assert.deepEqual(h.calls.filter(call => call.command.includes("import")).map(call => [call.command, call.args]), [
            ["preview_import_translations", { format, path: "assignment.po", keepExisting: true }],
            ["preview_import_translations", { format, path: "assignment.po", keepExisting: false }],
            ["apply_import_preview", { format, path: "assignment.po", keepExisting: false, planToken: "replace-plan" }],
          ]);
        } else {
          assert.deepEqual(h.bodies, [
            { path: `/api/import/${format}/preview`, body: { content: "reviewer catalog", keep_existing: true } },
            { path: `/api/import/${format}/preview`, body: { content: "reviewer catalog", keep_existing: false } },
            { path: `/api/import/${format}/apply`, body: { content: "reviewer catalog", keep_existing: false, plan_token: "replace-plan" } },
          ]);
        }
      });
    }
  }
}

for (const native of [true, false]) {
  test(`${native ? "Tauri" : "browser"}: stale plan stays inline, disables apply and can be previewed again`, async () => {
    setLocale("es"); await chooseImport(native); await click("export.preview.action");
    h.failure = "import preview is stale; preview again";
    await click("export.preview.apply");
    assert.equal(h.imported, 0); assert.equal(h.closed, 0);
    assert.equal(button("export.preview.apply").props.disabled, true);
    assert.ok(nodes(render()).some(node => node.props?.role === "alert" && text(node).includes(t("export.preview.stale"))));
    h.failure = "";
    await click("export.preview.again");
    assert.equal(button("export.preview.apply").props.disabled, false);
    assert.ok(!nodes(render()).some(node => node.props?.role === "alert"));
  });

  test(`${native ? "Tauri" : "browser"}: preview failure keeps file selected and permits retry`, async () => {
    await chooseImport(native); h.failure = "invalid catalog";
    await click("export.preview.action");
    assert.ok(nodes(render()).some(node => node.props?.role === "alert" && text(node).includes("invalid catalog")));
    assert.equal(button("export.preview.apply").props.disabled, true);
    h.failure = ""; await click("export.preview.action");
    assert.equal(button("export.preview.apply").props.disabled, false);
  });
}

test("out-of-order radio previews cannot restore an obsolete token", async () => {
  await chooseImport(true);
  const pending: ((value: ImportPreview) => void)[] = [];
  h.previewHandler = () => new Promise(resolve => pending.push(resolve));
  button("export.preview.action").props.onClick();
  await flush();
  radio(false).props.onChange();
  await flush();
  assert.equal(pending.length, 2);
  pending[1](importPreview(false)); await flush();
  assert.equal(button("export.preview.apply").props.disabled, false);
  pending[0](importPreview(true)); await flush();
  await click("export.preview.apply");
  assert.equal(h.calls.find(call => call.command === "apply_import_preview")?.args.planToken, "replace-plan");
});

test("an obsolete apply handler cannot use a token after synchronous invalidation", async () => {
  await chooseImport(true); await click("export.preview.action");
  const oldApply = button("export.preview.apply").props.onClick;
  radio(false).props.onChange();
  oldApply();
  await flush();
  assert.ok(!h.calls.some(call => call.command === "apply_import_preview"));
  assert.equal(button("export.preview.apply").props.disabled, false);
});

test("apply consumes the preview before another click can submit it", async () => {
  await chooseImport(true); await click("export.preview.action");
  const apply = button("export.preview.apply").props.onClick;
  apply(); apply(); await flush();
  assert.equal(h.calls.filter(call => call.command === "apply_import_preview").length, 1);
  assert.equal(h.imported, 1);
});

test("format, database, close and mode changes discard in-flight previews", async () => {
  for (const change of ["format", "database", "close", "mode"] as const) {
    h.states = []; h.effects = []; h.project = project;
    await chooseImport(true);
    let finish!: (value: ImportPreview) => void;
    h.previewHandler = () => new Promise(resolve => { finish = resolve; });
    button("export.preview.action").props.onClick(); await flush();
    if (change === "format") select("format", "xliff");
    if (change === "database") { h.project = { ...project, database_path: "fixture/other.db" }; render(); }
    if (change === "close") render(false);
    if (change === "mode") await click("export.export");
    finish(importPreview(true)); await flush();
    if (change === "mode") await click("export.import");
    else render();
    assert.equal(button("export.preview.apply").props.disabled, true, change);
    assert.ok(!text(render()).includes(t("export.preview.kept", { count: 1 })), change);
  }
});

test("selecting another catalog invalidates the preview before reading finishes", async () => {
  await chooseImport(false); await click("export.preview.action");
  let finish!: (bytes: ArrayBuffer) => void;
  const input = nodes(render()).find(node => node.type === "input" && node.props.type === "file");
  input.props.onChange({ target: { files: [{ name: "changed.po", arrayBuffer: () => new Promise<ArrayBuffer>(resolve => { finish = resolve; }) }], value: "chosen" } });
  assert.equal(button("export.preview.apply").props.disabled, true);
  assert.equal(button("export.preview.action").props.disabled, true);
  finish(new TextEncoder().encode("changed catalog").buffer); await flush();
  assert.equal(button("export.preview.apply").props.disabled, true);
  assert.equal(button("export.preview.action").props.disabled, false);
});

test("native import file cancellation never previews or applies", async () => {
  h.output = null; await chooseImport(true);
  assert.equal(button("export.preview.action").props.disabled, true);
  assert.ok(!h.calls.some(call => call.command === "preview_import_translations" || call.command === "apply_import_preview"));
});

test("browser import preserves the exact UTF-8 BOM and non-ASCII catalog content", async () => {
  await chooseImport(false);
  const content = "\uFEFFmsgctxt \"日本語\"\r\nmsgid \"Hello\"\r\nmsgstr \"¡Hola!\"\r\n";
  const input = nodes(render()).find(node => node.type === "input" && node.props.type === "file");
  input.props.onChange({ target: { files: [{ name: "bom.po", arrayBuffer: async () => new TextEncoder().encode(content).buffer }], value: "chosen" } });
  await flush(); await click("export.preview.action"); await click("export.preview.apply");
  assert.equal(h.bodies[0].body.content, content);
  assert.equal(h.bodies[1].body.content, content);
  assert.equal(h.bodies[1].body.plan_token, "keep-plan");
});

for (const locale of ["en", "es"] as const) {
  test(`${locale}: malformed UTF-8 clears the browser preview and shows a readable error without submitting`, async () => {
    setLocale(locale); await chooseImport(false); await click("export.preview.action");
    const input = nodes(render()).find(node => node.type === "input" && node.props.type === "file");
    input.props.onChange({ target: { files: [{ name: "invalid.po", arrayBuffer: async () => new Uint8Array([0xc3, 0x28]).buffer }], value: "chosen" } });
    await flush();
    assert.equal(button("export.preview.apply").props.disabled, true);
    assert.equal(button("export.preview.action").props.disabled, true);
    assert.ok(nodes(render()).some(node => node.props?.role === "alert" && text(node).includes(t("export.preview.readFailed"))));
    assert.equal(h.bodies.length, 1);
    assert.equal(h.imported, 0);
  });
}
