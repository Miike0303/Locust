import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, afterEach, beforeEach, test } from "node:test";
import { addToast, useToastStore as store } from "../stores/toastStore";
import { setLocale, t } from "../lib/i18n";

// Render authored JSX and invoke its actual handlers; keep timers and store real.
(globalThis as any).__toastHarness = { store, t };
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/ToastContainer.tsx")) {
      const stubs: Record<string, string> = {
        "../stores/toastStore": "export const useToastStore=()=>h.store.getState();",
        "../lib/i18n": "export const useT=()=>h.t;",
      };
      if (stubs[specifier]) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__toastHarness;" + stubs[specifier])}` };
    }
    return nextResolve(specifier, context);
  },
});
const { default: ToastContainer } = await import("./ToastContainer");
function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(nodes);
  return [node, ...nodes(node.props?.children)];
}
const render = () => nodes(ToastContainer());
const toastNode = () => render().find(node => node.props.role === "status" || node.props.role === "alert");
beforeEach((ctx) => {
  assert.ok("mock" in ctx, "toast fixtures run in a test context");
  ctx.mock.timers.enable({ apis: ["setTimeout", "Date"], now: 1000 });
});
afterEach(() => {
  for (const toast of store.getState().toasts) store.getState().dismiss(toast.id);
  setLocale("en");
});
after(() => { hooks.deregister(); Reflect.deleteProperty(globalThis, "__toastHarness"); });

for (const locale of ["en", "es"] as const) {
  test(`${locale}: errors alert; other severities announce politely and dismiss accessibly`, () => {
    setLocale(locale);
    assert.equal(ToastContainer(), null);
    for (const severity of ["success", "info", "warning", "error"] as const) {
      addToast(severity, severity);
      const tree = render();
      assert.equal(tree.find(node => node.props.role === "region").props["aria-label"], t("toast.notifications"));
      const toast = toastNode();
      assert.equal(toast.props.role, severity === "error" ? "alert" : "status");
      assert.equal(toast.props["aria-live"], severity === "error" ? "assertive" : "polite");
      assert.equal(toast.props["aria-atomic"], true);
      const dismiss = tree.find(node => node.type === "button");
      assert.equal(dismiss.props.type, "button");
      assert.equal(dismiss.props["aria-label"], t("toast.dismiss"));
      assert.match(dismiss.props.className, /focus:ring-2/);
      dismiss.props.onClick();
      assert.equal(ToastContainer(), null);
    }
  });
}
test("long unbroken text is wrapped inside a viewport-bounded scrollable stack without slide-in", () => {
  const message = "X".repeat(5000);
  addToast("error", message);
  const tree = render();
  const container = tree.find(node => node.props.role === "region");
  assert.match(container.props.className, /w-\[calc\(100%-2rem\)\]/);
  assert.match(container.props.className, /max-h-\[calc\(100dvh-2rem\)\]/);
  assert.match(container.props.className, /overflow-y-auto/);
  const toast = toastNode();
  assert.match(toast.props.className, /min-w-0/);
  assert.doesNotMatch(toast.props.className, /slideIn/);
  const text = tree.find(node => node.type === "span");
  assert.equal(text.props.children, message);
  assert.match(text.props.className, /min-w-0/);
  assert.match(text.props.className, /\[overflow-wrap:anywhere\]/);
});
for (const first of ["pointer", "focus"] as const) {
  test(`${first} plus overlapping pause reason resumes only the remaining duration`, (ctx) => {
    addToast("warning", "timed", 4000);
    const toast = toastNode();
    const child = {};
    const currentTarget = { contains: (target: unknown) => target === child };
    ctx.mock.timers.tick(1000);
    if (first === "pointer") toast.props.onPointerEnter();
    else toast.props.onFocus();
    ctx.mock.timers.tick(500);
    toast.props.onPointerEnter();
    toast.props.onFocus();
    toast.props.onBlur({ currentTarget, relatedTarget: child });
    ctx.mock.timers.tick(5000);
    if (first === "pointer") toast.props.onPointerLeave();
    else toast.props.onBlur({ currentTarget, relatedTarget: null });
    ctx.mock.timers.tick(5000);
    assert.equal(store.getState().toasts.length, 1, "other pause reason still holds");
    toast.props.onPointerLeave();
    toast.props.onBlur({ currentTarget, relatedTarget: null });
    ctx.mock.timers.tick(2999);
    assert.equal(store.getState().toasts.length, 1);
    ctx.mock.timers.tick(1);
    assert.equal(ToastContainer(), null);
  });
}
for (const severity of ["success", "info", "error"] as const) {
  test(`${severity}: keyboard focus alone holds an explicitly timed message`, (ctx) => {
    addToast(severity, "keyboard", 2000);
    const toast = toastNode();
    ctx.mock.timers.tick(500);
    toast.props.onFocus();
    ctx.mock.timers.tick(5000);
    assert.equal(store.getState().toasts.length, 1);
    toast.props.onBlur({ currentTarget: { contains: () => false }, relatedTarget: null });
    ctx.mock.timers.tick(1499);
    assert.equal(store.getState().toasts.length, 1);
    ctx.mock.timers.tick(1);
    assert.equal(ToastContainer(), null);
  });
}
test("dismiss while paused cleans state and stale mounted handlers cannot restart the timer", (ctx) => {
  addToast("info", "timed", 1000);
  const toast = toastNode();
  toast.props.onPointerEnter();
  render().find(node => node.type === "button").props.onClick();
  toast.props.onPointerLeave();
  ctx.mock.timers.tick(2000);
  assert.equal(ToastContainer(), null);
});
