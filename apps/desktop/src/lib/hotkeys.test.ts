import assert from "node:assert/strict";
import { after, beforeEach, test } from "node:test";
import { registerHooks } from "node:module";

const h = {
  handlers: [] as ((event: any) => void)[],
  navigations: [] as string[],
};
(globalThis as any).__hotkeysTest = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/lib/hotkeys.ts")) {
      const stubs: Record<string, string> = {
        react: "export const useRef=value=>({current:value});export const useEffect=fn=>{fn();};",
        "react-router-dom": "export const useNavigate=()=>path=>h.navigations.push(path);",
        "@tanstack/react-query": "export const useQueryClient=()=>({});",
        "./openProjectFlow": "export const completeOpenProject=async()=>null;export const isDetectionFailure=()=>false;export const pickGameSource=async()=>null;",
        "./projectOpenMerge": "export const PENDING_AFTER_STALE_FILTER={};export const projectOpenMergeNotice=()=>({});export const shouldFocusPendingAfterOpen=()=>false;",
        "../stores/projectStore": "export const useProjectStore=select=>select({setProject(){}});",
        "../stores/editorStore": "export const useEditorStore=select=>select({setFilter(){}});",
        "../stores/logStore": "export const addLog=()=>{};",
        "../stores/toastStore": "export const addToast=()=>{};",
      };
      const source = stubs[specifier];
      if (source) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__hotkeysTest;" + source)}` };
    }
    return nextResolve(specifier, context);
  },
});
const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
const previousDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
Object.defineProperty(globalThis, "window", {
  configurable: true,
  value: { addEventListener(_type: string, handler: (event: any) => void) { h.handlers.push(handler); }, removeEventListener() {} },
});
Object.defineProperty(globalThis, "document", { configurable: true, value: { querySelector: () => null } });
const { HOTKEY_MAP, formatKey, getGroupedHotkeys, matchesEvent, useGlobalHotkeys } = await import("./hotkeys");
after(() => {
  hooks.deregister();
  if (previousWindow) Object.defineProperty(globalThis, "window", previousWindow);
  else Reflect.deleteProperty(globalThis, "window");
  if (previousDocument) Object.defineProperty(globalThis, "document", previousDocument);
  else Reflect.deleteProperty(globalThis, "document");
  Reflect.deleteProperty(globalThis, "__hotkeysTest");
});
beforeEach(() => { h.handlers = []; h.navigations = []; });

function keyEvent(key: string, modifiers: { ctrlKey?: boolean; shiftKey?: boolean; altKey?: boolean; metaKey?: boolean } = {}) {
  return {
    key, ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, ...modifiers,
    target: null, preventDefault() {}, stopImmediatePropagation() {},
  } as unknown as KeyboardEvent;
}

// Sidebar order in components/Layout.tsx: Alt+N opens the Nth item.
const SIDEBAR = [
  ["navHome", "/"],
  ["navDashboard", "/dashboard"],
  ["navEditor", "/editor"],
  ["navReview", "/review"],
  ["navQa", "/qa"],
  ["navSpeakers", "/speakers"],
  ["navMemory", "/memory"],
  ["navSettings", "/settings"],
] as const;

test("? opens Shortcuts when the layout needs Shift to type it, and without Shift", () => {
  assert.equal(matchesEvent(keyEvent("?", { shiftKey: true }), HOTKEY_MAP.showHelp), true);
  assert.equal(matchesEvent(keyEvent("?"), HOTKEY_MAP.showHelp), true);
  assert.equal(matchesEvent(keyEvent("/", { shiftKey: true }), HOTKEY_MAP.showHelp), true);
  assert.equal(matchesEvent(keyEvent("?", { shiftKey: true, ctrlKey: true }), HOTKEY_MAP.showHelp), false);
  assert.equal(matchesEvent(keyEvent("?", { shiftKey: true, altKey: true }), HOTKEY_MAP.showHelp), false);
  assert.equal(matchesEvent(keyEvent("/"), HOTKEY_MAP.showHelp), false);
});

test("Shift still distinguishes the other bindings", () => {
  assert.equal(matchesEvent(keyEvent("f", { ctrlKey: true }), HOTKEY_MAP.search), true);
  assert.equal(matchesEvent(keyEvent("F", { ctrlKey: true, shiftKey: true }), HOTKEY_MAP.search), false);
  assert.equal(matchesEvent(keyEvent("F", { ctrlKey: true, shiftKey: true }), HOTKEY_MAP.searchReplace), true);
});

test("Alt+1..Alt+8 open the sidebar items in sidebar order, including QA and Speakers", () => {
  useGlobalHotkeys(() => {});
  SIDEBAR.forEach(([action], index) => {
    assert.equal(formatKey(HOTKEY_MAP[action]), `Alt+${index + 1}`, action);
  });
  for (let digit = 1; digit <= SIDEBAR.length; digit++) {
    for (const handler of h.handlers) handler(keyEvent(String(digit), { altKey: true }));
  }
  assert.deepEqual(h.navigations, SIDEBAR.map(([, to]) => to));
});

test("help lists the Alt shortcuts in sidebar order and the Review keys", () => {
  const groups = getGroupedHotkeys();
  const alt = groups.Navigation.filter(({ binding }) => binding.alt).map(({ action }) => action);
  assert.deepEqual(alt, SIDEBAR.map(([action]) => action));
  assert.deepEqual(
    groups.Review.map(({ action, binding }) => [action, formatKey(binding)]),
    [
      ["reviewApprove", "A"],
      ["reviewApproveCtrl", "Ctrl+Enter"],
      ["reviewSkip", "S"],
      ["reviewPrevious", "P"],
      ["reviewEdit", "E"],
      ["reviewExit", "Esc"],
    ],
  );
});
