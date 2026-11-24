import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import { setLocale, t } from "../lib/i18n";
import * as a11y from "../lib/modalA11y";
import type { LogEntry } from "../stores/logStore";

const entries: LogEntry[] = [
  { id: "with-detail", timestamp: Date.now(), level: "error", message: "Patch failed", detail: "raw backend detail", source: "patch" },
  { id: "plain", timestamp: Date.now(), level: "info", message: "Project opened" },
];
const h = {
  t, a11y, states: [] as any[], cursor: 0,
  options: null as null | { open: boolean; ownEscape?: boolean; onClose?: () => void; initialFocusRef?: { current: unknown } },
  store: {
    entries, filter: "all" as const, isOpen: true, cleared: 0, closed: 0,
    setOpen(open: boolean) { if (!open) h.store.closed++; h.store.isOpen = open; },
    setFilter() {},
    clear() { h.store.cleared++; },
  },
  state(initial: unknown) {
    const index = this.cursor++;
    if (!(index in this.states)) this.states[index] = typeof initial === "function" ? initial() : initial;
    return [this.states[index], (value: any) => {
      this.states[index] = typeof value === "function" ? value(this.states[index]) : value;
    }];
  },
  ref(initial: unknown) {
    const index = this.cursor++;
    return this.states[index] ??= { current: initial };
  },
};
(globalThis as any).__activityLog = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/components/ActivityLog.tsx")) {
      const stubs: Record<string, string> = {
        react: "export const useState=initial=>h.state(initial);export const useRef=initial=>h.ref(initial);",
        "../lib/i18n": "export const useT=()=>h.t;",
        "../stores/logStore": "export const useLogStore=()=>h.store;",
        "../lib/modalA11y": "export const useModalA11y=options=>{h.options=options;return {dialogRef:{current:null},dialogProps:h.a11y.buildModalDialogProps({titleId:'log-title'}),titleProps:h.a11y.buildModalTitleProps('log-title')};};",
      };
      if (stubs[specifier]) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__activityLog;" + stubs[specifier])}` };
    }
    return nextResolve(specifier, context);
  },
});
const { default: ActivityLog } = await import("./ActivityLog");
const { default: ConfirmDialog } = await import("./ConfirmDialog");
after(() => { hooks.deregister(); Reflect.deleteProperty(globalThis, "__activityLog"); setLocale("en"); });
beforeEach(() => {
  h.states = []; h.options = null;
  Object.assign(h.store, { entries, isOpen: true, cleared: 0, closed: 0 });
});

function render() {
  h.cursor = 0;
  return ActivityLog();
}
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
const rowButton = (message: string) => nodes(render()).find(node => node.type === "button" && text(node).includes(message));
const clearConfirm = () => {
  const dialog = nodes(render()).find(node => node.type === ConfirmDialog);
  assert.ok(dialog, "clearing the log has a confirmation");
  return dialog;
};

for (const locale of ["en", "es"] as const) {
  test(`${locale}: the open log is a labelled dialog that closes on Escape and focuses its close button`, () => {
    setLocale(locale);
    const root = render();
    assert.ok(root);
    assert.equal(root.props.role, "dialog");
    assert.equal(root.props["aria-labelledby"], "log-title");
    const title = nodes(root).find(node => node.props?.id === "log-title");
    assert.equal(text(title), t("log.title"));
    assert.equal(h.options?.open, true);
    assert.equal(h.options?.ownEscape, true, "Escape closes the log");
    h.options?.onClose?.();
    assert.equal(h.store.closed, 1);
    assert.equal(render(), null, "Escape closes the panel");
    h.store.isOpen = true;
    const close = nodes(render()).find(node => node.type === "button" && node.props["aria-label"] === t("log.closeAria"));
    assert.ok(close);
    assert.ok(close.props.ref);
    assert.equal(h.options?.initialFocusRef, close.props.ref, "focus starts on Close, not on the destructive Clear");
  });

  test(`${locale}: entries with details are buttons that expand by keyboard; plain entries are not clickable`, () => {
    setLocale(locale);
    assert.ok(!nodes(render()).some(node => node.type === "div" && node.props.onClick), "no clickable div rows");
    const toggle = rowButton("Patch failed");
    assert.ok(toggle, "an entry with details is a real button");
    assert.equal(toggle.props.type, "button");
    assert.equal(toggle.props["aria-expanded"], false);
    assert.ok(!nodes(render()).some(node => node.type === "pre"));
    toggle.props.onClick();
    const expanded = rowButton("Patch failed");
    assert.equal(expanded.props["aria-expanded"], true);
    const detail = nodes(render()).find(node => node.type === "pre");
    assert.equal(text(detail), "raw backend detail");
    assert.equal(detail.props.id, expanded.props["aria-controls"]);
    assert.equal(rowButton("Project opened"), undefined, "an entry without details has nothing to expand");
    assert.ok(text(render()).includes("Project opened"));
  });

  test(`${locale}: clearing the log needs a destructive confirmation`, () => {
    setLocale(locale);
    assert.equal(clearConfirm().props.open, false);
    const clear = () => nodes(render()).find(node => node.type === "button" && node.props["aria-label"] === t("log.clearAria"));
    clear().props.onClick();
    let dialog = clearConfirm();
    assert.equal(dialog.props.open, true);
    assert.equal(dialog.props.destructive, true);
    assert.equal(dialog.props.title, t("log.clearConfirm.title"));
    assert.equal(dialog.props.message, t("log.clearConfirm.message", { count: 2 }));
    dialog.props.onCancel();
    assert.equal(clearConfirm().props.open, false);
    assert.equal(h.store.cleared, 0, "cancel keeps the entries");
    clear().props.onClick();
    dialog = clearConfirm();
    dialog.props.onConfirm();
    assert.equal(h.store.cleared, 1);
    assert.equal(clearConfirm().props.open, false);
  });
}

test("a closed log renders nothing but keeps its dialog hooks in order", () => {
  h.store.isOpen = false;
  assert.equal(render(), null);
  assert.equal(h.options?.open, false);
});

test("Editor's Escape hotkey pauses while the Activity Log is open so Escape reaches the log", () => {
  const source = readFileSync(new URL("../pages/Editor.tsx", import.meta.url), "utf8");
  const closePanel = source.match(/useHotkey\(\s*"closePanel",([\s\S]*?)\n\t\);/);
  assert.ok(closePanel, "Editor registers its Escape hotkey");
  assert.match(closePanel[1], /!activityLogOpen/);
  assert.match(source, /useLogStore\.subscribe\(/, "Editor follows the log's open state");
});
