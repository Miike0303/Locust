import assert from "node:assert/strict";
import { after, test } from "node:test";
import { registerHooks } from "node:module";
import { renderToStaticMarkup } from "react-dom/server";
import { setLocale, t } from "../lib/i18n/index.ts";
import * as a11y from "../lib/modalA11y.ts";

// As in ResumeProjectDialog.test.tsx: exercise real element callbacks and SSR,
// replacing only React's lifecycle hooks and non-DOM side-effect sinks.
const h = {
  t, a11y, values: [] as unknown[], cursor: 0,
  toasts: [] as unknown[][], logs: [] as unknown[][],
  changed: 0,
  jobHandlers: null as null | { onDone(event: any): void; onError(error: Error): void },
  state(initial: unknown) {
    const index = this.cursor++;
    if (!(index in this.values)) this.values[index] = typeof initial === "function" ? initial() : initial;
    return [this.values[index], (value: any) => {
      this.values[index] = typeof value === "function" ? value(this.values[index]) : value;
    }];
  },
  ref(initial: unknown) {
    const index = this.cursor++;
    if (!(index in this.values)) this.values[index] = { current: initial };
    return this.values[index];
  },
};
(globalThis as any).__patchModal = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/components/PatchModal.tsx")) {
      const stubs: Record<string, string> = {
        react: `const h=globalThis.__patchModal;export const useState=initial=>h.state(initial);export const useRef=initial=>h.ref(initial);export const useEffect=()=>{};`,
        "../lib/i18n": `export const useT=()=>globalThis.__patchModal.t;`,
        "../stores/toastStore": `export const addToast=(...args)=>globalThis.__patchModal.toasts.push(args);`,
        "../stores/logStore": `export const addLog=(...args)=>globalThis.__patchModal.logs.push(args);`,
        "../lib/ws": `export const subscribeToJob=(_id,handlers)=>{globalThis.__patchModal.jobHandlers=handlers;return ()=>{};};`,
        "../lib/modalA11y": `const h=globalThis.__patchModal;
          export const MODAL_BACKDROP_CLASS=h.a11y.MODAL_BACKDROP_CLASS;
          export const modalPanelClass=h.a11y.modalPanelClass;
          export const useModalA11y=()=>({dialogRef:{current:null},dialogProps:h.a11y.buildModalDialogProps({titleId:'title'}),titleProps:h.a11y.buildModalTitleProps('title')});`,
      };
      if (stubs[specifier]) return { url: `data:text/javascript,${encodeURIComponent(stubs[specifier])}`, shortCircuit: true };
    }
    return nextResolve(specifier, context);
  },
});
const calls: { url: string; body: any }[] = [];
let failed = false;
let deferred: (() => void) | null = null;
let deferRollback = false;
let status = "interrupted";
let report = {
  restored: 2, deleted: 1, baseline: "Pristine", dry_run: true,
  messages: ["preview <message>"], aborted_edited: ["data/user<edit>.txt"], torn_deleted: [] as string[],
};
const originalFetch = globalThis.fetch;
const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
Object.defineProperty(globalThis, "window", { configurable: true, value: {
  __TAURI_INTERNALS__: { invoke: async (command: string) => {
    assert.equal(command, "get_server_port");
    return 7842;
  } },
} });
globalThis.fetch = async (url, init) => {
  const path = String(url);
  const body = init?.body ? JSON.parse(String(init.body)) : null;
  calls.push({ url: path, body });
  if (path.endsWith("/patch/rollback")) {
    if (deferRollback) await new Promise<void>(resolve => { deferred = resolve; });
    return new Response(failed ? "cannot preview rollback" : JSON.stringify({ ...report, dry_run: body.dry_run ?? false }), { status: failed ? 500 : 200 });
  }
  if (path.endsWith("/patch/apply")) return new Response(JSON.stringify({ job_id: "test-job" }));
  if (path.endsWith("/patch/verify")) return new Response(JSON.stringify({
    outcome: "Clean", tier: "Strict", replaced: ["data/original.txt"], added: [], conflicts: [],
    messages: ["verification retained"], backup_compromised: false, manifest: {},
  }));
  return new Response(JSON.stringify({ status, patch_id: "test-patch" }));
};
const { default: PatchModal } = await import("./PatchModal.tsx");
after(() => {
  hooks.deregister();
  globalThis.fetch = originalFetch;
  if (windowDescriptor) Object.defineProperty(globalThis, "window", windowDescriptor);
  else Reflect.deleteProperty(globalThis, "window");
  Reflect.deleteProperty(globalThis, "__patchModal");
  setLocale("en");
});
function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(nodes);
  return [node, ...nodes(node.props?.children)];
}
function text(node: any): string {
  if (typeof node === "string") return node;
  if (Array.isArray(node)) return node.map(text).join("");
  return node && typeof node === "object" ? text(node.props?.children) : "";
}
function render() {
  h.cursor = 0;
  return PatchModal({ open: true, onClose() {}, defaultGamePath: " /game ", allowPack: false,
    onPatchStateChanged() { h.changed++; } });
}
function button(label: string, index = 0) {
  const found = nodes(render()).filter(node => node.type === "button" && text(node).trim() === label)[index];
  assert.ok(found, `missing action: ${label} (${index})`);
  return found;
}
function reset(locale: "en" | "es") {
  setLocale(locale);
  h.values = []; h.changed = 0; h.toasts = []; h.logs = [];
  calls.length = 0; failed = false; deferRollback = false; deferred = null;
  status = "interrupted"; h.jobHandlers = null;
  report = { restored: 2, deleted: 1, baseline: "Pristine", dry_run: true,
    messages: ["preview <message>"], aborted_edited: ["data/user<edit>.txt"], torn_deleted: [] };
  render();
}
async function verifyAndInterrupt() {
  const input = nodes(render()).find(node => node.type === "input" && node.props.placeholder === t("patch.zipPlaceholder"));
  input.props.onChange({ target: { value: "/patch.zip" } });
  await button(t("patch.verify")).props.onClick();
  assert.ok(renderToStaticMarkup(render()).includes("verification retained"));
  calls.length = 0; h.toasts = []; h.logs = []; h.changed = 0;
}
for (const locale of ["en", "es"] as const) {
  const previewLabel = locale === "en" ? "Preview rollback" : "Previsualizar reversión";
  test(`${locale} preview sends dry_run and renders counts, messages and edited files without rollback side effects`, async () => {
    reset(locale);
    await verifyAndInterrupt();
    assert.equal(nodes(render()).filter(node => node.type === "button" && text(node).trim() === previewLabel).length, 2);
    await button(previewLabel).props.onClick();
    assert.deepEqual(calls, [{ url: "http://localhost:7842/api/patch/rollback", body: { game_path: "/game", force: false, dry_run: true } }]);
    const html = renderToStaticMarkup(render());
    assert.ok(html.includes(locale === "en" ? "Rollback preview" : "Vista previa de la reversión"), html);
    assert.ok(html.includes(locale === "en" ? "Would restore: 2. Would delete: 1." : "Se restaurarían: 2. Se eliminarían: 1."), html);
    assert.ok(html.includes("preview &lt;message&gt;") && html.includes("data/user&lt;edit&gt;.txt"), html);
    assert.ok(html.includes("verification retained") && html.includes(t("patch.partialWarning")), "preview must preserve verification and interrupted state");
    assert.equal(h.changed, 0);
    assert.deepEqual(h.toasts, [], "preview must not emit real-rollback success/error toasts");
    assert.ok(!h.logs.some(log => log[1] === t("activity.patch.rollback")), "preview must not log a completed rollback");
  });

  test(`${locale} interrupted recovery preview uses the same read-only payload and busy guard`, async () => {
    reset(locale);
    await button(t("patch.refreshStatus")).props.onClick();
    calls.length = 0; h.changed = 0;
    deferRollback = true;
    const pending = button(previewLabel, 1).props.onClick();
    for (let i = 0; !deferred && i < 20; i++) await new Promise(resolve => setTimeout(resolve, 0));
    assert.ok(deferred);
    assert.equal(button(previewLabel).props.disabled, true);
    assert.equal(button(previewLabel, 1).props.disabled, true);
    assert.equal(button(t("patch.rollback")).props.disabled, true);
    deferred!();
    await pending;
    assert.deepEqual(calls[0].body, { game_path: "/game", force: false, dry_run: true });
    assert.equal(h.changed, 0);
    assert.ok(renderToStaticMarkup(render()).includes(t("patch.partialWarning")));
  });

  test(`${locale} preview errors are visible without clearing verification or refreshing status`, async () => {
    reset(locale);
    await verifyAndInterrupt();
    failed = true;
    await button(previewLabel).props.onClick();
    const html = renderToStaticMarkup(render());
    assert.ok(html.includes("cannot preview rollback"), html);
    assert.ok(html.includes("verification retained") && html.includes(t("patch.partialWarning")), html);
    assert.equal(calls.length, 1);
    assert.equal(h.changed, 0);
    assert.ok(!h.toasts.some(toast => toast[0] === "success"));
  });
}

test("forced preview lists edited deletions and hides the old plan after changing its game/force scope", async () => {
  reset("en");
  const force = nodes(render()).find(node => node.type === "input" && node.props.type === "checkbox");
  force.props.onChange({ target: { checked: true } });
  report.aborted_edited = [];
  report.torn_deleted = ["data/forced.txt"];
  await button("Preview rollback").props.onClick();
  assert.deepEqual(calls[0].body, { game_path: "/game", force: true, dry_run: true });
  assert.ok(renderToStaticMarkup(render()).includes("data/forced.txt"));
  force.props.onChange({ target: { checked: false } });
  assert.ok(!renderToStaticMarkup(render()).includes("data/forced.txt"), "a changed Force option invalidates the displayed preview");
  force.props.onChange({ target: { checked: true } });
  const game = nodes(render()).find(node => node.type === "input" && node.props.value === " /game ");
  game.props.onChange({ target: { value: "/other-game" } });
  assert.ok(!renderToStaticMarkup(render()).includes("data/forced.txt"), "a plan must not be shown for another game");
});

test("preview preserves a completed apply result alongside verification", async () => {
  reset("en");
  await verifyAndInterrupt();
  await button(t("patch.applyBtn")).props.onClick();
  assert.ok(h.jobHandlers);
  h.jobHandlers.onDone({ report: {
    patch_id: "apply-result", patch_version: "1", replaced: 2, added: 1,
    baseline: "Pristine", dry_run: false, forced: false, user_edits_overwritten: [], messages: [],
  } });
  for (let i = 0; button(t("patch.applyBtn")).props.disabled && i < 20; i++) {
    await new Promise(resolve => setTimeout(resolve, 0));
  }
  assert.equal(button(t("patch.applyBtn")).props.disabled, false);
  const appliedLabel = t("patch.applied", { id: "apply-result", version: "1" });
  assert.ok(renderToStaticMarkup(render()).includes(appliedLabel));
  calls.length = 0; h.toasts = []; h.changed = 0;
  await button("Preview rollback").props.onClick();
  const html = renderToStaticMarkup(render());
  assert.ok(html.includes(appliedLabel) && html.includes("verification retained"), html);
  assert.equal(calls.length, 1);
  assert.equal(h.changed, 0);
  assert.deepEqual(h.toasts, []);
});

test("a successful preview does not clear the modal's recovery warning after an apply error", async () => {
  reset("en");
  await verifyAndInterrupt();
  await button(t("patch.applyBtn")).props.onClick();
  assert.ok(h.jobHandlers);
  status = "not_patched"; // Warning must be driven by needsRollback, not status.
  h.jobHandlers.onError(new Error("partial apply"));
  for (let i = 0; button(t("patch.applyBtn")).props.disabled && i < 20; i++) {
    await new Promise(resolve => setTimeout(resolve, 0));
  }
  assert.equal(button(t("patch.applyBtn")).props.disabled, false);
  assert.ok(renderToStaticMarkup(render()).includes(t("patch.partialWarning")));
  calls.length = 0; h.toasts = []; h.changed = 0;
  report.aborted_edited = [];
  await button("Preview rollback", 1).props.onClick();
  assert.ok(renderToStaticMarkup(render()).includes(t("patch.partialWarning")));
  assert.equal(calls.length, 1);
  assert.equal(h.changed, 0);
  assert.deepEqual(h.toasts, []);
});

for (const recovery of [false, true]) test(`${recovery ? "recovery" : "regular"} real rollback still omits dry_run and resets verification, refreshes status and reports success`, async () => {
  reset("en");
  await verifyAndInterrupt();
  report.aborted_edited = [];
  await button(t(recovery ? "patch.rollbackNow" : "patch.rollback")).props.onClick();
  assert.deepEqual(calls, [
    { url: "http://localhost:7842/api/patch/rollback", body: { game_path: "/game", force: false } },
    { url: "http://localhost:7842/api/patch/status", body: { game_path: "/game" } },
  ]);
  assert.equal(h.changed, 1);
  assert.ok(h.toasts.some(toast => toast[0] === "success" && toast[1] === t("patch.toast.rollback", { restored: 2, deleted: 1 })));
  assert.ok(!renderToStaticMarkup(render()).includes("verification retained"));
});
