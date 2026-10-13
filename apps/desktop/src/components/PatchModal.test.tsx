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
let statusFailed = false;
let verification = {
  outcome: "Clean", tier: "Strict", replaced: ["data/original.txt"], added: [] as string[], conflicts: [] as string[],
  messages: ["verification retained"], backup_compromised: false, manifest: {},
};
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
  if (path.endsWith("/patch/verify")) return new Response(JSON.stringify(verification));
  if (path.endsWith("/patch/status") && statusFailed) return new Response("status unavailable", { status: 500 });
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
  status = "interrupted"; statusFailed = false; h.jobHandlers = null;
  verification = { outcome: "Clean", tier: "Strict", replaced: ["data/original.txt"], added: [], conflicts: [],
    messages: ["verification retained"], backup_compromised: false, manifest: {} };
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
  status = "interrupted"; // The backend journal, not the error itself, requires recovery.
  h.jobHandlers.onError({ message: "partial apply" });
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

async function settleApply() {
  for (let i = 0; button(t("patch.applyBtn")).props.disabled && i < 30; i++) {
    await new Promise(resolve => setTimeout(resolve, 0));
  }
  assert.equal(button(t("patch.applyBtn")).props.disabled, false);
}
function selectSource() {
  nodes(render()).find(node => node.type === "input" && node.props.placeholder === t("patch.zipPlaceholder"))
    .props.onChange({ target: { value: "/patch.zip" } });
}
for (const locale of ["en", "es"] as const) {
  test(`${locale} verification refusal before writes reports an unchanged game, not a partial apply`, async () => {
    reset(locale);
    status = "not_patched";
    selectSource();
    await button(t("patch.applyBtn")).props.onClick();
    const raw = "patch verification failed: data/new.txt: expected absent (added path), found present";
    h.jobHandlers!.onError({ message: raw });
    await settleApply();
    const message = locale === "en"
      ? "This patch does not match the selected game. Nothing was changed. Check the game folder and version."
      : "Este parche no corresponde al juego seleccionado. No se realizó ningún cambio. Compruebe la carpeta y la versión del juego.";
    const html = renderToStaticMarkup(render());
    assert.ok(html.includes(message), html);
    assert.ok(!html.includes(t("patch.partialWarning")), html);
    assert.ok(!html.includes(t("patch.rollbackNow")), html);
    assert.deepEqual(h.toasts, [["error", t("patch.toast.applyFailed", { error: message })]]);
    assert.equal(h.logs.filter(([, , detail]) => detail === raw).length, 1);
    assert.ok(!h.toasts.some(([, text]) => String(text).includes(raw)));
  });

  test(`${locale} conflicting verification is a localized warning with per-file lines and technical-only Debug text`, async () => {
    reset(locale);
    status = "not_patched";
    selectSource();
    const outcome = 'Mismatch([FileMismatch { path: "data/new.txt", expected: "absent (added path)", found: "present" }, FileMismatch { path: "data/original.txt", expected: "aaaa", found: "bbbb" }])';
    verification = { ...verification, outcome, conflicts: ["data/new.txt"], messages: ["opaque backend detail"] };
    await button(t("patch.verify")).props.onClick();
    const summary = locale === "en" ? "Verification found 2 file conflicts. Nothing was changed."
      : "La verificación detectó 2 conflictos de archivos. No se realizó ningún cambio.";
    assert.deepEqual(h.toasts, [["warning", summary]]);
    const tree = render();
    const details = nodes(tree).filter(node => node.type === "details");
    assert.equal(details.length, 1, "raw diagnostics must be explicitly technical details");
    assert.ok(text(details[0]).includes(outcome) && text(details[0]).includes("opaque backend detail"));
    const html = renderToStaticMarkup(tree);
    assert.ok(html.includes(summary), html);
    assert.ok(html.includes(locale === "en" ? "data/new.txt: already exists" : "data/new.txt: ya existe"), html);
    assert.ok(html.includes(locale === "en" ? "data/original.txt: content does not match" : "data/original.txt: el contenido no coincide"), html);
    assert.ok(!html.includes("not_patched"), html);
    assert.equal(h.logs[0][0], "warning");
    assert.ok(String(h.logs[0][2]).includes(outcome));
  });

  test(`${locale} clean verification still succeeds and unknown/downgrade/interrupted outcomes never succeed`, async () => {
    reset(locale); selectSource();
    status = "not_patched";
    await button(t("patch.verify")).props.onClick();
    assert.equal(h.toasts[0][0], "success");
    for (const outcome of ["Unknown", "Interrupted", 'DowngradeBlocked { installed: "2.0.0", incoming: "1.0.0" }', "FutureOutcome"]) {
      h.toasts = [];
      verification.outcome = outcome;
      await button(t("patch.verify")).props.onClick();
      assert.equal(h.toasts[0][0], "warning", outcome);
      assert.ok(!String(h.toasts[0][1]).includes("DowngradeBlocked"));
      assert.ok(!String(h.toasts[0][1]).includes("FutureOutcome"));
    }
  });
}

test("failed status lookup and stream loss do not invent a partly patched state", async () => {
  reset("en"); selectSource();
  statusFailed = true;
  await button(t("patch.applyBtn")).props.onClick();
  h.jobHandlers!.onClosed();
  await settleApply();
  const html = renderToStaticMarkup(render());
  assert.ok(!html.includes(t("patch.partialWarning")), html);
  assert.ok(!h.toasts.some(([, text]) => String(text).includes("Nothing was changed")));
  assert.ok(html.includes("Could not confirm the game state"), html);
  assert.ok(calls.some(call => call.url.endsWith("/patch/status")), "stream loss must refresh actual backend state");
});

test("mismatch paths escaped by Rust Debug remain readable and duplicate added conflicts count once", async () => {
  for (const found of ["missing", "absent"]) {
    reset("en"); selectSource();
    verification.outcome = String.raw`Mismatch([FileMismatch { path: "data/quote\"name.txt", expected: "aaaa", found: "${found}" }])`;
    verification.conflicts = ['data/quote"name.txt'];
    await button(t("patch.verify")).props.onClick();
    assert.deepEqual(h.toasts, [["warning", "Verification found 1 file conflict. Nothing was changed."]]);
    assert.ok(renderToStaticMarkup(render()).includes("data/quote&quot;name.txt: missing file"), found);
  }
});

test("a recovery banner belongs to the game whose backend status was read", async () => {
  reset("en");
  await button(t("patch.refreshStatus")).props.onClick();
  assert.ok(renderToStaticMarkup(render()).includes(t("patch.partialWarning")));
  const game = nodes(render()).find(node => node.type === "input" && node.props.value === " /game ");
  game.props.onChange({ target: { value: "/other-game" } });
  const html = renderToStaticMarkup(render());
  assert.ok(!html.includes(t("patch.partialWarning")), html);
  assert.ok(!html.includes(t("patch.rollbackNow")), html);
});

for (const partial of [false, true]) test(`cancellation preserves recovery only when the backend reports an interruption (partial=${partial})`, async () => {
  reset("en"); selectSource();
  status = partial ? "interrupted" : "not_patched";
  await button(t("patch.applyBtn")).props.onClick();
  await button(t("patch.cancelApply")).props.onClick();
  h.toasts = [];
  h.jobHandlers!.onError({ message: "cancelled" });
  await settleApply();
  const html = renderToStaticMarkup(render());
  assert.equal(html.includes(t("patch.partialWarning")), partial);
  assert.equal(h.toasts[0][0], "warning");
  assert.equal(h.toasts[0][1], t(partial ? "patch.toast.applyCancelled" : "patch.toast.applyCancelledSafe"));
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
