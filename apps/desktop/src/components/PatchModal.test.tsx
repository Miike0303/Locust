import assert from "node:assert/strict";
import PatchApplyTab, { PatchApplyActions } from "./patch/PatchApplyTab";
import PatchPackTab, { PatchPackActions } from "./patch/PatchPackTab";
import { after, test } from "node:test";
import { registerHooks } from "node:module";
import { renderToStaticMarkup } from "react-dom/server";
import { setLocale, t } from "../lib/i18n/index.ts";
import * as a11y from "../lib/modalA11y.ts";
import type { PatchVerifyResult } from "../lib/api.ts";

// As in ResumeProjectDialog.test.tsx: exercise real element callbacks and SSR,
// replacing only React's lifecycle hooks and non-DOM side-effect sinks.
const h = {
  t, a11y, values: [] as unknown[], cursor: 0,
  toasts: [] as unknown[][], logs: [] as unknown[][], apiLogs: [] as unknown[][],
  changed: 0,
  runEffects: false, effectCursor: 0, effectDeps: [] as unknown[][], effects: [] as (() => void)[],
  pickerCalls: [] as any[], selectedDirectory: null as string | null,
  effect(fn: () => void, deps: unknown[]) {
    if (!this.runEffects) return;
    const index = this.effectCursor++;
    const previous = this.effectDeps[index];
    if (!previous || deps.some((dep, i) => !Object.is(dep, previous[i]))) {
      this.effectDeps[index] = deps;
      this.effects.push(fn);
    }
  },
  jobHandlers: null as null | { onDone(event: any): void; onError(error: { message: string }): void; onClosed(): void },
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
    if (specifier === "@tauri-apps/plugin-dialog") return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("export async function open(options) { const h=globalThis.__patchModal; h.pickerCalls.push(options); return h.selectedDirectory; }")}` };
    if (context.parentURL?.endsWith("/lib/api.ts") && specifier === "../stores/logStore") {
      return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("export const addLog=(...args)=>globalThis.__patchModal.apiLogs.push(args);")}` };
    }
    if (context.parentURL?.endsWith("/components/PatchModal.tsx")) {
      const stubs: Record<string, string> = {
        react: `const h=globalThis.__patchModal;export const useState=initial=>h.state(initial);export const useRef=initial=>h.ref(initial);export const useEffect=(fn,deps)=>h.effect(fn,deps);`,
        "../lib/i18n": `export const useT=()=>globalThis.__patchModal.t;`,
        "../stores/toastStore": `export const addToast=(...args)=>globalThis.__patchModal.toasts.push(args);`,
        "../stores/logStore": `export const addLog=(...args)=>globalThis.__patchModal.logs.push(args);`,
        "../lib/ws": `export const subscribeToJob=(_id,handlers)=>{globalThis.__patchModal.jobHandlers=handlers;return ()=>{};};`,
        "../lib/modalA11y": `const h=globalThis.__patchModal;
          export const MODAL_BACKDROP_CLASS=h.a11y.MODAL_BACKDROP_CLASS;
          export const MODAL_HEADER_CLASS=h.a11y.MODAL_HEADER_CLASS;
          export const MODAL_BODY_CLASS=h.a11y.MODAL_BODY_CLASS;
          export const MODAL_FOOTER_CLASS=h.a11y.MODAL_FOOTER_CLASS;
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
let statusFailed = false;
let applyStartFailed = false;
let deferStatus = false;
let deferredStatus: (() => void) | null = null;
let recordingsEmpty = false;
let statusConflict = false;
let recordedRoot = "/copies/game.pck-es";
let verification: Partial<PatchVerifyResult> = {};
let report = {
  restored: 2, deleted: 1, baseline: "Pristine", dry_run: true,
  messages: ["preview <message>"], aborted_edited: ["data/user<edit>.txt"], torn_deleted: [] as string[],
};
const originalFetch = globalThis.fetch;
const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
Object.defineProperty(globalThis, "window", { configurable: true, value: {
  __TAURI_INTERNALS__: { invoke: async (command: string) => {
    if (command === "get_config") return { default_target_lang: "es" };
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
  if (path.endsWith("/patch/recordings")) return new Response(JSON.stringify(recordingsEmpty ? { languages: [], pristine_languages: [], roots: {} } : { languages: ["es"], pristine_languages: ["es"], roots: { es: recordedRoot } }));
  if (path.endsWith("/config")) return new Response(JSON.stringify({ default_target_lang: "es" }));
  if (path.endsWith("/patch/pack")) return new Response(JSON.stringify({ patch_id: "godot", patch_version: "1", files_packed: 1, size_bytes: 200, tier: "Strict" }));
  if (path.endsWith("/patch/apply")) return new Response(applyStartFailed ? "pre-write refusal" : JSON.stringify({ job_id: "test-job" }), { status: applyStartFailed ? 400 : 200 });
  if (path.endsWith("/patch/status")) {
    if (statusConflict) return new Response("patch backup incomplete: invalid backup manifest", { status: 409 });
    const response = new Response(statusFailed ? "status unavailable" : JSON.stringify({ status, patch_id: "test-patch" }), { status: statusFailed ? 500 : 200 });
    if (deferStatus) await new Promise<void>(resolve => { deferredStatus = resolve; });
    return response;
  }
  if (path.endsWith("/patch/verify")) return new Response(JSON.stringify({
    outcome: "Clean", tier: "Strict", replaced: ["data/original.txt"], added: [], conflicts: [],
    messages: ["verification retained"], backup_compromised: false, manifest: {}, ...verification,
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
  if (node.type === PatchApplyTab || node.type === PatchPackTab || node.type === PatchApplyActions || node.type === PatchPackActions) return nodes(node.type(node.props));
  return [node, ...nodes(node.props?.children)];
}
function text(node: any): string {
  if (typeof node === "string") return node;
  if (Array.isArray(node)) return node.map(text).join("");
  return node && typeof node === "object" ? text(node.props?.children) : "";
}
let renderOptions: Partial<Parameters<typeof PatchModal>[0]> = {};
function render() {
  h.cursor = 0; h.effectCursor = 0;
  return PatchModal({ open: true, onClose() {}, defaultGamePath: " /game ", allowPack: false,
    onPatchStateChanged() { h.changed++; }, ...renderOptions });
}
function button(label: string, index = 0) {
  const found = nodes(render()).filter(node => node.type === "button" && text(node).trim() === label)[index];
  assert.ok(found, `missing action: ${label} (${index})`);
  return found;
}
function reset(locale: "en" | "es") {
  setLocale(locale);
  h.values = []; h.changed = 0; h.toasts = []; h.logs = []; h.apiLogs = [];
  recordingsEmpty = false; statusConflict = false;
  calls.length = 0; failed = false; deferRollback = false; deferred = null;
  status = "interrupted"; statusFailed = false; applyStartFailed = false;
  verification = {};
  deferStatus = false; deferredStatus = null; h.jobHandlers = null;
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
  status = "interrupted"; // Recovery follows the refreshed backend journal.
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

function selectSource() {
  nodes(render()).find(node => node.type === "input" && node.props.placeholder === t("patch.zipPlaceholder"))
    .props.onChange({ target: { value: "/patch.zip" } });
}

for (const locale of ["en", "es"] as const) {
  test(`${locale} verification conflicts produce a localized warning, escaped files and collapsed diagnostics`, async () => {
    reset(locale); selectSource(); status = "not_patched";
    const path = "data/<script>alert(1)</script>.txt";
    verification = {
      outcome: `Mismatch([FileMismatch { path: ${JSON.stringify(path)}, expected: "present", found: "absent" }])`,
      conflicts: [path, path], messages: ["raw <diagnostic>"],
    };
    await button(t("patch.verify")).props.onClick();
    const summary = locale === "en" ? "Verification found 1 conflicting file."
      : "La verificación encontró 1 archivo en conflicto.";
    assert.deepEqual(h.toasts, [["warning", summary]]);
    assert.equal(h.logs[0][0], "warning");
    assert.equal(h.logs[0][1], summary);
    assert.ok(String(h.logs[0][2]).includes(verification.outcome!));
    const tree = render();
    const details = nodes(tree).find(node => node.type === "details" && text(node).includes("raw <diagnostic>"));
    assert.ok(details, "raw messages must be confined to technical details");
    assert.ok(!details.props.open, "technical details must start collapsed");
    assert.equal(text(nodes(details).find(node => node.type === "summary")), locale === "en" ? "Technical details" : "Detalles técnicos");
    assert.ok(text(details).includes(verification.outcome!));
    const visible = nodes(tree).filter(node => node.type === "li");
    assert.equal(visible.filter(node => text(node).includes(path)).length, 1);
    assert.ok(text(visible).includes(locale === "en" ? "Required file is missing" : "Falta un archivo necesario"));
    const html = renderToStaticMarkup(tree);
    assert.ok(html.includes("data/&lt;script&gt;alert(1)&lt;/script&gt;.txt"));
    assert.ok(!html.includes("<script>"));
    assert.ok(html.includes(summary));
    const panel = nodes(tree).find(node => node.props?.className?.includes("border-warning") && text(node).includes(summary));
    assert.ok(panel, "warning results need warning styling");
  });

  test(`${locale} not_patched status renders its localized label`, async () => {
    reset(locale); status = "not_patched";
    await button(t("patch.refreshStatus")).props.onClick();
    const html = renderToStaticMarkup(render());
    const label = locale === "en" ? "No patch applied" : "Sin parche aplicado";
    assert.ok(html.includes(t("patch.status", { status: label })));
    assert.ok(!html.includes("not_patched"));
  });

  for (const [outcome, backup_compromised, level, key] of [
    ["Clean", false, "success", "patch.verify.clean"],
    ["FutureOutcome", false, "warning", "patch.verify.unknown"],
    ['DowngradeBlocked { installed: "2", incoming: "1" }', false, "warning", "patch.verify.downgrade"],
    ["Interrupted", false, "error", "patch.verify.interrupted"],
    ["Clean", true, "error", "patch.verify.backupCompromised"],
  ] as const) test(`${locale} verification ${outcome} backup_compromised=${backup_compromised} uses matching toast, log and panel severity`, async () => {
    reset(locale); selectSource(); status = "not_patched";
    verification = { outcome, backup_compromised };
    await button(t("patch.verify")).props.onClick();
    assert.deepEqual(h.toasts, [[level, t(key)]]);
    assert.equal(h.logs[0][0], level === "success" ? "info" : level);
    assert.equal(h.logs[0][1], t(key));
    assert.ok(nodes(render()).some(node => node.props?.className?.includes(level === "error" ? "border-danger" : level === "warning" ? "border-warning" : "border-border") && text(node).includes(t(key))));
  });
}
function selectGame(path: string) {
  nodes(render()).find(node => node.type === "input" && node.props.placeholder === t("patch.placeholder.game"))
    .props.onChange({ target: { value: path } });
}
async function settleApply() {
  for (let i = 0; button(t("patch.applyBtn")).props.disabled && i < 30; i++) {
    await new Promise(resolve => setTimeout(resolve, 0));
  }
  assert.equal(button(t("patch.applyBtn")).props.disabled, false);
}
function assertRecovery(expected: boolean) {
  const html = renderToStaticMarkup(render());
  assert.equal(html.includes(t("patch.partialWarning")), expected);
  assert.equal(html.includes(t("patch.rollbackNow")), expected);
}
for (const locale of ["en", "es"] as const) {
  for (const terminal of ["job error", "stream loss", "start refusal"] as const) {
    test(`${locale} ${terminal} before writes refreshes status without partial warning or rollback prompt`, async () => {
      reset(locale); selectSource();
      // A stale interrupted status must be replaced by the post-failure status.
      await button(t("patch.refreshStatus")).props.onClick();
      status = "not_patched";
      calls.length = 0;
      applyStartFailed = terminal === "start refusal";
      await button(t("patch.applyBtn")).props.onClick();
      if (terminal === "job error") h.jobHandlers!.onError({ message: "pre-write refusal" });
      if (terminal === "stream loss") h.jobHandlers!.onClosed();
      await settleApply();
      assert.deepEqual(calls.filter(call => call.url.endsWith("/patch/status")).map(call => call.body), [{ game_path: "/game" }]);
      assertRecovery(false);
      assert.ok(!h.toasts.some(([, message]) => String(message).includes(t("patch.toast.applyFailedPartial", { error: "pre-write refusal" }))));
      assert.ok(!h.toasts.some(([level]) => level === "success"));
    });
  }
  for (const partial of [false, true]) {
    test(`${locale} cancellation shows recovery only for refreshed interrupted status (partial=${partial})`, async () => {
      reset(locale); selectSource();
      status = partial ? "interrupted" : "not_patched";
      await button(t("patch.applyBtn")).props.onClick();
      await button(t("patch.cancelApply")).props.onClick();
      h.toasts = []; h.logs = [];
      h.jobHandlers!.onError({ message: "cancelled" });
      await settleApply();
      assert.deepEqual(calls.filter(call => call.url.endsWith("/patch/status")).map(call => call.body), [{ game_path: "/game" }]);
      assertRecovery(partial);
      const message = partial ? t("patch.toast.applyCancelled") : locale === "en"
        ? "Apply cancelled. Patch status does not report a partial application."
        : "Aplicación cancelada. El estado del parche no indica una aplicación parcial.";
      assert.deepEqual(h.toasts, [["warning", message]]);
      assert.equal(h.logs.some(([, title]) => title === t("activity.patch.cancelledPartial")), partial);
    });
  }
  for (const terminal of ["job error", "stream loss", "start refusal", "cancelled"] as const) {
    for (const lookupFails of [false, true]) {
      test(`${locale} ${terminal} reports state unknown when ${lookupFails ? "status lookup fails" : "backend status is unknown"}`, async () => {
        reset(locale); selectSource();
        await button(t("patch.refreshStatus")).props.onClick();
        statusFailed = lookupFails; status = "unknown";
        applyStartFailed = terminal === "start refusal";
        await button(t("patch.applyBtn")).props.onClick();
        if (terminal === "cancelled") await button(t("patch.cancelApply")).props.onClick();
        if (terminal !== "start refusal") { h.toasts = []; h.logs = []; }
        if (terminal === "stream loss") h.jobHandlers!.onClosed();
        if (terminal === "job error" || terminal === "cancelled") h.jobHandlers!.onError({ message: "apply stopped" });
        await settleApply();
        const message = locale === "en" ? "Game state unknown. Check patch status before continuing."
          : "Estado del juego desconocido. Compruebe el estado del parche antes de continuar.";
        assert.ok(renderToStaticMarkup(render()).includes(message));
        assertRecovery(false);
        assert.ok(h.toasts.some(([, text]) => String(text).includes(message)));
        assert.ok(!h.toasts.some(([level]) => level === "success"));
        assert.ok(!h.logs.some(([, title]) => title === t("activity.patch.cancelledPartial")));
      });
    }
  }
}

test("status recorded for game A is ignored after selecting game B", async () => {
  reset("en");
  await button(t("patch.refreshStatus")).props.onClick();
  assertRecovery(true);
  selectGame("/other-game");
  assertRecovery(false);
  assert.ok(!renderToStaticMarkup(render()).includes(t("patch.interrupted", { id: "test-patch" })));
});

test("an in-flight status response for game A cannot supply recovery for game B", async () => {
  reset("en");
  deferStatus = true;
  const pending = button(t("patch.refreshStatus")).props.onClick();
  for (let i = 0; !deferredStatus && i < 20; i++) await new Promise(resolve => setTimeout(resolve, 0));
  assert.ok(deferredStatus);
  selectGame("/other-game");
  deferredStatus!(); await pending;
  assertRecovery(false);
  selectGame("/game");
  assertRecovery(true);
});

test("apply interruption refreshes its original game and hides its recovery message on another game", async () => {
  reset("en"); selectSource();
  await button(t("patch.applyBtn")).props.onClick();
  selectGame("/other-game");
  calls.length = 0;
  h.jobHandlers!.onError({ message: "apply stopped" });
  await settleApply();
  assert.deepEqual(calls.filter(call => call.url.endsWith("/patch/status")).map(call => call.body), [{ game_path: "/game" }]);
  assertRecovery(false);
  assert.ok(!renderToStaticMarkup(render()).includes("apply stopped"));
  selectGame("/game");
  assertRecovery(true);
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

for (const locale of ["en", "es"] as const) test(`${locale} Rollback is outlined in normal and recovery states with unchanged action guards`, () => {
  reset(locale);
  const applyTab = (node: any): any => {
    if (!node || typeof node !== "object") return undefined;
    if (Array.isArray(node)) return node.map(applyTab).find(Boolean);
    return node.type === PatchApplyTab ? node : applyTab(node.props?.children);
  };
  const props = applyTab(render()).props;
  for (const loading of [false, true]) for (const applying of [false, true]) {
    for (const gamePath of ["", " /game "]) for (const sourceOk of [false, true]) {
      for (const showPartialWarning of [false, true]) {
        const tree = PatchApplyActions({ ...props, loading, applying, gamePath, sourceOk,
          canVerifyApply: Boolean(gamePath.trim()) && sourceOk, showPartialWarning });
        const action = (key: Parameters<typeof t>[0]) => nodes(tree).find(node => node.type === "button" && text(node).trim() === t(key));
        const rollback = action("patch.rollback");
        assert.equal(rollback.props.disabled, loading || applying);
        assert.equal(rollback.props.onClick, props.handleRollback);
        assert.match(rollback.props.className, /\bborder-warning\b/);
        assert.match(rollback.props.className, /\btext-warning\b/);
        assert.ok(!/(?:^|\s)(?:hover:)?bg-(?:warning|amber|red)/.test(rollback.props.className));
        for (const key of ["patch.applyBtn", "patch.verify"] as const) {
          assert.equal(action(key).props.disabled, loading || applying || !gamePath.trim() || !sourceOk);
        }
        assert.match(action("patch.applyBtn").props.className, /\bbg-accent\b/);
        assert.equal(action("patch.rollbackPreview").props.disabled, loading || applying || !gamePath.trim());
        const recovery = action("patch.rollbackNow");
        assert.equal(Boolean(recovery), !applying && showPartialWarning);
        if (recovery) {
          assert.equal(recovery.props.disabled, loading);
          assert.equal(recovery.props.onClick, props.handleRollback);
        }
      }
    }
  }
});

async function flushEffects() {
  for (let i = 0; i < 8; i++) {
    render();
    h.effects.splice(0).forEach(effect => effect());
    await new Promise(resolve => setTimeout(resolve, 0));
  }
}

for (const root of ["/copies/game.pck-es", "/shared"]) test(`Godot pack uses recorded ${root} root; apply requires an explicit directory and preserves cancellation`, async () => {
  recordedRoot = root;
  reset("en");
  h.values = []; h.runEffects = true; h.effectDeps = [];
  renderOptions = { defaultGamePath: "/shared/game.pck", defaultGamePathIsFile: true, initialTab: "pack", initialBackupId: "original", allowPack: true };
  try {
    await flushEffects();
    assert.ok(nodes(render()).some(node => node.type === "input" && node.props.value === root));
    const pristine = nodes(render()).find(node => node.type === "input" && node.props.type === "checkbox" && node.props.checked === true);
    assert.ok(pristine, "incoming backup enables pristine hashes");
    const output = nodes(render()).find(node => node.type === "input" && node.props.placeholder === t("patch.outputPlaceholder"));
    output.props.onChange({ target: { value: "/patch.zip" } });
    await button(t("patch.packBtn"), 1).props.onClick();
    await new Promise(resolve => setTimeout(resolve, 0));
    assert.ok(calls.some(call => call.url.endsWith("/patch/pack")), JSON.stringify(h.toasts));
    let request = [...calls].reverse().find(call => call.url.endsWith("/patch/pack"))!.body;
    assert.equal(request.game_path, root);
    assert.equal(request.pristine, true);
    const pristineFolder = nodes(render()).find(node => node.type === "input" && node.props.placeholder === t("patch.pristinePathPlaceholder"));
    pristineFolder.props.onChange({ target: { value: "/clean" } });
    pristine.props.onChange({ target: { checked: false } });
    await button(t("patch.packBtn"), 1).props.onClick();
    await new Promise(resolve => setTimeout(resolve, 0));
    request = [...calls].reverse().find(call => call.url.endsWith("/patch/pack"))!.body;
    assert.equal(request.pristine, false);
    assert.ok(!("pristine_path" in request) && !("pristine_backup_id" in request));
    pristine.props.onChange({ target: { checked: true } });
    await button(t("patch.packBtn"), 1).props.onClick();
    await new Promise(resolve => setTimeout(resolve, 0));
    request = [...calls].reverse().find(call => call.url.endsWith("/patch/pack"))!.body;
    assert.equal(request.pristine_path, "/clean");
    assert.ok(!("pristine_backup_id" in request));
    await button(t("patch.apply")).props.onClick();
    const input = () => nodes(render()).find(node => node.type === "input" && node.props.placeholder === t("patch.placeholder.game"));
    assert.equal(input().props.value, "");
    assert.equal(button(t("patch.verify")).props.disabled, true);
    const browse = () => nodes(render()).find(node => node.type === "button" && node.props.title === t("common.browse"));
    h.selectedDirectory = null;
    await browse().props.onClick();
    assert.equal(input().props.value, "");
    h.selectedDirectory = "/explicit-target";
    await browse().props.onClick();
    assert.equal(input().props.value, "/explicit-target");
    assert.ok(h.pickerCalls.every(options => options.directory === true));
    await button(t("patch.pack")).props.onClick();
    assert.ok(nodes(render()).some(node => node.type === "input" && node.props.value === root));
  } finally {
    h.runEffects = false; h.effects = []; renderOptions = {};
  }
});

for (const locale of ["en", "es"] as const) {
  for (const initialTab of ["apply", "pack"] as const) {
    test(`${locale} opening ${initialTab} for a never-patched game loads empty recordings without status, recovery or errors`, async () => {
      reset(locale);
      recordingsEmpty = true; status = "not_patched";
      h.values = []; h.runEffects = true; h.effectDeps = []; h.effects = [];
      renderOptions = { allowPack: true, initialTab };
      try {
        await flushEffects();
        assert.equal(calls.filter(call => call.url.endsWith("/patch/recordings")).length, 1);
        assert.ok(calls.every(call => call.url.endsWith("/patch/recordings") || call.url.includes("/patch/identity?")), JSON.stringify(calls));
        if (initialTab === "pack") {
          assert.ok(calls.some(call => new URL(call.url).searchParams.get("game_path") === "/game"));
          assert.ok(text(nodes(render())).includes(t("patch.recordedLangsEmpty")));
          assert.equal(button(t("patch.packBtn"), 1).props.disabled, true);
        }
        await button(t("patch.apply")).props.onClick();
        await flushEffects();
        await button(t("patch.refreshStatus")).props.onClick();
        assert.deepEqual(calls.filter(call => call.url.endsWith("/patch/status")).map(call => call.body), [{ game_path: "/game" }]);
        assert.ok(text(nodes(render())).includes(t("patch.status", { status: t("patch.status.notPatched") })));
        assertRecovery(false);
        assert.deepEqual(h.logs, []);
        assert.deepEqual(h.apiLogs, []);
        assert.deepEqual(h.toasts, []);
      } finally {
        h.runEffects = false; h.effects = []; renderOptions = {};
      }
    });
  }
  test(`${locale} a real status conflict remains visible and logged instead of becoming an empty state`, async () => {
    reset(locale); statusConflict = true;
    await button(t("patch.refreshStatus")).props.onClick();
    assert.ok(text(nodes(render())).includes("invalid backup manifest"));
    assert.equal(h.apiLogs.length, 1);
    assert.equal(h.apiLogs[0][0], "error");
    assert.ok(String(h.apiLogs[0][1]).includes("409"));
    assertRecovery(false);
  });
}
