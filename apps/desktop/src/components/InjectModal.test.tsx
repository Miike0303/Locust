import assert from "node:assert/strict";
import { after, test } from "node:test";
import { registerHooks } from "node:module";
import { renderToStaticMarkup } from "react-dom/server";
import { setLocale, t } from "../lib/i18n/index.ts";
import * as a11y from "../lib/modalA11y.ts";
import { queryClient } from "../lib/queryClient.ts";
import { useLogStore } from "../stores/logStore.ts";

// Follow the existing modal tests: run real callbacks and SSR with a small
// hook harness. Keep the API client real and capture its Tauri commands.
const h = {
  t, a11y, values: [] as unknown[], cursor: 0,
  setTauri: (_value: boolean) => {},
  effects: [] as (() => void)[], changed: false,
  toasts: [] as unknown[][], logs: [] as unknown[][],
  effect(callback: () => void, deps: unknown[]) {
    const index = this.cursor++;
    const previous = this.values[index] as unknown[] | undefined;
    if (!previous || deps.some((dep, i) => !Object.is(dep, previous[i]))) {
      this.values[index] = deps;
      this.effects.push(callback);
    }
  },
  project: { path: "/game", name: "Game", format_id: "rpgmaker-mz", supported_modes: ["replace", "add"] },
  state(initial: unknown) {
    const index = this.cursor++;
    if (!(index in this.values)) this.values[index] = typeof initial === "function" ? initial() : initial;
    return [this.values[index], (value: any) => {
      const next = typeof value === "function" ? value(this.values[index]) : value;
      if (!Object.is(next, this.values[index])) this.changed = true;
      this.values[index] = next;
    }];
  },
};
(globalThis as any).__injectModal = h;
const hooks = registerHooks({
  load(url, context, nextLoad) {
    if (url.replace(/\\/g, "/").endsWith("/src/lib/runtime.ts")) {
      return { format: "module", shortCircuit: true, source: `
        export let IS_TAURI=true;
        export const isTauri=()=>IS_TAURI;
        globalThis.__injectModal.setTauri=value=>{IS_TAURI=value;};` };
    }
    return nextLoad(url, context);
  },
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/components/InjectModal.tsx")) {
      const stubs: Record<string, string> = {
        react: `const h=globalThis.__injectModal;export const useState=initial=>h.state(initial);export const useMemo=fn=>fn();export const useEffect=(fn,deps)=>h.effect(fn,deps);export const useSyncExternalStore=(_subscribe,read)=>read();`,
        "react-router-dom": `export const useNavigate=()=>()=>{};`,
        "../lib/i18n": `export const useT=()=>globalThis.__injectModal.t;`,
        "../stores/projectStore": `export const useProjectStore=()=>({project:globalThis.__injectModal.project});`,
        "../stores/toastStore": `export const addToast=(...args)=>globalThis.__injectModal.toasts.push(args);`,
        "../stores/logStore": `export const addLog=(...args)=>globalThis.__injectModal.logs.push(args);`,
        "../lib/modalA11y": `const h=globalThis.__injectModal;
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
const calls: { command: string; params: any }[] = [];
const storage = new Map<string, string>();
const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
const storageDescriptor = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
let stringsWritten = 3;
let injectFailure: string | Error | undefined;
const originalFetch = globalThis.fetch;
Object.defineProperty(globalThis, "window", { configurable: true, value: {
  addEventListener() {},
  __TAURI_INTERNALS__: { invoke: async (command: string, args: any) => {
    calls.push({ command, params: args?.params });
    if (command === "run_validation") return { validation: { by_kind: {} } };
    if (command === "get_server_port") return 7842;
    if (command === "run_inject" && injectFailure !== undefined) throw injectFailure;
    if (command === "run_inject") return {
      mode: args.params.direct ? "direct" : args.params.mode,
      languages_processed: ["es"], languages_failed: [], backup_id: "backup",
      reports: { es: { strings_written: stringsWritten, files_modified: stringsWritten ? 1 : 0 } },
      injected_roots: args.params.mode === "replace" ? { es: "/returned/actual-copy-es" } : {},
    };
    if (command === "register_lang") return {
      plugins_js: true, iavra_languages: false, visumz_options: true,
      maps_patched: ["data/Map001.json"], backups: [], notes: [],
    };
    throw new Error(`Unexpected API command: ${command}`);
  } },
} });
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => storage.get(key) ?? null,
  setItem: (key: string, value: string) => { storage.set(key, value); },
  removeItem: (key: string) => { storage.delete(key); },
} });
const { default: InjectModal } = await import("./InjectModal.tsx");
// Resolve the base through the mocked native port before switching to HTTP in Node.
globalThis.fetch = async () => new Response(null, { status: 404 });
await (await import("../lib/api")).getCurrentProject();
globalThis.fetch = originalFetch;
after(() => {
  hooks.deregister();
  useLogStore.getState().clear();
  globalThis.fetch = originalFetch;
  setLocale("en");
  if (windowDescriptor) Object.defineProperty(globalThis, "window", windowDescriptor);
  else Reflect.deleteProperty(globalThis, "window");
  if (storageDescriptor) Object.defineProperty(globalThis, "localStorage", storageDescriptor);
  else Reflect.deleteProperty(globalThis, "localStorage");
  Reflect.deleteProperty(globalThis, "__injectModal");
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
  for (let attempt = 0; attempt < 10; attempt++) {
    h.cursor = 0; h.effects = []; h.changed = false;
    const tree = InjectModal({ open: true, onClose() {} });
    h.effects.forEach(effect => effect());
    if (!h.changed) return tree;
  }
  throw new Error("InjectModal did not settle after effects");
}
function button(label: string) {
  const tree = render();
  const found = nodes(tree).find(node => node.type === "button" && text(node).trim() === label);
  assert.ok(found, `missing action: ${label}`);
  const footer = nodes(tree).find(node => node.type === "footer");
  assert.ok(nodes(footer).includes(found), `action must stay in the footer: ${label}`);
  return found;
}
function checkbox() {
  const found = nodes(render()).find(node => node.type === "input" && node.props.type === "checkbox");
  assert.ok(found, "missing auto-register checkbox");
  return found;
}
function selectMode(mode: "add" | "direct" | "replace") {
  const select = nodes(render()).find(node => node.type === "select" && node.props.id === "inject-mode");
  select.props.onChange({ target: { value: mode } });
}
function reset(locale: "en" | "es", mode: "add" | "direct" | "replace", autoRegister = true) {
  storage.clear();
  injectFailure = undefined;
  globalThis.fetch = originalFetch;
  h.setTauri(true);
  storage.set("locust.inject.autoRegister", autoRegister ? "1" : "0");
  h.values = []; h.toasts = []; h.logs = []; calls.length = 0; stringsWritten = 3;
  h.project = { path: "/game", name: "Game", format_id: "rpgmaker-mz", supported_modes: ["replace", "add"] };
  setLocale(locale);
  selectMode(mode);
}
async function inject(mode: "add" | "direct" | "replace") {
  if (mode === "replace") {
    const output = nodes(render()).find(node => node.type === "input" && node.props.placeholder === t("inject.outputPlaceholder"));
    output.props.onChange({ target: { value: "/output" } });
  }
  const action = button(t(mode === "direct" ? "inject.directAction" : mode === "replace" ? "inject.copyAction" : "inject.injectAction"));
  assert.equal(action.props.disabled, false);
  await action.props.onClick();
  assert.equal(calls.filter(call => call.command === "run_inject").length, 1);
  assert.ok(renderToStaticMarkup(render()).includes(t(stringsWritten ? "inject.injectionComplete" : "inject.injectionEmpty")));
}
function modeSelect() {
  return nodes(render()).find(node => node.type === "select" && node.props.id === "inject-mode");
}
function setOutput(value: string) {
  const input = nodes(render()).find(node => node.type === "input" && node.props.placeholder === t("inject.outputPlaceholder"));
  assert.ok(input, "copy mode must offer an output folder");
  input.props.onChange({ target: { value } });
}
function replaceOnly(format = "yuris") {
  h.project = { ...h.project, format_id: format, supported_modes: ["replace"] };
}
const registerCalls = () => calls.filter(call => call.command === "register_lang");
const directHint = {
  en: "Registering a language would change files recorded by Direct injection and block packing or re-injection. Use Add mode to register a selectable language.",
  es: "Registrar un idioma modificaría archivos registrados por la inyección directa e impediría empaquetar o volver a inyectar. Use el modo Añadir para registrar un idioma seleccionable.",
};

for (const locale of ["en", "es"] as const) {
  test(`${locale} Inject selection summary uses the language name, not the selected code`, () => {
    reset(locale, "add");
    const summary = () => nodes(render()).find(node => node.type === "p" && text(node).startsWith(t("inject.selected", { langs: "" })));
    assert.ok(text(summary()).startsWith(t("inject.selected", { langs: "Español" })));
    const french = nodes(render()).find(node => node.type === "label" && text(node) === "Français");
    nodes(french).find(node => node.type === "input").props.onChange();
    assert.ok(text(summary()).startsWith(t("inject.selected", { langs: "Français" })));
    assert.equal(calls.length, 0);
  });

  test(`${locale} Inject warns only for known empty translation counts without gating injection`, () => {
    reset(locale, "add");
    queryClient.clear();
    const key = ["stats", JSON.stringify([null, h.project.path, h.project.format_id])];
    const warningText = locale === "en"
      ? "The last loaded project counts show no translated or approved strings. Check your translations before injecting."
      : "Los últimos conteos cargados no muestran textos traducidos ni aprobados. Revise las traducciones antes de inyectar.";
    const warning = () => nodes(render()).find(node => node.props?.role === "status" && text(node) === warningText);
    try {
      assert.equal(warning(), undefined, "unknown counts are not zero");
      queryClient.setQueryData(["stats", "another-project"], { translated: 0, approved: 0, reviewed: 0 });
      assert.equal(warning(), undefined, "counts must belong to this project");
      for (const [translated, approved, reviewed] of [[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, 0, 1]]) {
        queryClient.setQueryData(key, { translated, approved, reviewed });
        const cached = queryClient.getQueryCache().find({ queryKey: key })!;
        const options = cached.options;
        const empty = translated + approved + reviewed === 0;
        assert.equal(Boolean(warning()), empty);
        if (empty) assert.match(warning().props.className, /\btext-warning\b/);
        assert.equal(button(t("inject.injectAction")).props.disabled, false);
        assert.equal(cached.options, options, "presentation must not change the Editor's query options");
        assert.equal(cached.getObserversCount(), 0, "presentation must not register another query observer");
      }
      assert.equal(calls.length, 0, "displaying counts must not call the backend");
    } finally { queryClient.clear(); }
  });
}

test("Replace-only mounts in copy mode and coerces Add after project changes", () => {
  reset("en", "add");
  replaceOnly();
  h.values = [];
  assert.equal(modeSelect().props.value, "replace");
  assert.equal(button(t("inject.copyAction")).props.disabled, true);
  reset("en", "add");
  replaceOnly("kirikiri");
  assert.equal(modeSelect().props.value, "replace");
});

test("explicit Direct survives rerenders and equivalent project/mode state updates", async () => {
  reset("en", "add");
  replaceOnly();
  selectMode("direct");
  assert.equal(modeSelect().props.value, "direct");
  h.project = { ...h.project, supported_modes: ["replace"] };
  assert.equal(modeSelect().props.value, "direct");
  replaceOnly("kirikiri");
  assert.equal(modeSelect().props.value, "direct");
  await inject("direct");
  assert.deepEqual(calls.find(call => call.command === "run_inject")?.params, {
    project_path: "/game", format_id: "kirikiri", languages: ["es"], direct: true,
    mode: undefined, output_dir: undefined,
  });
});

for (const locale of ["en", "es"] as const) {
  test(`${locale} copy gates empty/whitespace output before validation or injection`, async () => {
    reset(locale, "replace");
    replaceOnly();
    for (const output of ["", "   "]) {
      setOutput(output);
      const action = button(t("inject.copyAction"));
      assert.equal(action.props.disabled, true);
      await action.props.onClick();
      assert.equal(calls.length, 0, "even a manually invoked handler must not send a request");
      assert.equal(h.toasts[h.toasts.length - 1]?.[1], t("inject.toast.selectOutput"));
    }
    setOutput(" /output ");
    const action = button(t("inject.copyAction"));
    assert.equal(action.props.disabled, false);
    await action.props.onClick();
    assert.deepEqual(calls.find(call => call.command === "run_inject")?.params, {
      project_path: "/game", format_id: "yuris", mode: "replace", languages: ["es"],
      output_dir: "/output", direct: false, require_binary_slot_fit: true,
    });
  });

  test(`${locale} copy explains originals stay untouched and shows the returned destination`, async () => {
    reset(locale, "replace");
    replaceOnly();
    const originalHint = locale === "en" ? "your original stays untouched" : "el original no se toca";
    assert.ok(text(render()).includes(originalHint));
    const capacityHint = locale === "en"
      ? "Known binary slot limits are checked before backup or copy creation. This does not guarantee a complete translation."
      : "Los límites conocidos de los espacios binarios se comprueban antes de crear la copia de seguridad o la copia traducida. Esto no garantiza una traducción completa.";
    assert.ok(text(render()).includes(capacityHint));
    await inject("replace");
    const destination = locale === "en"
      ? "Translated copy (es): /returned/actual-copy-es"
      : "Copia traducida (es): /returned/actual-copy-es";
    assert.ok(text(render()).includes(destination), "result must identify the actual backend-returned copy, not just the chosen parent folder");
    assert.ok(h.logs.some(log => String(log[2]).includes("/returned/actual-copy-es")));
    assert.ok(!text(render()).includes("Output: /output/"));
  });
}

for (const locale of ["en", "es"] as const) {
  for (const shape of ["string", "Error", "HTTP"] as const) {
    test(`${locale} capacity refusal (${shape}) shows count-only guidance, not success`, async () => {
      reset(locale, "replace");
      replaceOnly("unity");
      const raw = "injection error: 2 translation(s) exceed known binary slot capacity; Replace is refused before backup or output; run validation and resolve the capacity issues";
      injectFailure = shape === "Error" ? new Error(raw) : raw;
      if (shape === "HTTP") {
        h.setTauri(false);
        globalThis.fetch = async (url, init) => {
          if (String(url).endsWith("/validate")) return new Response(JSON.stringify({ validation: { by_kind: {} } }));
          assert.equal(String(url), "http://localhost:7842/api/inject");
          calls.push({ command: "run_inject", params: JSON.parse(String(init?.body)) });
          return new Response(raw, { status: 422 });
        };
      }
      setOutput("/output");
      await button(t("inject.copyAction")).props.onClick();
      const expected = locale === "en"
        ? "2 translation(s) exceed known binary slot capacity. The translated copy was refused before backup or output creation. Run Validate and resolve the capacity issues."
        : "2 traducción(es) superan la capacidad conocida de los espacios binarios. Se rechazó la copia traducida antes de crear copias de seguridad o archivos de salida. Ejecute Validar y resuelva los problemas de capacidad.";
      assert.deepEqual(h.toasts, [["error", t("inject.toast.failed", { error: expected })]]);
      assert.deepEqual(h.logs, [["error", t("activity.inject.failed"), expected, "inject"]], "existing capacity family keeps localized activity detail");
      assert.ok(!text(render()).includes(t("inject.injectionComplete")));
      assert.equal(calls.find(call => call.command === "run_inject")?.params.require_binary_slot_fit, true);
      assert.ok(!JSON.stringify(h.toasts).includes("injection error:"));
    });
  }
}

const yurisEncodingRefusal = "injection error: translation cannot be encoded as Shift-JIS for the declared YU-RIS quoted literal; injection is refused before backup or output; the original game and saved translation remain unchanged";
for (const locale of ["en", "es"] as const) {
  for (const shape of ["string", "Error", "HTTP"] as const) {
    test(`${locale} encoding refusal (${shape}) preserves raw activity and localized toast without success`, async () => {
      reset(locale, "replace");
      replaceOnly();
      useLogStore.getState().clear();
      injectFailure = shape === "Error" ? new Error(yurisEncodingRefusal) : yurisEncodingRefusal;
      if (shape === "HTTP") {
        h.setTauri(false);
        globalThis.fetch = async (url, init) => {
          if (String(url).endsWith("/validate")) return new Response(JSON.stringify({ validation: { by_kind: {} } }));
          assert.equal(String(url), "http://localhost:7842/api/inject");
          calls.push({ command: "run_inject", params: JSON.parse(String(init?.body)) });
          return new Response(yurisEncodingRefusal, { status: 500 });
        };
      }
      setOutput("/output");
      const projectBefore = structuredClone(h.project);
      await button(t("inject.copyAction")).props.onClick();
      const raw = shape === "HTTP" ? `500: ${yurisEncodingRefusal}` : yurisEncodingRefusal;
      const expected = locale === "es" ? t("api.error.yurisEncodingRefused")
        : shape === "HTTP" ? `Server error ${raw}` : raw;
      assert.deepEqual(h.logs, [["error", t("activity.inject.failed"), raw, "inject"]]);
      assert.deepEqual(h.toasts, [["error", t("inject.toast.failed", { error: expected })]]);
      assert.ok(!text(render()).includes(t("inject.injectionComplete")));
      assert.ok(!text(render()).includes("/returned/actual-copy-es"));
      assert.deepEqual(h.project, projectBefore);
      assert.deepEqual(calls.filter(call => call.command === "run_inject"), [{ command: "run_inject", params: {
        project_path: "/game", format_id: "yuris", languages: ["es"], output_dir: "/output",
        direct: false, mode: "replace", require_binary_slot_fit: true,
      } }]);
      assert.equal(registerCalls().length, 0);
      const apiLogs = useLogStore.getState().entries;
      assert.equal(apiLogs.length, shape === "HTTP" ? 1 : 0, "no duplicate API-layer diagnostics");
      if (shape === "HTTP") assert.deepEqual([apiLogs[0].message, apiLogs[0].detail, apiLogs[0].source],
        ["API 500: /inject", yurisEncodingRefusal, "api"]);
    });
  }

  test(`${locale} unowned encoding diagnostic retains existing activity and toast fallback`, async () => {
    for (const raw of ["unknown synthetic diagnostic", yurisEncodingRefusal + " extra", yurisEncodingRefusal + "\nopaque"]) {
      reset(locale, "replace");
      injectFailure = new Error(raw);
      setOutput("/output");
      await button(t("inject.copyAction")).props.onClick();
      assert.deepEqual(h.logs, [["error", t("activity.inject.failed"), raw, "inject"]]);
      assert.deepEqual(h.toasts, [["error", t("inject.toast.failed", { error: raw })]]);
    }
  });

  test(`${locale} non-ApiError failures retain Error and unknown-value behavior`, async () => {
    const invalidateQueries = queryClient.invalidateQueries;
    try {
      for (const failure of [new Error("synthetic refresh failure"), "synthetic non-Error failure"]) {
        reset(locale, "replace");
        queryClient.invalidateQueries = () => { throw failure; };
        setOutput("/output");
        await button(t("inject.copyAction")).props.onClick();
        const expected = failure instanceof Error ? failure.message : String(failure);
        assert.deepEqual(h.logs, [["error", t("activity.inject.failed"), expected, "inject"]]);
        assert.deepEqual(h.toasts, [["error", t("inject.toast.failed", { error: expected })]]);
      }
    } finally { queryClient.invalidateQueries = invalidateQueries; }
  });
}

// Exercise the real inject client at both mocked transport boundaries, including legacy callers.
for (const tauri of [true, false]) {
  test(`${tauri ? "Tauri" : "HTTP"} injection preserves the optional capacity flag and normal response`, async () => {
    reset("en", "replace");
    h.setTauri(tauri);
    const { inject: sendInject } = await import("../lib/api");
    const httpCalls: unknown[][] = [];
    globalThis.fetch = async (url, init) => {
      httpCalls.push([String(url), init?.method, JSON.parse(String(init?.body))]);
      return new Response(JSON.stringify({ mode: "replace", languages_processed: ["es"], languages_failed: [], reports: {} }));
    };
    for (const flag of [undefined, false, true]) {
      const params = {
        project_path: "/game", format_id: "unity", mode: "replace" as const,
        languages: ["es"], output_dir: "/output", direct: false,
        ...(flag === undefined ? {} : { require_binary_slot_fit: flag }),
      };
      const result = await sendInject(params);
      assert.equal(result.mode, "replace");
      assert.deepEqual(result.languages_failed, []);
      if (tauri) assert.deepEqual(calls[calls.length - 1], { command: "run_inject", params });
      else assert.deepEqual(httpCalls[httpCalls.length - 1], ["http://localhost:7842/api/inject", "POST", params]);
    }
    h.setTauri(true);
    globalThis.fetch = originalFetch;
  });
}

test("Direct injection preserves the stored auto-register preference without calling register-lang", async () => {
  reset("en", "direct");
  await inject("direct");
  assert.equal(registerCalls().length, 0);
  assert.notEqual(calls.find(call => call.command === "run_inject")?.params.require_binary_slot_fit, true);
  assert.equal(storage.get("locust.inject.autoRegister"), "1");
});

test("Add injection with the stored preference registers the selected language exactly once", async () => {
  reset("en", "add");
  await inject("add");
  assert.deepEqual(registerCalls(), [{ command: "register_lang", params: { game_path: "/game", lang: "es", label: "Español" } }]);
  assert.notEqual(calls.find(call => call.command === "run_inject")?.params.require_binary_slot_fit, true);
  assert.equal(storage.get("locust.inject.autoRegister"), "1");
});

for (const locale of ["en", "es"] as const) {
  test(`${locale} Direct controls are disabled with an accessible explanation; Add restores the remembered option`, () => {
    reset(locale, "direct");
    const controls = [checkbox(), button(t("inject.registerOnly", { langs: "es" }))];
    for (const control of controls) {
      assert.equal(control.props.disabled, true);
      const hint = nodes(render()).find(node => node.props?.id === control.props["aria-describedby"]);
      assert.ok(hint && text(hint) === directHint[locale], "disabled control must describe the recording conflict and Add alternative");
    }
    assert.ok(renderToStaticMarkup(render()).includes(directHint[locale]));
    assert.equal(storage.get("locust.inject.autoRegister"), "1");
    selectMode("add");
    assert.equal(checkbox().props.disabled, false);
    assert.equal(checkbox().props.checked, true);
    assert.equal(button(t("inject.registerOnly", { langs: "es" })).props.disabled, false);
    assert.ok(!renderToStaticMarkup(render()).includes(directHint[locale]));
  });

  test(`${locale} Direct result also disables manual registration and explains why`, async () => {
    reset(locale, "direct");
    await inject("direct");
    const action = button(t("inject.registerUi", { langs: "es" }));
    assert.equal(action.props.disabled, true);
    const hint = nodes(render()).find(node => node.props?.id === action.props["aria-describedby"]);
    assert.ok(hint && text(hint) === directHint[locale]);
    assert.ok(renderToStaticMarkup(render()).includes(directHint[locale]));
  });
}

test("Direct manual handlers cannot register before or after injection", async () => {
  reset("en", "direct");
  await button(t("inject.registerOnly", { langs: "es" })).props.onClick();
  await inject("direct");
  await button(t("inject.registerUi", { langs: "es" })).props.onClick();
  assert.equal(registerCalls().length, 0);
});

test("Replace mode cannot register automatically or manually", async () => {
  reset("en", "replace");
  assert.equal(checkbox().props.disabled, true);
  assert.equal(button(t("inject.registerOnly", { langs: "es" })).props.disabled, true);
  await button(t("inject.registerOnly", { langs: "es" })).props.onClick();
  await inject("replace");
  assert.equal(button(t("inject.registerUi", { langs: "es" })).props.disabled, true);
  await button(t("inject.registerUi", { langs: "es" })).props.onClick();
  assert.equal(registerCalls().length, 0);
});

test("Add manual registration still uses the custom label before and after injection", async () => {
  reset("en", "add", false);
  const label = nodes(render()).find(node => node.type === "input" && node.props.placeholder === "Español");
  label.props.onChange({ target: { value: "Spanish (custom)" } });
  await button(t("inject.registerOnly", { langs: "es" })).props.onClick();
  assert.equal(registerCalls().length, 1);
  await inject("add");
  assert.equal(registerCalls().length, 1, "auto-registration remains opt-in");
  const action = button(t("inject.registerUi", { langs: "es" }));
  assert.equal(action.props.disabled, false);
  await action.props.onClick();
  assert.deepEqual(registerCalls().map(call => call.params), [
    { game_path: "/game", lang: "es", label: "Spanish (custom)" },
    { game_path: "/game", lang: "es", label: "Spanish (custom)" },
  ]);
});

test("Add injection still skips automatic registration when no translations were written", async () => {
  reset("en", "add");
  stringsWritten = 0;
  await inject("add");
  assert.equal(registerCalls().length, 0);
});

for (const locale of ["en", "es"] as const) test(`${locale} Godot defaults to Replace, offers Direct, omits Add and explains archive-only copy`, () => {
  reset(locale, "add");
  replaceOnly("godot-dialogic");
  h.project.path = "/shared/game.pck";
  h.values = [];
  assert.equal(modeSelect().props.value, "replace");
  const options = nodes(modeSelect()).filter(node => node.type === "option").map(node => node.props.value);
  assert.deepEqual(options, ["replace", "direct"]);
  assert.ok(text(render()).includes(t("inject.archiveOnly")));
});
