import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import * as tableCore from "@tanstack/react-table";
import { t, setLocale } from "./i18n";
import * as drafts from "../stores/draftStore";
import { useEditorStore } from "../stores/editorStore";
import { useProjectStore } from "../stores/projectStore";
import { useLogStore, addLog } from "../stores/logStore";
import { useQueueStore } from "../stores/queueStore";
import { readSidebarCollapsed, saveSidebarCollapsed, isCompactLayout, subscribeCompactLayout } from "./layoutPrefs";

// Exercise authored handlers and effects with controlled React hooks and real stores.
const h = {
  states: [] as any[], cursor: 0, effects: [] as (() => void)[], hotkeys: [] as any[],
  queryData: {} as Record<string, unknown>, tableCore, t, drafts, useEditorStore, useProjectStore, useLogStore, useQueueStore, addLog,
  useState(initial: any) {
    const index = h.cursor++;
    if (!(index in h.states)) h.states[index] = typeof initial === "function" ? initial() : initial;
    const states = h.states;
    return [states[index], (value: any) => { states[index] = typeof value === "function" ? value(states[index]) : value; }];
  },
  useRef(initial: any) { return h.states[h.cursor++] ??= { current: initial }; },
  useEffect(effect: () => void | (() => void), deps?: unknown[]) {
    const index = h.cursor++, previous = h.states[index];
    if (previous && deps?.every((dep, i) => Object.is(dep, previous.deps?.[i]))) return;
    const slot = { deps, cleanup: undefined as void | (() => void) };
    h.states[index] = slot;
    h.effects.push(() => { previous?.cleanup?.(); slot.cleanup = effect(); });
  },
};
(globalThis as any).__compactLayout = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (/\/(components\/(Layout|DetailPanel|StringTable)|pages\/(Settings|Editor)|lib\/modalA11y)\.tsx?$/.test(context.parentURL ?? "")) {
      const stubs: Record<string, string> = {
        react: "export const useState=h.useState;export const useRef=h.useRef;export const useEffect=h.useEffect;export const useMemo=fn=>fn();export const useCallback=fn=>fn;export const useId=()=>'drawer-title';export const Suspense='suspense';",
        "@tanstack/react-table": "export const useReactTable=options=>{const table=h.tableCore.createTable(options);table.setOptions({...options,state:{...table.initialState,...options.state}});return table;};export const getCoreRowModel=h.tableCore.getCoreRowModel;export const getSortedRowModel=h.tableCore.getSortedRowModel;export const flexRender=(value,context)=>typeof value==='function'?value(context):value;",
        "../lib/i18n": "export const useT=()=>h.t;",
        "@tanstack/react-query": "export const useIsMutating=()=>0;export const useQuery=o=>({data:h.queryData[o.queryKey[0]],isLoading:false,isError:false});export const useQueryClient=()=>({invalidateQueries:async()=>{}});",
        "react-router-dom": "export const NavLink='nav-link';export const Outlet='outlet';export const useLocation=()=>({pathname:'/settings',search:'?section=appearance'});export const useNavigate=()=>()=>{};",
        "../lib/hotkeys": "export const useGlobalHotkeys=()=>{};export const useHotkey=(action,callback,enabled=true)=>h.hotkeys.push({action,callback,enabled});",
        "../lib/projectScope": "export const useProjectScopedQueryEnabled=()=>true;export const projectScopedQueryFn=(_key,fn)=>fn;",
        ...Object.fromEntries(["editor", "project", "log", "queue"].map(name => {
          const symbol = `use${name[0].toUpperCase()}${name.slice(1)}Store`;
          return [`../stores/${name}Store`, `export const ${symbol}=Object.assign(selector=>selector?selector(h.${symbol}.getState()):h.${symbol}.getState(),{getState:h.${symbol}.getState});${name === "log" ? "export const addLog=h.addLog;" : ""}`];
        })),
        "../stores/draftStore": `export const useDraftStore=selector=>selector(h.drafts.useDraftStore.getState());${Object.keys(drafts).filter(k => k !== "useDraftStore").map(k => `export const ${k}=h.drafts.${k};`).join("")}`,
      };
      if (stubs[specifier]) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__compactLayout;" + stubs[specifier])}` };
    }
    return nextResolve(specifier, context);
  },
});
const { default: Layout } = await import("../components/Layout");
const { default: Settings } = await import("../pages/Settings");
const { default: DetailPanel } = await import("../components/DetailPanel");
const { default: Editor } = await import("../pages/Editor");
const { default: StringTable } = await import("../components/StringTable");
function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(nodes);
  return [node, ...nodes(node.props?.children)];
}
function render(component: () => any, states = h.states) {
  h.states = states; h.cursor = 0; h.effects = []; h.hotkeys = [];
  return nodes(component());
}
function cleanup(states = h.states) { states.forEach(slot => slot?.cleanup?.()); }
const originals = new Map(["window", "document", "localStorage"].map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
let compact = true;
let storage: Storage;
let mediaListeners: Set<(event: { matches: boolean }) => void>;
let keyListeners: Set<(event: any) => void>;
let focusListeners: Set<(event: any) => void>;
let active: any;
let dialogs: any[];
const project = { path: "C:/fixture", name: "Fixture", format_id: "html-game" };
const projectKey = drafts.draftProjectKey(project);
const entry = { id: "row-1", source: "Source", translation: "Translation", status: "translated" as const, tags: [], metadata: {}, created_at: "2026-01-01", file_path: "story.html", context: null, provider_used: null, char_limit: null, translated_at: null, reviewed_at: null };
beforeEach(() => {
  h.states = []; h.effects = []; h.queryData = {};
  compact = true; mediaListeners = new Set(); keyListeners = new Set(); focusListeners = new Set(); active = null; dialogs = [];
  const values = new Map<string, string>();
  storage = { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => { values.set(key, value); } } as Storage;
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: storage });
  Object.defineProperty(globalThis, "window", { configurable: true, value: {
    matchMedia: () => ({ get matches() { return compact; }, addEventListener: (_: string, listener: any) => mediaListeners.add(listener), removeEventListener: (_: string, listener: any) => mediaListeners.delete(listener) }),
    addEventListener: (type: string, listener: any) => { if (type === "keydown") keyListeners.add(listener); else if (type === "focusin") focusListeners.add(listener); },
    removeEventListener: (type: string, listener: any) => { if (type === "keydown") keyListeners.delete(listener); else if (type === "focusin") focusListeners.delete(listener); },
  } });
  Object.defineProperty(globalThis, "document", { configurable: true, value: {
    documentElement: { lang: "en" },
    get activeElement() { return active; },
    querySelectorAll: () => ({ item: (i: number) => dialogs[i], length: dialogs.length }),
  } });
  setLocale("en");
  useProjectStore.setState({ project });
  useEditorStore.setState({ selectedEntryId: null });
  useLogStore.setState({ unreadErrors: 7 });
  useQueueStore.setState({ items: [], isRunning: false });
  drafts.useDraftStore.setState({ drafts: {}, records: [], persistenceIssue: null, persistenceIssues: {} });
});
after(() => {
  hooks.deregister(); Reflect.deleteProperty(globalThis, "__compactLayout"); setLocale("en");
  for (const [key, descriptor] of originals) {
    if (descriptor) Object.defineProperty(globalThis, key, descriptor);
    else Reflect.deleteProperty(globalThis, key);
  }
});

test("sidebar preference preserves both explicit choices and ignores malformed values", () => {
  assert.equal(readSidebarCollapsed(), null);
  saveSidebarCollapsed(true);
  assert.equal(storage.getItem("locust.ui.sidebarCollapsed"), "true");
  assert.equal(readSidebarCollapsed(), true);
  saveSidebarCollapsed(false);
  assert.equal(readSidebarCollapsed(), false);
  storage.setItem("locust.ui.sidebarCollapsed", "invalid");
  assert.equal(readSidebarCollapsed(), null);
});
test("sidebar preference tolerates unavailable and throwing storage", () => {
  Object.defineProperty(globalThis, "localStorage", { configurable: true, get() { throw new Error("denied"); } });
  assert.equal(readSidebarCollapsed(), null);
  assert.doesNotThrow(() => saveSidebarCollapsed(true));
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: { getItem() { throw new Error("denied"); }, setItem() { throw new Error("denied"); } } });
  assert.equal(readSidebarCollapsed(), null);
  assert.doesNotThrow(() => saveSidebarCollapsed(false));
});
test("compact layout observes the 1200px boundary and removes its listener", () => {
  const queries: string[] = [];
  const matchMedia = window.matchMedia;
  window.matchMedia = query => { queries.push(query); return matchMedia(query); };
  assert.equal(isCompactLayout(), true);
  let seen = false;
  const unsubscribe = subscribeCompactLayout(value => { seen = value; });
  mediaListeners.forEach(listener => listener({ matches: true }));
  assert.equal(seen, true);
  assert.ok(queries.every(query => query === "(width < 1200px)"));
  unsubscribe();
  assert.equal(mediaListeners.size, 0);
  Reflect.deleteProperty(globalThis, "window");
  assert.equal(isCompactLayout(), false);
  assert.doesNotThrow(() => subscribeCompactLayout(() => {})());
});
for (const locale of ["en", "es"] as const) {
  test(`${locale}: collapsed rail names every navigation and utility control, including tooltips and badges`, () => {
    setLocale(locale);
    const tree = render(Layout);
    assert.equal(tree.find(n => n.type === "aside").props["data-collapsed"], true);
    const links = tree.filter(n => n.type === "nav-link");
    assert.equal(links.length, 8);
    for (const control of [...links, ...tree.filter(n => ["button", "a"].includes(n.type))]) {
      assert.ok(control.props["aria-label"]);
      assert.equal(control.props.title, control.props["aria-label"]);
      assert.ok(nodes(control).some(n => n.props.role === "tooltip" && n.props.children === control.props["aria-label"]));
    }
    assert.ok(tree.some(n => n.type === "span" && n.props.children === 7), "Activity Log badge survives");
    assert.ok(tree.some(n => n.props.children === "L"));
  });
}
test("sidebar follows viewport defaults until toggled and persists expansion across remount and resize", () => {
  let tree = render(Layout);
  h.effects.forEach(effect => effect());
  assert.equal(tree.find(n => n.type === "aside").props["data-collapsed"], true);
  compact = false; mediaListeners.forEach(listener => listener({ matches: compact }));
  tree = render(Layout);
  assert.equal(tree.find(n => n.type === "aside").props["data-collapsed"], false);
  compact = true; mediaListeners.forEach(listener => listener({ matches: compact }));
  tree = render(Layout);
  tree.find(n => n.props["aria-label"] === t("nav.expandSidebar")).props.onClick();
  tree = render(Layout);
  assert.equal(tree.find(n => n.type === "aside").props["data-collapsed"], false);
  assert.equal(readSidebarCollapsed(), false);
  cleanup(); h.states = [];
  tree = render(Layout);
  assert.equal(tree.find(n => n.type === "aside").props["data-collapsed"], false);
  tree.find(n => n.props["aria-label"] === t("nav.collapseSidebar")).props.onClick();
  assert.equal(readSidebarCollapsed(), true);
  cleanup(); h.states = []; compact = false;
  assert.equal(render(Layout).find(n => n.type === "aside").props["data-collapsed"], true);
});
test("Settings has one scrollable compact tab row with all sections and the active underline", () => {
  const tree = render(Settings), nav = tree.find(n => n.type === "nav");
  assert.match(nav.props.className, /overflow-x-auto/);
  assert.match(nav.props.className, /border-b border-border/);
  assert.match(nav.props.className, /min-\[1200px\]:w-56/);
  const buttons = nodes(nav).filter(n => n.type === "button");
  assert.equal(buttons.length, 6);
  const current = buttons.filter(n => n.props["aria-current"] === "page");
  assert.equal(current.length, 1);
  assert.equal(current[0].props.children, t("settings.nav.appearance"));
  assert.match(current[0].props.className, /border-accent/);
});
test("running queue retains its accent in the icon rail", () => {
  useQueueStore.setState({ isRunning: true });
  const queue = render(Layout).find(n => n.props["aria-label"] === t("nav.queue"));
  assert.match(queue.props.className, /text-accent-fg/);
  assert.doesNotMatch(queue.props.className, /text-text-muted/);
});
for (const closeWith of ["Escape", "backdrop", "button"] as const) {
  test(`compact detail ${closeWith} closes to the roving row and retains the draft on reopen`, () => {
    const editorState: any[] = [], detailState: any[] = [];
    useEditorStore.setState({ selectedEntryId: entry.id });
    h.queryData = { strings: { entries: [entry], total: 1 }, string: entry };
    const tree = render(Editor, editorState);
    const detail = tree.find(n => n.type === DetailPanel);
    let rowFocus = 0;
    const row = { isConnected: true, focus() { active = row; rowFocus++; } };
    const table = tree.find(n => n.props.tableRef);
    table.props.tableRef.current = { querySelector: () => row };
    active = row;
    const panel = render(() => DetailPanel(detail.props), detailState);
    const dialog = panel.find(n => n.props.role === "dialog");
    assert.ok(dialog, "compact details are a modal drawer");
    assert.equal(dialog.props["aria-modal"], true);
    const textarea = panel.find(n => n.type === "textarea");
    const input = { focus() { active = input; } };
    textarea.props.ref.current = input;
    const root = { contains: () => true, querySelector: () => input, querySelectorAll: () => [], focus() {} };
    dialog.props.ref.current = root; dialogs = [root];
    h.effects.forEach(effect => effect());
    assert.equal(active, input, "drawer focuses its translation field");
    textarea.props.onChange({ target: { value: "Unsaved compact draft" } });
    if (closeWith === "Escape") {
      let stopped = false;
      keyListeners.forEach(listener => listener({ key: "Escape", preventDefault() {}, stopImmediatePropagation() { stopped = true; } }));
      assert.equal(stopped, true);
    } else if (closeWith === "backdrop") {
      panel.find(n => n.props["data-detail-backdrop"] !== undefined).props.onClick();
    } else panel.find(n => n.props["aria-label"] === t("detail.closeAria")).props.onClick();
    assert.equal(useEditorStore.getState().selectedEntryId, null);
    assert.ok(rowFocus > 0);
    cleanup(detailState);
    assert.equal(active, row);
    dialogs = [];
    const reopened = render(() => DetailPanel(detail.props), []);
    assert.equal(reopened.find(n => n.type === "textarea").props.value, "Unsaved compact draft");
  });
}
test("wide details retain the inline 340px panel without a backdrop or dialog", () => {
  compact = false;
  const tree = render(() => DetailPanel({ projectKey, entry, onRefetch() {}, onClose() {} }));
  assert.equal(tree[0].type, "aside", "wide layout retains its original root and flex participation");
  assert.ok(!tree.some(n => n.props.role === "dialog" || n.props["data-detail-backdrop"] !== undefined));
  assert.match(tree.find(n => n.type === "aside").props.className, /w-\[340px\]/);
});

test("drawer traps Tab, yields Escape to a nested modal, and preserves drafts across resize", () => {
  let closed = 0;
  const props = { projectKey, entry, onRefetch() {}, onClose() { closed++; } };
  let tree = render(() => DetailPanel(props));
  const dialog = tree.find(n => n.props.role === "dialog");
  const input = { tabIndex: 0, hasAttribute: () => false, focus() { active = input; } };
  const button = { tabIndex: 0, hasAttribute: () => false, focus() { active = button; } };
  const root = { contains: () => true, querySelector: () => input, querySelectorAll: () => [input, button] };
  tree.find(n => n.type === "textarea").props.ref.current = input;
  dialog.props.ref.current = root; dialogs = [root];
  h.effects.forEach(effect => effect());
  const key = (value: string, shiftKey = false) => {
    let prevented = false;
    keyListeners.forEach(listener => listener({ key: value, shiftKey, preventDefault() { prevented = true; }, stopImmediatePropagation() {} }));
    return prevented;
  };
  button.focus(); assert.equal(key("Tab"), true); assert.equal(active, input);
  assert.equal(key("Tab", true), true); assert.equal(active, button);
  dialogs = [root, {}];
  assert.equal(key("Escape"), false); assert.equal(closed, 0);
  dialogs = [root]; assert.equal(key("Escape"), true); assert.equal(closed, 1);
  tree.find(n => n.type === "textarea").props.onChange({ target: { value: "Resize draft" } });
  compact = false; mediaListeners.forEach(listener => listener({ matches: false }));
  tree = render(() => DetailPanel(props));
  assert.ok(!tree.some(n => n.props.role === "dialog"));
  assert.match(tree.find(n => n.type === "aside").props.className, /w-\[340px\]/);
  assert.equal(tree.find(n => n.type === "textarea").props.value, "Resize draft");
  h.effects.forEach(effect => effect()); cleanup();
});

// Disabled fields refuse focus, matching the browser behavior missed by the old fixture.
function mountDrawer(states: any[], props: Parameters<typeof DetailPanel>[0]) {
  const controls = new Map<string, any>();
  const root: any = {
    contains: (target: any) => target === root || [...controls.values()].includes(target),
    querySelector: () => [...controls.values()].find(control => !control.disabled),
    querySelectorAll: () => [...controls.values()].filter(control => !control.disabled),
    focus() { active = root; },
  };
  const update = () => {
    const tree = render(() => DetailPanel(props), states);
    for (const [index, node] of tree.filter(n => ["button", "textarea"].includes(n.type)).entries()) {
      const id = node.props.id ?? node.props["aria-label"] ?? String(index);
      let control = controls.get(id);
      if (!control) {
        control = { disabled: false, tabIndex: 0,
          hasAttribute: (name: string) => name === "disabled" && control.disabled,
          focus() {
            if (control.disabled) return;
            active = control; focusListeners.forEach(listener => listener({ target: control }));
          },
        };
        controls.set(id, control);
      }
      control.disabled = !!node.props.disabled;
      if (node.props.ref) node.props.ref.current = control;
    }
    tree.find(n => n.props.role === "dialog").props.ref.current = root;
    dialogs = [root];
    h.effects.forEach(effect => effect());
    return tree;
  };
  const tree = update();
  return { root, update, tree, controls,
    input: controls.get(`detail-${encodeURIComponent(entry.id)}-translation`),
    close: controls.get(t("detail.closeAria")),
  };
}

for (const moveFocus of [false, true]) {
  test(`pending save -> reopen row keeps ArrowDown inside drawer; save completion ${moveFocus ? "preserves chosen control" : "promotes fallback focus"}`, async () => {
    const detailStates: any[] = [], tableStates: any[] = [];
    const data = [entry, { ...entry, id: "row-2" }];
    const props = { projectKey, entry, onRefetch() {}, onClose() { useEditorStore.getState().setSelected(null); } };
    const table = render(() => StringTable({ data, onRefetch() {} }), tableStates);
    const rows = table.filter(n => n.type === "tr" && n.props.onKeyDown);
    const nativeRows = rows.map(row => ({ isConnected: true, focus() { active = row; row.props.onFocus(); } }));
    table.find(n => n.type === "tbody").props.ref.current = { rows: nativeRows };
    nativeRows[0].focus();
    rows[0].props.onClick({ currentTarget: nativeRows[0] });
    const first = mountDrawer(detailStates, props);
    first.tree.find(n => n.type === "textarea").props.onChange({ target: { value: "Pending translation" } });
    let finish!: () => void;
    const pendingWrite = new Promise<void>(resolve => { finish = resolve; });
    const saving = drafts.saveDraft(drafts.draftEntryKey(projectKey, entry.id), entry.translation, () => pendingWrite);
    first.tree.find(n => n.props["aria-label"] === t("detail.closeAria")).props.onClick();
    cleanup(detailStates); dialogs = []; nativeRows[0].focus();
    rows[0].props.onClick({ currentTarget: nativeRows[0] });
    const reopenedStates: any[] = [];
    const drawer = mountDrawer(reopenedStates, props);
    try {
      assert.equal(drawer.input.disabled, true);
      assert.ok(drawer.root.contains(active), "a disabled preferred field must not leave focus on the table row");
      assert.equal(active, drawer.close, "the first enabled drawer control receives fallback focus");
      const selection = useEditorStore.getState().selectedEntryId;
      const event = { key: "ArrowDown", target: active, currentTarget: rows[0], preventDefault() {} };
      keyListeners.forEach(listener => listener(event));
      // The browser only bubbles a row key handler when the focused target is that row.
      if (rows.includes(active)) active.props.onKeyDown(event);
      const afterArrow = render(() => StringTable({ data, onRefetch() {} }), tableStates);
      assert.equal(afterArrow.filter(n => n.type === "tr" && n.props.onKeyDown)[0].props.tabIndex, 0);
      assert.equal(useEditorStore.getState().selectedEntryId, selection);
      if (moveFocus) {
        keyListeners.forEach(listener => listener({ key: "Tab", shiftKey: true, preventDefault() {} }));
        assert.notEqual(active, drawer.close, "Shift+Tab moves to another enabled drawer control");
        assert.ok(drawer.root.contains(active));
      }
      const chosen = active;
      finish(); await saving;
      drawer.update();
      assert.equal(active, moveFocus ? chosen : drawer.input, "save completion only moves focus from the chosen fallback");
    } finally {
      finish(); await saving; cleanup(reopenedStates);
    }
  });
}

test("overlay drawer contains outside focus, yields to a nested dialog, and removes containment on close", () => {
  const states: any[] = [];
  const drawer = mountDrawer(states, { projectKey, entry, onRefetch() {}, onClose() {} });
  const outside = { focus() { active = outside; focusListeners.forEach(listener => listener({ target: outside })); } };
  outside.focus();
  assert.ok(drawer.root.contains(active), "background controls cannot take focus from the overlay drawer");
  const nested = {};
  dialogs = [drawer.root, nested];
  outside.focus();
  assert.equal(active, outside, "a nested dialog retains ownership of focus");
  cleanup(states); dialogs = [];
  assert.equal(focusListeners.size, 0);
  outside.focus(); assert.equal(active, outside);
});
