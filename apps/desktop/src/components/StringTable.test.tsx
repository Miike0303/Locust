import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import * as tableCore from "@tanstack/react-table";
import { setLocale, t } from "../lib/i18n";
import { shouldRunActionHotkey } from "../lib/hotkeyPolicy";
import { useEditorStore } from "../stores/editorStore";
import { useProjectStore } from "../stores/projectStore";
import * as drafts from "../stores/draftStore";
import type { StringEntry } from "../lib/api";
import { ApiError } from "../lib/apiError";

// Invoke authored event handlers with controlled hooks; keep sorting and stores real.
const h = {
  states: [] as any[], cursor: 0, tableCore, t, drafts, useEditorStore, useProjectStore,
  effects: [] as (() => void)[], hotkeys: [] as any[], queryData: {} as Record<string, unknown>,
  writes: [] as { id: string; patch: any }[], reads: [] as string[],
  write: async (_id: string, _patch: any): Promise<void> => {},
  latest: "Latest translation",
  async patchString(id: string, patch: any) { h.writes.push({ id, patch }); await h.write(id, patch); },
  async getString(id: string) { h.reads.push(id); return { translation: h.latest }; },
  useState(initial: any) {
    const index = h.cursor++;
    const states = h.states;
    if (!(index in states)) states[index] = typeof initial === "function" ? initial() : initial;
    return [states[index], (value: any) => {
      states[index] = typeof value === "function" ? value(states[index]) : value;
    }];
  },
  useRef(initial: any) {
    const index = h.cursor++;
    return h.states[index] ??= { current: initial };
  },
  useEffect(effect: () => void | (() => void), deps?: unknown[]) {
    const index = h.cursor++;
    const previous = h.states[index];
    if (previous && deps && deps.length === previous.deps?.length && deps.every((dep, i) => Object.is(dep, previous.deps[i]))) return;
    const slot = { deps, cleanup: undefined as void | (() => void) };
    h.states[index] = slot;
    h.effects.push(() => { previous?.cleanup?.(); slot.cleanup = effect(); });
  },
  createTable(options: any) {
    const table = tableCore.createTable(options);
    table.setOptions({ ...options, state: { ...table.initialState, ...options.state } });
    return table;
  },
};
(globalThis as any).__editorAccess = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (/\/(components\/(StringTable|FilterBar|DetailPanel)|pages\/Editor|lib\/(modalA11y|hotkeys))\.tsx?$/.test(context.parentURL ?? "")) {
      const stubs: Record<string, string> = {
        react: "export const useState=h.useState;export const useRef=h.useRef;export const useMemo=fn=>fn();export const useCallback=fn=>fn;export const useEffect=h.useEffect;export const useId=()=>'test-dialog';",
        "../lib/i18n": "export const useT=()=>h.t;",
        "../stores/editorStore": "export const useEditorStore=selector=>selector?selector(h.useEditorStore.getState()):h.useEditorStore.getState();",
        "../stores/projectStore": "export const useProjectStore=Object.assign(selector=>selector?selector(h.useProjectStore.getState()):h.useProjectStore.getState(),{getState:h.useProjectStore.getState});",
        "../stores/draftStore": `export const useDraftStore=Object.assign(selector=>selector(h.drafts.useDraftStore.getState()),{getState:h.drafts.useDraftStore.getState});${Object.keys(drafts).filter(k => k !== "useDraftStore").map(k => `export const ${k}=h.drafts.${k};`).join("")}`,
        "@tanstack/react-query": "export const useIsMutating=()=>0;export const useQuery=options=>({data:h.queryData[options.queryKey[0]],isLoading:false,isError:false});export const useQueryClient=()=>({invalidateQueries:async()=>{}});",
        "@tanstack/react-table": "export const useReactTable=h.createTable;export const getCoreRowModel=h.tableCore.getCoreRowModel;export const getSortedRowModel=h.tableCore.getSortedRowModel;export const flexRender=(value,context)=>typeof value==='function'?value(context):value;",
        "react-router-dom": "export const useNavigate=()=>()=>{};",
        "../lib/projectScope": "export const useProjectScopedQueryEnabled=()=>true;export const projectScopedQueryFn=(_key,fn)=>fn;",
        "../lib/hotkeys": "export const useHotkey=(action,callback,enabled=true,allowInEditable=false)=>h.hotkeys.push({action,callback,enabled,allowInEditable});",
      };
      if (specifier === "../lib/api" && /\/StringTable\.tsx$/.test(context.parentURL ?? "")) {
        return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__editorAccess;export const getConfig=async()=>({});export const getString=h.getString;export const patchString=h.patchString;")}` };
      }
      if (stubs[specifier]) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__editorAccess;" + stubs[specifier])}` };
    }
    return nextResolve(specifier, context);
  },
});
const { default: StringTable } = await import("./StringTable");
const { default: FilterBar } = await import("./FilterBar");
const { default: DetailPanel } = await import("./DetailPanel");
const { default: Editor } = await import("../pages/Editor");
const { default: TranslationModal } = await import("./TranslationModal");
const { useModalA11y } = await import("../lib/modalA11y");
const { useHotkey } = await import("../lib/hotkeys");
const project = { path: "C:/fixture", name: "Fixture", format_id: "html-game" };
const projectKey = drafts.draftProjectKey(project);
const entries: StringEntry[] = ["Zulu", "Alpha", "Bravo"].map((source, index) => ({
  id: `row-${index}`, source, translation: "Translation", status: "translated", tags: [],
  metadata: {}, created_at: "2026-01-01", file_path: "story.html", context: null,
  provider_used: null, char_limit: 3, translated_at: null, reviewed_at: null,
}));
function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(nodes);
  return [node, ...nodes(node.props?.children)];
}
function renderTable(data = entries) {
  h.cursor = 0;
  return StringTable({ data, onRefetch() {} });
}
function rows(tree: any) { return nodes(tree).filter(n => n.type === "tr" && n.props.onClick); }
function key(row: any, value: string, extras = {}) {
  let prevented = false;
  row.props.onKeyDown({ key: value, target: row, currentTarget: row,
    preventDefault() { prevented = true; }, ...extras });
  return prevented;
}
beforeEach(() => {
  h.states = [];
  h.effects = []; h.hotkeys = []; h.queryData = {};
  h.writes = []; h.reads = []; h.write = async () => {}; h.latest = "Latest translation";
  useEditorStore.setState({ selectedEntryId: null, filter: { limit: 100, offset: 0 } });
  useProjectStore.setState({ project });
  drafts.useDraftStore.setState({ drafts: {}, records: [], persistenceIssue: null, persistenceIssues: {} });
  setLocale("en");
});
after(() => { hooks.deregister(); Reflect.deleteProperty(globalThis, "__editorAccess"); setLocale("en"); useProjectStore.setState({ project: null }); });

for (const locale of ["en", "es"] as const) {
  test(`${locale}: sortable header buttons expose sort state and current-page scope`, () => {
    setLocale(locale);
    const header = () => nodes(renderTable()).find(n => n.type === "th" && nodes(n).some(child => child.props?.title?.includes(t("table.source"))));
    let th = header();
    assert.ok(th, "column header has a named button");
    assert.equal(th.props["aria-sort"], "none");
    const button = nodes(th).find(n => n.type === "button");
    assert.equal(button.props.type, "button");
    assert.equal(button.props.title, locale === "en" ? "Sort this page by Source" : "Ordenar esta página por Origen");
    assert.ok(nodes(button).some(n => n.props.className === "sr-only" && n.props.children === button.props.title));
    assert.match(button.props.className, /focus-visible:ring-2 focus-visible:ring-accent-fg/);
    button.props.onClick({});
    th = header();
    assert.equal(th.props["aria-sort"], "ascending");
    assert.equal(rows(renderTable())[0].props.children[1].props.children.props.children, "Alpha");
    nodes(th).find(n => n.type === "button").props.onClick({});
    assert.equal(header().props["aria-sort"], "descending");
    nodes(header()).find(n => n.type === "button").props.onClick({});
    assert.equal(header().props["aria-sort"], "none");
  });

  test(`${locale}: pagination buttons have names and native disabled states; search has a label`, () => {
    setLocale(locale);
    const render = () => { h.cursor = 0; return nodes(FilterBar({ total: 200, showing: 100 })); };
    let tree = render();
    const previous = tree.find(n => n.type === "button" && n.props["aria-label"] === (locale === "en" ? "Previous page" : "Página anterior"));
    const next = tree.find(n => n.type === "button" && n.props["aria-label"] === (locale === "en" ? "Next page" : "Página siguiente"));
    assert.ok(previous); assert.ok(next);
    assert.equal(previous.props.disabled, true); assert.equal(next.props.disabled, false);
    next.props.onClick();
    tree = render();
    assert.equal(tree.find(n => n.props["aria-label"] === next.props["aria-label"]).props.disabled, true);
    assert.equal(tree.find(n => n.props["aria-label"] === previous.props["aria-label"]).props.disabled, false);
    const search = tree.find(n => n.type === "input" && "data-search-input" in n.props);
    assert.ok(search.props.id);
    assert.ok(tree.some(n => n.type === "label" && n.props.htmlFor === search.props.id && n.props.children === (locale === "en" ? "Search strings" : "Buscar cadenas")));
  });

  test(`${locale}: detail inputs have associated labels and linked help and validation errors`, () => {
    setLocale(locale);
    const tree = nodes(DetailPanel({ projectKey, entry: entries[0], onRefetch() {}, onClose() {} }));
    const labels = tree.filter(n => n.type === "label");
    assert.equal(labels.length, 2, "source and translation have native labels");
    for (const label of labels) {
      assert.ok(label.props.htmlFor);
      assert.ok(tree.some(n => n.props.id === label.props.htmlFor && ["textarea", "input", "select", "output"].includes(n.type)));
    }
    const translation = tree.find(n => n.type === "textarea" && !n.props.readOnly);
    assert.ok(labels.some(label => label.props.htmlFor === translation.props.id && label.props.children === t("detail.translation")));
    assert.equal(translation.props["aria-invalid"], true);
    const ids = translation.props["aria-describedby"].split(" ");
    assert.ok(ids.length >= 2, "help and limit error are both linked");
    for (const id of ids) assert.ok(tree.some(n => n.props.id === id), id);
    assert.ok(tree.some(n => ids.includes(n.props.id) && nodes(n).some(child => child === n && child.props.children?.includes?.(t("detail.exceedsLimit")))));
  });
}

test("ArrowDown then Enter opens second row details through the shared selection store", () => {
  let tree = renderTable();
  let bodyRows = rows(tree);
  assert.deepEqual(bodyRows.map(n => n.props.tabIndex), [0, -1, -1]);
  let focused = -1;
  nodes(tree).find(n => n.type === "tbody").props.ref.current = { rows: entries.map((_, i) => ({ focus() { focused = i; } })) };
  nodes(tree).find(n => n.type === "tbody").props.onFocus();
  bodyRows[0].props.onFocus();
  assert.equal(key(bodyRows[0], "ArrowDown"), true);
  assert.equal(focused, 1);
  bodyRows = rows(renderTable());
  assert.deepEqual(bodyRows.map(n => n.props.tabIndex), [-1, 0, -1]);
  assert.deepEqual(bodyRows.map(row => row.props["aria-selected"]), [false, false, false]);
  assert.match(bodyRows[1].props.className, /ring-1 ring-accent-fg/);
  assert.match(bodyRows[1].props.className, /bg-surface-muted/);
  assert.equal(useEditorStore.getState().selectedEntryId, null, "navigation does not open details");
  assert.equal(key(bodyRows[1], "Enter"), true);
  assert.equal(useEditorStore.getState().selectedEntryId, entries[1].id);
  assert.deepEqual(rows(renderTable()).map(row => row.props["aria-selected"]), [false, true, false]);
});

test("opened selection, not keyboard focus, determines aria-selected and the detail highlight", () => {
  useEditorStore.setState({ selectedEntryId: entries[0].id });
  let bodyRows = rows(renderTable());
  bodyRows[1].props.onFocus();
  bodyRows = rows(renderTable());
  assert.deepEqual(bodyRows.map(row => row.props["aria-selected"]), [true, false, false]);
  assert.match(bodyRows[0].props.className, /bg-accent-muted/);
  assert.doesNotMatch(bodyRows[1].props.className, /bg-accent-muted/);
  assert.equal(key(bodyRows[1], "Enter"), true);
  assert.deepEqual(rows(renderTable()).map(row => row.props["aria-selected"]), [false, true, false]);
  assert.deepEqual(rows(renderTable(entries.slice(2))).map(row => row.props["aria-selected"]), [false], "off-page details do not select a different visible row");
});

for (const locale of ["en", "es"] as const) {
  test(`${locale}: F2 invokes the current row's inline editor without opening details`, () => {
    setLocale(locale);
    const tree = renderTable();
    const row = rows(tree)[1];
    let clicked = 0;
    const target = { querySelector(selector: string) {
      assert.equal(selector, "[data-inline-edit]");
      return { click() { clicked++; } };
    } };
    assert.equal(key(row, "F2", { target, currentTarget: target }), true);
    assert.equal(clicked, 1);
    assert.equal(useEditorStore.getState().selectedEntryId, null);
    const caption = nodes(tree).find(node => node.type === "caption");
    assert.match(caption.props.children, /F2/);
    assert.match(caption.props.children, /Escape/);
  });
}

test("row navigation clamps at boundaries, follows sorting, and recovers when its row disappears", () => {
  let bodyRows = rows(renderTable());
  key(bodyRows[0], "ArrowUp");
  assert.equal(rows(renderTable())[0].props.tabIndex, 0);
  bodyRows[2].props.onFocus();
  key(bodyRows[2], "ArrowDown");
  assert.equal(rows(renderTable())[2].props.tabIndex, 0);
  const th = nodes(renderTable()).find(n => n.type === "th" && nodes(n).some(child => child.props?.title?.includes(t("table.source"))));
  nodes(th).find(n => n.type === "button").props.onClick({});
  bodyRows = rows(renderTable());
  assert.deepEqual(bodyRows.map(n => n.props.tabIndex), [-1, 0, -1], "focus stays on Bravo after sorting");
  key(bodyRows[1], "Enter");
  assert.equal(useEditorStore.getState().selectedEntryId, entries[2].id);
  assert.deepEqual(rows(renderTable(entries.slice(0, 2))).map(n => n.props.tabIndex), [0, -1]);
  assert.equal(rows(renderTable([])).length, 0);
});

test("row keys ignore nested controls and modifiers and preserve action hotkeys", () => {
  const row = rows(renderTable())[0];
  for (const value of ["ArrowDown", "ArrowUp", "Enter", "F2"]) {
    for (const tagName of ["TEXTAREA", "BUTTON", "INPUT"]) {
      assert.equal(key(row, value, { target: { tagName } }), false);
    }
    for (const modifier of ["ctrlKey", "metaKey", "altKey", "shiftKey"]) {
      assert.equal(key(row, value, { [modifier]: true }), false);
    }
  }
  for (const value of ["f", "t", "?", "Escape"]) assert.equal(key(row, value), false);
  assert.equal(shouldRunActionHotkey({ overlayOpen: false, target: { tagName: "TR" } }), true);
  assert.equal(useEditorStore.getState().selectedEntryId, null);
});

test("inline edit triggers leave one row in the table body's tab order", () => {
  const tree = renderTable();
  const inline = nodes(tree).find(n => typeof n.type === "function" && n.type.name === "InlineEdit");
  h.states = []; h.cursor = 0;
  const button = nodes(inline.type(inline.props)).find(n => n.type === "button");
  assert.equal(button.props.tabIndex, -1);
  assert.equal(rows(tree).filter(n => n.props.tabIndex === 0).length, 1);
});

// Model native row/button focus and dispatch while executing the authored handlers/effects.
// Each component keeps its own hook slots; draft persistence and save guards remain real.
function inlineHarness(index = 1) {
  const tableState: any[] = [], inlineState: any[] = [];
  let focused = 0, refetched = 0, stopped = 0;
  function scoped(states: any[], render: () => any) {
    h.states = states; h.cursor = 0; h.effects = [];
    const tree = render();
    h.effects.forEach(effect => effect());
    return tree;
  }
  const table = () => scoped(tableState, () => StringTable({ data: entries, onRefetch() { refetched++; } }));
  const row = () => rows(table())[index];
  const inline = () => {
    const cell = nodes(row()).find(n => typeof n.type === "function" && n.type.name === "InlineEdit");
    return scoped(inlineState, () => cell.type(cell.props));
  };
  const button = () => nodes(inline()).find(n => n.type === "button" && n.props.tabIndex === -1);
  const input = () => nodes(inline()).find(n => n.type === "textarea");
  const domRow = {
    focus() { focused++; row().props.onFocus(); },
    querySelector(selector: string) {
      assert.equal(selector, "[data-inline-edit]");
      const trigger = button();
      assert.equal(trigger.props["data-inline-edit"], true);
      return { click() { if (!trigger.props.disabled) trigger.props.onClick({ stopPropagation() { stopped++; } }); } };
    },
  };
  return {
    inline, input, button, table,
    enter() { return key(row(), "F2", { target: domRow, currentTarget: domRow }); },
    click() { button().props.onClick({ stopPropagation() { stopped++; } }); },
    change(text: string) { input().props.onChange({ target: { value: text } }); },
    press(value: string, extras = {}) {
      let prevented = false, propagationStopped = false;
      input().props.onKeyDown({ key: value, currentTarget: { closest(selector: string) {
        assert.equal(selector, "tr"); return domRow;
      } }, preventDefault() { prevented = true; }, stopPropagation() { propagationStopped = true; }, ...extras });
      return { prevented, propagationStopped };
    },
    counts: () => ({ focused, refetched, stopped }),
  };
}
const settleWrites = () => new Promise(resolve => setImmediate(resolve));

for (const locale of ["en", "es"] as const) {
  test(`${locale}: F2 edits the current draft; Escape cancels only this session and restores row focus`, () => {
    setLocale(locale);
    const draftKey = drafts.draftEntryKey(projectKey, entries[1].id);
    drafts.editDraft(draftKey, "Earlier draft", entries[1].translation);
    useEditorStore.setState({ selectedEntryId: entries[0].id });
    const view = inlineHarness();
    assert.equal(view.enter(), true);
    const input = view.input();
    assert.equal(input.props.autoFocus, true);
    assert.equal(input.props.value, "Earlier draft");
    assert.equal(input.props["aria-label"], t("table.editTranslation"));
    assert.match(input.props["aria-description"], /F2.*Ctrl\+Enter.*Escape/);
    view.change("Cancelled text");
    assert.deepEqual(view.press("Escape"), { prevented: true, propagationStopped: true });
    // React's removal/focus transfer may blur the old textarea: never save cancelled text.
    void input.props.onBlur();
    assert.equal(view.input(), undefined);
    assert.equal(drafts.useDraftStore.getState().drafts[draftKey].text, "Earlier draft");
    assert.deepEqual(h.writes, []);
    assert.deepEqual(view.counts(), { focused: 1, refetched: 0, stopped: 1 });
    assert.equal(useEditorStore.getState().selectedEntryId, entries[0].id);
    assert.deepEqual(rows(view.table()).map(row => row.props.tabIndex), [-1, 0, -1]);
  });
}

test("pointer inline entry stays independent of row selection; cancelling a fresh edit clears its draft", () => {
  const view = inlineHarness();
  view.click();
  assert.equal(useEditorStore.getState().selectedEntryId, null);
  view.change("Discard");
  assert.deepEqual(view.press("Escape", { shiftKey: true }), { prevented: true, propagationStopped: true }, "modified Escape retains its prior cancel semantics");
  assert.equal(view.input(), undefined);
  assert.equal(drafts.useDraftStore.getState().drafts[drafts.draftEntryKey(projectKey, entries[1].id)], undefined);
  assert.deepEqual(h.writes, []);
});

test("plain/Meta Enter stay in the textarea; Ctrl+Enter saves once with the existing optimistic guard", async () => {
  const view = inlineHarness();
  view.enter();
  view.change("Saved text");
  assert.deepEqual(view.press("Enter"), { prevented: false, propagationStopped: false });
  assert.deepEqual(view.press("Enter", { metaKey: true }), { prevented: false, propagationStopped: false });
  const input = view.input();
  assert.equal(view.press("Enter", { ctrlKey: true }).prevented, true);
  await input.props.onBlur();
  await settleWrites();
  assert.deepEqual(h.writes, [{ id: entries[1].id, patch: { translation: "Saved text", expected_translation: "Translation" } }]);
  assert.equal(view.input(), undefined);
  assert.equal(view.counts().refetched, 1);
});

test("blur saves, ordinary errors retain draft and Retry uses the same guard", async () => {
  const view = inlineHarness();
  view.enter(); view.change("Retry text");
  h.write = async () => { throw new Error("Offline"); };
  await view.input().props.onBlur();
  const draftKey = drafts.draftEntryKey(projectKey, entries[1].id);
  assert.equal(drafts.useDraftStore.getState().drafts[draftKey].text, "Retry text");
  const tree = nodes(view.inline());
  assert.ok(tree.some(n => n.props.role === "alert" && n.props.children === "Offline"));
  h.write = async () => {};
  tree.find(n => n.type === "button" && n.props.children === t("common.retry")).props.onClick();
  await settleWrites();
  assert.equal(h.writes.length, 2);
  assert.deepEqual(h.writes[1], h.writes[0]);
  assert.equal(view.counts().refetched, 1);
});

for (const recovery of ["loadLatest", "overwrite"] as const) {
  test(`inline conflicts block blur and preserve explicit ${recovery} recovery`, async () => {
    const view = inlineHarness();
    view.enter(); view.change("Local text");
    h.write = async () => { throw new ApiError("409: translation changed since it was loaded"); };
    await view.input().props.onBlur();
    const draftKey = drafts.draftEntryKey(projectKey, entries[1].id);
    assert.equal(drafts.useDraftStore.getState().drafts[draftKey].conflict, true);
    view.enter(); await view.input().props.onBlur();
    assert.equal(h.writes.length, 1, "ordinary blur never overwrites a conflict");
    h.write = async () => {};
    await nodes(view.inline()).find(n => n.type === "button" && n.props.children === t(`detail.${recovery}`)).props.onClick();
    await settleWrites();
    assert.equal(drafts.useDraftStore.getState().drafts[draftKey].conflict, false);
    if (recovery === "loadLatest") {
      assert.deepEqual(h.reads, [entries[1].id]);
      assert.equal(drafts.useDraftStore.getState().drafts[draftKey].text, h.latest);
    } else {
      assert.deepEqual(h.writes[1].patch, { translation: "Local text" }, "explicit overwrite omits the optimistic guard");
    }
    assert.equal(view.counts().refetched, 1);
  });
}

test("F2 respects disabled entry and in-flight saves; project changes cannot write a stale draft", async () => {
  const view = inlineHarness();
  useProjectStore.setState({ project: null });
  assert.equal(view.button().props.disabled, true);
  view.enter(); assert.equal(view.input(), undefined);
  useProjectStore.setState({ project });
  view.enter(); view.change("Project A draft");
  const oldInput = view.input();
  const other = { ...project, path: "C:/other" };
  useProjectStore.setState({ project: other });
  await oldInput.props.onBlur();
  assert.deepEqual(h.writes, []);
  assert.equal(view.button().props.children, "Translation");
  assert.equal(drafts.useDraftStore.getState().drafts[drafts.draftEntryKey(projectKey, entries[1].id)].text, "Project A draft");
  useProjectStore.setState({ project });
  let complete!: () => void;
  h.write = () => new Promise<void>(resolve => { complete = resolve; });
  view.enter(); const pending = view.input().props.onBlur();
  await Promise.resolve();
  assert.equal(view.button().props.disabled, true);
  view.enter(); assert.equal(view.input(), undefined);
  complete(); await pending;
  assert.equal(view.button().props.disabled, false);
});

test("row pointer selection still opens details and focuses the clicked row", () => {
  let focused = 0;
  rows(renderTable())[2].props.onClick({ currentTarget: { focus() { focused++; } } });
  assert.equal(focused, 1);
  assert.equal(useEditorStore.getState().selectedEntryId, entries[2].id);
  assert.deepEqual(rows(renderTable()).map(row => row.props["aria-selected"]), [false, false, true]);
});

test("detail describes save conflicts and storage errors without dangling references", () => {
  const draftKey = drafts.draftEntryKey(projectKey, entries[0].id);
  drafts.editDraft(draftKey, "Draft", entries[0].translation);
  drafts.useDraftStore.setState(state => ({
    drafts: { ...state.drafts, [draftKey]: { ...state.drafts[draftKey], error: "Save failed", conflict: true } },
    persistenceIssues: { [draftKey]: "unavailable" },
  }));
  const tree = nodes(DetailPanel({ projectKey, entry: entries[0], onRefetch() {}, onClose() {} }));
  const input = tree.find(n => n.type === "textarea");
  const descriptions = input.props["aria-describedby"].split(" ").map((id: string) => tree.find(n => n.props.id === id));
  assert.ok(descriptions.every(Boolean));
  assert.ok(descriptions.some((n: any) => n.props.children === "Save failed"));
  assert.ok(descriptions.some((n: any) => n.props.children === t("api.error.translationConflict")));
  assert.ok(descriptions.some((n: any) => n.props.children === t("detail.draftStorageFailed")));
});

test("Editor focuses opened details; Escape and Close restore the roving row, preserving modal precedence", () => {
  useEditorStore.setState({ selectedEntryId: entries[1].id });
  h.queryData = { strings: { entries, total: entries.length }, string: entries[1] };
  const render = () => { h.cursor = 0; h.hotkeys = []; h.effects = []; return nodes(Editor()); };
  let tree = render();
  const table = tree.find(n => n.type === StringTable);
  let rowFocused = 0, inputFocused = 0;
  table.props.tableRef.current = { querySelector(selector: string) {
    assert.equal(selector, 'tbody tr[tabindex="0"]');
    return { focus() { rowFocused++; } };
  } };
  const detail = tree.find(n => n.type === DetailPanel);
  detail.props.panelRef.current = { querySelector(selector: string) {
    assert.equal(selector, "textarea:not([readonly])");
    return { focus() { inputFocused++; } };
  } };
  h.effects.forEach(effect => effect());
  assert.equal(inputFocused, 1);
  const close = h.hotkeys.find(binding => binding.action === "closePanel");
  assert.equal(close.enabled, true);
  assert.equal(close.allowInEditable, true, "Escape works in the translation field");
  close.callback();
  assert.equal(useEditorStore.getState().selectedEntryId, null);
  assert.equal(rowFocused, 1);
  useEditorStore.setState({ selectedEntryId: entries[1].id });
  tree = render();
  tree.find(n => n.type === DetailPanel).props.onClose();
  assert.equal(rowFocused, 2);
  useEditorStore.setState({ selectedEntryId: entries[1].id });
  render();
  const translate = h.hotkeys.find(binding => binding.action === "translate");
  assert.equal(translate.enabled, true, "table action hotkey remains available");
  translate.callback();
  render();
  assert.equal(h.hotkeys.find(binding => binding.action === "translate").enabled, false);
  h.hotkeys.find(binding => binding.action === "closePanel").callback();
  assert.equal(useEditorStore.getState().selectedEntryId, entries[1].id, "Escape closes the work modal first");
  assert.equal(rowFocused, 2);
});

test("closing details returns focus to the table when filters leave no visible rows", () => {
  useEditorStore.setState({ selectedEntryId: entries[0].id });
  h.queryData = { strings: { entries: [], total: 0 }, string: entries[0] };
  const tree = nodes(Editor());
  let focused = false;
  tree.find(n => n.type === StringTable).props.tableRef.current = {
    querySelector() { return null; }, focus() { focused = true; },
  };
  h.hotkeys.find(binding => binding.action === "closePanel").callback();
  assert.equal(useEditorStore.getState().selectedEntryId, null);
  assert.equal(focused, true);
});


test("open details: row focused -> Ctrl+T -> Escape restores the same row and table action hotkeys", () => {
  useEditorStore.setState({ selectedEntryId: entries[1].id });
  h.queryData = { strings: { entries, total: entries.length }, string: entries[1] };
  const editorState: any[] = [], modalState: any[] = [];
  const bindingStates = new Map<string, any[]>();
  const listeners = new Set<(event: any) => void>();
  let active: any = null, overlayOpen = false;
  const row = { tagName: "TR", isConnected: true, focus() { active = row; } };
  const textarea = { tagName: "TEXTAREA", focus() { active = textarea; } };
  const modalButton = { tagName: "BUTTON", focus() { active = modalButton; } };
  const dialog = { querySelector() { return modalButton; } };
  const originals = ["document", "window", "navigator"].map(name => [name, Object.getOwnPropertyDescriptor(globalThis, name)] as const);
  Object.defineProperty(globalThis, "document", { configurable: true, value: {
    get activeElement() { return active; },
    querySelector() { return overlayOpen ? dialog : null; },
  } });
  Object.defineProperty(globalThis, "window", { configurable: true, value: {
    addEventListener(_type: string, callback: (event: any) => void) { listeners.add(callback); },
    removeEventListener(_type: string, callback: (event: any) => void) { listeners.delete(callback); },
  } });
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: { platform: "Win32" } });
  function scoped<T>(states: any[], render: () => T) {
    h.states = states; h.cursor = 0; h.effects = [];
    const result = render();
    return { result, effects: [...h.effects] };
  }
  function render() {
    h.hotkeys = [];
    const editor = scoped(editorState, () => nodes(Editor()));
    editor.result.find(n => n.type === StringTable).props.tableRef.current = { querySelector() { return row; } };
    const detail = editor.result.find(n => n.type === DetailPanel);
    if (detail) detail.props.panelRef.current = { querySelector() { return textarea; } };
    const modalProps = editor.result.find(n => n.type === TranslationModal).props;
    overlayOpen = modalProps.open;
    const modal = scoped(modalState, () => useModalA11y({ open: modalProps.open, onClose: modalProps.onClose }));
    modal.result.dialogRef.current = dialog as unknown as HTMLDivElement;
    // React runs the modal's cleanup before the parent's passive autofocus effect.
    modal.effects.forEach(effect => effect());
    editor.effects.forEach(effect => effect());
    for (const binding of h.hotkeys) {
      const states = bindingStates.get(binding.action) ?? [];
      bindingStates.set(binding.action, states);
      scoped(states, () => useHotkey(binding.action, binding.callback, binding.enabled, binding.allowInEditable))
        .effects.forEach(effect => effect());
    }
    return modalProps;
  }
  function press(key: string, ctrlKey = false) {
    let prevented = false, stopped = false;
    const event = { key, ctrlKey, shiftKey: false, altKey: false, metaKey: false, target: active,
      preventDefault() { prevented = true; }, stopImmediatePropagation() { stopped = true; } };
    for (const listener of [...listeners]) { listener(event); if (stopped) break; }
    assert.equal(prevented, true, `${ctrlKey ? "Ctrl+" : ""}${key} is handled`);
  }
  try {
    assert.equal(render().open, false);
    assert.equal(active, textarea, "opening details initially focuses the editor");
    row.focus();
    press("t", true);
    assert.equal(render().open, true);
    assert.equal(active, modalButton, "the translation modal takes focus");
    press("Escape");
    assert.equal(render().open, false);
    assert.equal(useEditorStore.getState().selectedEntryId, entries[1].id, "details remain open");
    assert.equal(active, row, "modal close preserves focus on the same opener row");
    assert.equal(shouldRunActionHotkey({ overlayOpen, target: active }), true);
    press("t", true);
    assert.equal(render().open, true, "Ctrl+T still opens the modal from the restored row");
  } finally {
    for (const [name, descriptor] of originals) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else Reflect.deleteProperty(globalThis, name);
    }
  }
});


test("detail autofocus follows opening and entry changes, without deferring focus until modal close", () => {
  useEditorStore.setState({ selectedEntryId: entries[0].id });
  h.queryData = { strings: { entries, total: entries.length }, string: entries[0] };
  let focused = 0;
  let panelRef: any;
  const render = () => {
    h.cursor = 0; h.hotkeys = []; h.effects = [];
    if (panelRef) panelRef.current = null;
    const tree = nodes(Editor());
    const detail = tree.find(n => n.type === DetailPanel);
    if (detail) {
      panelRef = detail.props.panelRef;
      panelRef.current = { querySelector() { return { focus() { focused++; } }; } };
    }
    h.effects.forEach(effect => effect());
  };
  render();
  assert.equal(focused, 1, "opening details focuses the editor");
  render();
  assert.equal(focused, 1, "ordinary renders preserve the user's focus");
  useEditorStore.setState({ selectedEntryId: entries[1].id });
  h.queryData.string = entries[1];
  render();
  assert.equal(focused, 2, "selecting another entry focuses its editor");
  useEditorStore.setState({ selectedEntryId: null });
  h.queryData.string = undefined;
  render();
  useEditorStore.setState({ selectedEntryId: entries[1].id });
  h.queryData.string = entries[1];
  render();
  assert.equal(focused, 3, "reopening the same entry focuses its editor");
  h.hotkeys.find(binding => binding.action === "translate").callback();
  render();
  useEditorStore.setState({ selectedEntryId: entries[2].id });
  h.queryData.string = entries[2];
  render();
  assert.equal(focused, 3, "selection changes behind a modal do not steal focus");
  h.hotkeys.find(binding => binding.action === "closePanel").callback();
  render();
  assert.equal(focused, 3, "closing a modal does not defer the earlier autofocus");
});
