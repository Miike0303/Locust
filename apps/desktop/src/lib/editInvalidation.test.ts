import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, afterEach, test } from "node:test";
import { setImmediate } from "node:timers/promises";
import { hashKey, QueryObserver, type QueryKey } from "@tanstack/react-query";
import { cancelProjectData, queryClient as client } from "./queryClient";
import { projectScopedQueryFn, isProjectScopedQueryEnabled } from "./projectScope";
import { qaLastRunKey } from "./qaDashboard";
import { workflowLastEditKey } from "./translationWrites";
import { setLocale, t } from "./i18n";
import * as drafts from "../stores/draftStore";
import { useProjectStore } from "../stores/projectStore";
import { useEditorStore } from "../stores/editorStore";
import type { ProjectStats, StringEntry, StringFilter, StringsResponse } from "./api";

const project = { path: "C:/edit-fixture", database_path: "C:/edit-fixture/es.db", format_id: "html-game", name: "Edits" };
const scope = drafts.draftProjectKey(project);
const otherScope = drafts.draftProjectKey({ ...project, database_path: "C:/edit-fixture/fr.db" });
const baseRow: StringEntry = {
  id: "row-0", source: "Source", translation: "Before", status: "translated", file_path: "story.html",
  tags: [], context: null, metadata: {}, provider_used: null, char_limit: null,
  created_at: "2026-01-01", translated_at: null, reviewed_at: null,
};
type Tree = { type?: any; props?: any };
function nodes(node: any): Tree[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(nodes);
  return [node, ...nodes(node.props?.children)];
}
const h = {
  t, drafts, useProjectStore, useEditorStore, client, projectScopedQueryFn, isProjectScopedQueryEnabled,
  states: [] as any[], cursor: 0, columns: [] as any[], rows: [] as StringEntry[],
  history: new Map<string, unknown[]>(), observers: new Map<string, () => void>(),
  requests: [] as QueryKey[], inFlight: new Map<string, number>(), overlaps: 0,
  readGate: null as Promise<void> | null, writeGate: null as Promise<void> | null,
  returnedFields: {} as Partial<StringEntry>,
  useState(initial: any) {
    const states = h.states, i = h.cursor++;
    if (!(i in states)) states[i] = typeof initial === "function" ? initial() : initial;
    return [states[i], (value: any) => { states[i] = typeof value === "function" ? value(states[i]) : value; }];
  },
  useRef(initial: any) { const i = h.cursor++; return h.states[i] ??= { current: initial }; },
  useQuery(options: any) {
    const hash = hashKey(options.queryKey);
    if (!h.observers.has(hash)) {
      const observer = new QueryObserver(client, { ...options, staleTime: Infinity, gcTime: Infinity,
        refetchOnMount: false, queryFn: (context: any) => h.read(options.queryKey, () => options.queryFn(context)) });
      h.observers.set(hash, observer.subscribe(() => {}));
    }
    return { data: client.getQueryData(options.queryKey), isLoading: false, isError: false };
  },
  async read(key: QueryKey, fn: () => Promise<unknown>) {
    const hash = hashKey(key);
    h.requests.push(key);
    if (h.inFlight.get(hash)) h.overlaps++;
    h.inFlight.set(hash, (h.inFlight.get(hash) ?? 0) + 1);
    // Capture the server snapshot before waiting, so joining an older read is
    // insufficient to make a subsequent write visible.
    const result = fn();
    try { if (h.readGate) await h.readGate; return await result; }
    finally { h.inFlight.set(hash, h.inFlight.get(hash)! - 1); }
  },
};
(globalThis as any).__editInvalidation = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/lib/api.ts") && specifier === "./runtime") {
      return { shortCircuit: true, url: "data:text/javascript,export const IS_TAURI=true;" };
    }
    if (/\/(pages\/Editor|components\/(DetailPanel|StringTable))\.tsx$/.test(context.parentURL ?? "")) {
      const stubs: Record<string, string> = {
        react: "export const useState=h.useState;export const useRef=h.useRef;export const useEffect=()=>{};export const useMemo=fn=>fn();export const useCallback=fn=>fn;",
        "react-router-dom": "export const useNavigate=()=>()=>{};",
        "@tanstack/react-query": "export const useQuery=h.useQuery;export const useQueryClient=()=>h.client;export const useIsMutating=()=>0;",
        "@tanstack/react-table": "export const useReactTable=options=>{h.columns=options.columns;return {getHeaderGroups:()=>[],getRowModel:()=>({rows:[]})};};export const getCoreRowModel=()=>{};export const getSortedRowModel=()=>{};export const flexRender=()=>null;",
        "../lib/i18n": "export const useT=()=>h.t;",
        "../lib/hotkeys": "export const useHotkey=()=>{};",
        "../lib/modalA11y": "export const isTopmostOverlayDialog=()=>false;export const useModalA11y=()=>({dialogRef:{current:null},dialogProps:{}});",
        "../lib/projectScope": "export const projectScopedQueryFn=h.projectScopedQueryFn;export const useProjectScopedQueryEnabled=(key,enabled=true)=>()=>enabled&&h.isProjectScopedQueryEnabled(key);",
        "../stores/projectStore": "export const useProjectStore=Object.assign(selector=>selector?selector(h.useProjectStore.getState()):h.useProjectStore.getState(),{getState:h.useProjectStore.getState});",
        "../stores/editorStore": "export const useEditorStore=selector=>selector?selector(h.useEditorStore.getState()):h.useEditorStore.getState();",
        "../stores/draftStore": `export const useDraftStore=Object.assign(selector=>selector(h.drafts.useDraftStore.getState()),{getState:h.drafts.useDraftStore.getState});${Object.keys(drafts).filter(k => k !== "useDraftStore").map(k => `export const ${k}=h.drafts.${k};`).join("")}`,
      };
      if (stubs[specifier]) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__editInvalidation;" + stubs[specifier])}` };
    }
    return nextResolve(specifier, context);
  },
});
const originalFetch = globalThis.fetch;
const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
function stats(): ProjectStats {
  const counts: ProjectStats = { total: h.rows.length, pending: 0, translated: 0, reviewed: 0, approved: 0, error: 0, total_cost_usd: 0 };
  for (const row of h.rows) counts[row.status]++;
  return counts;
}
function page(filter: StringFilter): StringsResponse {
  const entries = h.rows.filter(row => (!filter.status || row.status === filter.status)
    && (!filter.file_path || row.file_path === filter.file_path) && (!filter.tag || row.tags.includes(filter.tag))
    && (!filter.search || `${row.source} ${row.translation ?? ""}`.toLowerCase().includes(filter.search.toLowerCase())));
  const offset = filter.offset ?? 0, limit = filter.limit ?? 100;
  return structuredClone({ entries: entries.slice(offset, offset + limit), total: entries.length, offset, limit });
}
Object.defineProperty(globalThis, "window", { configurable: true, value: {
  __TAURI_INTERNALS__: { invoke: async (command: string, args: any) => {
    switch (command) {
      case "get_server_port": return 7842;
      case "get_strings": return page(args.filter);
      case "get_stats": return stats();
      case "get_string_facets": return { file_paths: [...new Set(h.rows.map(row => row.file_path))].sort(), tags: [...new Set(h.rows.flatMap(row => row.tags))].sort() };
      case "patch_string": {
        if (h.writeGate) await h.writeGate;
        const index = h.rows.findIndex(row => row.id === args.id), old = h.rows[index];
        if ("expected_translation" in args.data && args.data.expected_translation !== old.translation) throw "translation changed since it was loaded";
        const { expected_translation: _guard, ...patch } = args.data;
        const saved = { ...old, ...patch, ...(patch.translation !== undefined ? { status: "translated", provider_used: "manual", translated_at: "2026-10-28" } : {}), ...h.returnedFields };
        h.rows[index] = saved;
        h.history.set(old.id, [...(h.history.get(old.id) ?? []), { new_translation: saved.translation, new_status: saved.status }]);
        return structuredClone(saved);
      }
      default: throw new Error(`Unexpected IPC: ${command}`);
    }
  } },
} });
globalThis.fetch = async url => {
  const path = new URL(String(url)).pathname;
  const row = path.match(/\/strings\/([^/]+)(\/history)?$/);
  const data = row ? row[2] ? h.history.get(row[1]) ?? [] : h.rows.find(entry => entry.id === row[1]) : {};
  return new Response(JSON.stringify(data));
};
const api = await import("./api");
const { default: Editor } = await import("../pages/Editor");
const { default: DetailPanel } = await import("../components/DetailPanel");
const { default: StringTable } = await import("../components/StringTable");

afterEach(() => {
  for (const unsubscribe of h.observers.values()) unsubscribe();
  h.observers.clear(); client.clear();
  useProjectStore.setState({ project: null, confirmedOpenProjectKey: null, projectTransitionInProgress: false });
});
after(() => {
  hooks.deregister(); globalThis.fetch = originalFetch; setLocale("en");
  if (windowDescriptor) Object.defineProperty(globalThis, "window", windowDescriptor);
  else Reflect.deleteProperty(globalThis, "window");
  delete (globalThis as any).__editInvalidation;
});
function reset(filter: StringFilter = { limit: 2, offset: 0 }, initial: Partial<StringEntry> = {}) {
  client.setDefaultOptions({ queries: { retry: false, gcTime: Infinity, staleTime: Infinity } });
  h.states = []; h.cursor = 0; h.requests = []; h.overlaps = 0; h.inFlight.clear();
  h.readGate = null; h.writeGate = null;
  h.returnedFields = {};
  h.rows = Array.from({ length: 6 }, (_, i) => ({ ...baseRow, ...initial, id: `row-${i}`, source: `Source ${i}` }));
  h.history.clear();
  drafts.useDraftStore.setState({ drafts: {}, records: [], persistenceIssue: null, persistenceIssues: {} });
  useProjectStore.setState({ project, confirmedOpenProjectKey: scope, projectTransitionInProgress: false });
  useEditorStore.setState({ filter, selectedEntryId: "row-0", isTranslating: false });
  setLocale("en");
  client.setQueryData(["config"], { ui: {} });
  client.setQueryData(["strings", filter, scope], page(filter));
  client.setQueryData(["stats", scope], stats());
  client.setQueryData(["string", "row-0", scope], structuredClone(h.rows[0]));
  client.setQueryData(["validation-summary", scope], {});
  client.setQueryData(qaLastRunKey(scope), 100);
  for (const row of h.rows) client.setQueryData(["string-history", scope, row.id], []);
  for (const family of ["file-stats", "current-project", "translation-runs", "injection-status", "validation-issues", "memory-suggestions"]) {
    client.setQueryData([family, scope], "cached");
    client.setQueryData([family, otherScope], "other project");
  }
  // Current editor observers plus expanded history and the filter's facets.
  const tree = Editor();
  const detail = nodes(tree).find(node => node.type === DetailPanel)!;
  assert.ok(detail, "drive Editor's authored refresh callback");
  observe(["string-history", scope, "row-0"], () => api.getStringHistory("row-0"));
  client.setQueryData(["string-facets", project.path], { file_paths: ["story.html"], tags: [] });
  observe(["string-facets", project.path], api.getStringFacets);
  return detail.props.onRefetch as (...args: any[]) => Promise<void>;
}
function observe(key: QueryKey, queryFn: () => Promise<unknown>) {
  h.useQuery({ queryKey: key, queryFn: projectScopedQueryFn(scope, queryFn) });
}
function detailTree(refresh: (...args: any[]) => unknown, row = h.rows[0]) {
  h.cursor = 0; h.states = [];
  return DetailPanel({ projectKey: scope, entry: structuredClone(row), onRefetch: refresh, onClose() {} });
}
async function save(refresh: (...args: any[]) => unknown, id = "row-0", text = "After") {
  const tree = detailTree(refresh, h.rows.find(row => row.id === id)!);
  const textarea = nodes(tree).find(node => node.type === "textarea")!;
  textarea.props.onFocus(); textarea.props.onChange({ target: { value: text } });
  await textarea.props.onBlur();
}
function statusButton(tree: Tree, status: string): Tree {
  return nodes(tree).find(node => node.props?.["aria-pressed"] !== undefined
    && nodes(node.props.children).some(child => child.props?.status === status))!;
}
async function settle() {
  for (let turn = 0; turn < 100; turn++) {
    await setImmediate();
    if (!client.isFetching()) { await setImmediate(); if (!client.isFetching()) return; }
  }
  assert.fail("edit refetches did not settle");
}
function counts() {
  const families: Record<string, number> = {};
  for (const key of h.requests) families[String(key[0])] = (families[String(key[0])] ?? 0) + 1;
  return { families, total: h.requests.length, duplicates: h.requests.length - new Set(h.requests.map(hashKey)).size, overlaps: h.overlaps };
}
function checkState() {
  assert.deepEqual(client.getQueryData(["stats", scope]), stats(), "server counts stay current");
  const filter = useEditorStore.getState().filter;
  assert.deepEqual(client.getQueryData(["strings", filter, scope]), page(filter), "page membership, pagination and totals stay correct");
  assert.deepEqual(client.getQueryData(["string", "row-0", scope]), h.rows[0], "detail uses the saved server row");
  assert.deepEqual(client.getQueryData(["string-history", scope, "row-0"]), h.history.get("row-0"), "expanded history stays current");
  for (const query of client.getQueryCache().findAll({ predicate: q => q.queryKey[1] === otherScope })) {
    assert.equal(query.state.isInvalidated, false, "other database is untouched");
  }
}

const baseline = { translation: 6, status: 6, "filtered removal": 6, "five rapid saves": 25 };
const measured: number[] = [];
// These budgets deliberately fail against the original broad refresh. The
// before/after measurements are recorded in docs/EDITOR-PERFORMANCE.md.
for (const scenario of ["translation", "status", "filtered removal", "five rapid saves"] as const) {
  test(`edit requests: ${scenario}`, async () => {
    const refresh = reset(scenario === "filtered removal" ? { search: "Before", limit: 2, offset: 0 } : { limit: 2, offset: 0 });
    if (scenario === "status") {
      await statusButton(detailTree(refresh), "approved").props.onClick();
    } else if (scenario === "five rapid saves") {
      let release!: () => void;
      h.readGate = new Promise<void>(resolve => { release = resolve; });
      try {
        // Five inline edits while row-0's detail/history remain open. Complete
        // writes sequentially, with earlier reads still pending on the network.
        for (const row of h.rows.slice(0, 5)) {
          h.cursor = 0; h.states = [];
          StringTable({ data: h.rows, onRefetch: refresh });
          const cell = h.columns.find(column => column.accessorKey === "translation").cell({ row: { original: structuredClone(row) } });
          h.cursor = 0; h.states = [];
          cell.type(cell.props);
          drafts.editDraft(drafts.draftEntryKey(scope, row.id), `After ${row.id}`, row.translation);
          // Enter editing through the actual button, then blur its textarea.
          h.cursor = 0;
          nodes(cell.type(cell.props)).find(node => node.props?.["aria-label"] === t("table.editTranslation"))!.props.onClick({ stopPropagation() {} });
          h.cursor = 0;
          await nodes(cell.type(cell.props)).find(node => node.type === "textarea")!.props.onBlur();
        }
      } finally { h.readGate = null; release(); }
    } else await save(refresh, "row-0", scenario === "filtered removal" ? "" : "After");
    await settle();
    const result = counts();
    measured.push(result.total);
    console.log(`${scenario}: ${JSON.stringify(result)}`);
    checkState();
    if (scenario !== "status") assert.ok(client.getQueryData<number>(workflowLastEditKey(scope))! > 100, "translation writes stale QA");
    else assert.equal(client.getQueryData(workflowLastEditKey(scope)), null, "status-only leaves QA current");
    assert.ok(result.total <= (scenario === "filtered removal" ? 3 : scenario === "five rapid saves" ? 3 : 2), "target refetch budget");
    assert.deepEqual(result.families, scenario === "filtered removal" ? { strings: 1, stats: 1, "string-history": 1 }
      : { stats: scenario === "five rapid saves" ? 2 : 1, "string-history": 1 });
    assert.equal(result.duplicates, scenario === "five rapid saves" ? 1 : 0, "only the post-write stats catch-up repeats a key");
    assert.equal(result.overlaps, 0, "no concurrent requests for the same query key");
  });
}

test("the four measured cases together remove at least 30 percent of refetch requests", () => {
  assert.equal(measured.length, 4);
  assert.ok(measured.reduce((sum, count) => sum + count, 0) <= Object.values(baseline).reduce((sum, count) => sum + count, 0) * 0.7);
});

test("a status change removes a filtered row, refills its page, and stales only related inactive pages", async () => {
  const filter = { status: "translated", limit: 2, offset: 0 };
  const refresh = reset(filter);
  const next = ["strings", { ...filter, offset: 2 }, scope];
  const entering = ["strings", { status: "approved", limit: 2 }, scope];
  const unrelated = ["strings", { file_path: "unrelated.html" }, scope];
  const other = ["strings", filter, otherScope];
  const unfiltered = ["strings", { limit: 2 }, scope];
  for (const key of [next, entering, unrelated, unfiltered]) client.setQueryData(key, page(key[1] as StringFilter));
  client.setQueryData(other, "other database");
  await statusButton(detailTree(refresh), "approved").props.onClick();
  await settle();
  checkState();
  assert.deepEqual(client.getQueryData<StringsResponse>(["strings", filter, scope])?.entries.map(row => row.id), ["row-1", "row-2"]);
  assert.equal(client.getQueryState(next)?.isInvalidated, true);
  assert.equal(client.getQueryState(entering)?.isInvalidated, true);
  assert.equal(client.getQueryState(unrelated)?.isInvalidated, false);
  assert.equal(client.getQueryState(other)?.isInvalidated, false);
  assert.equal(client.getQueryData<StringsResponse>(unfiltered)?.entries[0].status, "approved");
  assert.equal(useEditorStore.getState().selectedEntryId, "row-0", "selected detail stays available outside the table filter");
  for (const family of ["validation-summary", "validation-issues", "translation-runs", "injection-status", "current-project", "memory-suggestions"]) {
    assert.equal(client.getQueryState([family, scope])?.isInvalidated, false, family);
  }
  assert.equal(client.getQueryState(["file-stats", scope])?.isInvalidated, true);
  assert.equal(client.getQueryState(["string-history", scope, "row-1"])?.isInvalidated, false);
  assert.equal(client.getQueryState(["string-facets", project.path])?.isInvalidated, false);
});

test("pending row/page reads cannot overwrite the saved server response", async () => {
  const refresh = reset();
  let release!: () => void;
  h.readGate = new Promise<void>(resolve => { release = resolve; });
  const reads = Promise.all([
    client.refetchQueries({ queryKey: ["strings"], type: "active" }),
    client.refetchQueries({ queryKey: ["string", "row-0", scope], exact: true }),
  ]);
  try {
    await setImmediate();
    await save(refresh);
    assert.equal(client.getQueryData<StringEntry>(["string", "row-0", scope])?.translation, "After");
  } finally { h.readGate = null; release(); }
  await reads; await settle();
  checkState();
});

test("writes during a stats read get a serial trailing read with the final counts", async () => {
  const refresh = reset();
  let release!: () => void;
  h.readGate = new Promise<void>(resolve => { release = resolve; });
  try {
    await statusButton(detailTree(refresh), "pending").props.onClick();
    await statusButton(detailTree(refresh), "approved").props.onClick();
    assert.equal(counts().families.stats, 1, "join the first read while it is pending");
  } finally { h.readGate = null; release(); }
  await settle();
  checkState();
  assert.equal(counts().families.stats, 2, "catch up once after the later write");
  assert.equal(counts().overlaps, 0);
});

test("status after a dirty translation refreshes membership from the intermediate server status", async () => {
  const refresh = reset({ status: "translated", limit: 2 }, { status: "approved" });
  const props = { projectKey: scope, entry: structuredClone(h.rows[0]), onClose() {},
    onRefetch(saved?: StringEntry, previous?: StringEntry) {
      void refresh(saved, previous);
      // A draft-store update can render the panel before the parent's query
      // subscription delivers new props. Retain the intermediate PATCH row.
      h.cursor = 0; DetailPanel(props);
    },
  };
  h.cursor = 0; h.states = [];
  const tree = DetailPanel(props);
  drafts.editDraft(drafts.draftEntryKey(scope, "row-0"), "Dirty translation", "Before");
  let release!: () => void;
  h.readGate = new Promise<void>(resolve => { release = resolve; });
  try { await statusButton(tree, "approved").props.onClick(); }
  finally { h.readGate = null; release(); }
  await settle();
  checkState();
  assert.equal(h.rows[0].status, "approved");
  assert.equal(h.rows[0].translation, "Dirty translation");
  assert.equal(client.getQueryData<StringsResponse>(["strings", { status: "translated", limit: 2 }, scope])?.total, 0);
});

for (const kind of ["translation", "status"] as const) {
  test(`a delayed ${kind} response after switching databases refreshes neither project's reads`, async () => {
    const refresh = reset();
    let release!: () => void;
    h.writeGate = new Promise<void>(resolve => { release = resolve; });
    const pending = kind === "translation" ? save(refresh) : statusButton(detailTree(refresh), "approved").props.onClick();
    try {
      await setImmediate();
      useProjectStore.getState().beginProjectTransition();
      await cancelProjectData(client, scope);
      useProjectStore.getState().setProject({ ...project, database_path: "C:/edit-fixture/fr.db" });
      const key = drafts.draftEntryKey(otherScope, "row-0");
      drafts.editDraft(key, "Other database draft", "Other baseline");
      client.setQueryData(["string", "row-0", otherScope], "other database");
      release(); await pending; await settle();
      assert.equal(counts().total, 0);
      assert.equal(client.getQueryData(["string", "row-0", otherScope]), "other database");
      assert.equal(client.getQueryData<StringEntry>(["string", "row-0", scope])?.translation, "Before");
      assert.equal(drafts.useDraftStore.getState().drafts[key].text, "Other database draft");
      assert.equal(client.getQueryData(workflowLastEditKey(otherScope)), undefined);
      if (kind === "translation") assert.ok(client.getQueryData<number>(workflowLastEditKey(scope))! > 100);
    } finally { h.writeGate = null; release(); await pending; }
  });
}

test("a project switch cancels pending edit reads and prevents their queued trailing reads", async () => {
  const refresh = reset();
  let release!: () => void;
  h.readGate = new Promise<void>(resolve => { release = resolve; });
  try {
    await statusButton(detailTree(refresh), "pending").props.onClick();
    await statusButton(detailTree(refresh), "approved").props.onClick();
    const beforeSwitch = counts().total;
    useProjectStore.getState().beginProjectTransition();
    await cancelProjectData(client, scope);
    useProjectStore.getState().setProject({ ...project, database_path: "C:/edit-fixture/fr.db" });
    h.readGate = null; release(); await settle();
    assert.equal(counts().total, beforeSwitch);
    assert.equal(client.getQueryState(["stats", scope])?.isInvalidated, true);
    assert.equal(client.getQueryData(["stats", otherScope]), undefined);
    assert.equal(client.getQueryData<StringEntry>(["string", "row-0", scope])?.status, "approved");
  } finally { h.readGate = null; release(); }
});

test("a conflict preserves the draft and performs no refresh; explicit overwrite still updates caches", async () => {
  const refresh = reset();
  const tree = detailTree(refresh);
  drafts.editDraft(drafts.draftEntryKey(scope, "row-0"), "Local", "Before");
  h.rows[0] = { ...h.rows[0], translation: "External edit" };
  await nodes(tree).find(node => node.type === "textarea")!.props.onBlur();
  const draft = drafts.useDraftStore.getState().drafts[drafts.draftEntryKey(scope, "row-0")];
  assert.equal(draft.conflict, true);
  assert.equal(draft.text, "Local");
  assert.equal(counts().total, 0);
  assert.equal(client.getQueryData(workflowLastEditKey(scope)), null);
  const overwrite = nodes(detailTree(refresh)).find(node => node.type === "button" && node.props.children === t("detail.overwrite"))!;
  await overwrite.props.onClick(); await settle();
  checkState();
  assert.equal(h.rows[0].translation, "Local");
});

test("closed history is left stale and refetches once when opened", async () => {
  const refresh = reset();
  const key = ["string-history", scope, "row-0"];
  h.observers.get(hashKey(key))!(); h.observers.delete(hashKey(key));
  await save(refresh); await settle();
  assert.equal(counts().families["string-history"], undefined);
  assert.equal(client.getQueryState(key)?.isInvalidated, true);
  const observer = new QueryObserver(client, { queryKey: key, staleTime: 0,
    queryFn: projectScopedQueryFn(scope, () => h.read(key, () => api.getStringHistory("row-0"))) });
  const unsubscribe = observer.subscribe(() => {});
  try {
    await settle();
    assert.equal(counts().families["string-history"], 1);
    checkState();
  } finally { unsubscribe(); }
});

test("facets refresh only when the server row changes a file path or tag", async () => {
  const refresh = reset();
  h.returnedFields = { file_path: "renamed.html", tags: ["new tag"] };
  await save(refresh); await settle();
  checkState();
  assert.equal(counts().families["string-facets"], 1);
  assert.deepEqual(client.getQueryData(["string-facets", project.path]), { file_paths: ["renamed.html", "story.html"], tags: ["new tag"] });
});
