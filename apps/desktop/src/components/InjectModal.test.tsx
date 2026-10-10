import assert from "node:assert/strict";
import { after, test } from "node:test";
import { registerHooks } from "node:module";
import { renderToStaticMarkup } from "react-dom/server";
import { setLocale, t } from "../lib/i18n/index.ts";
import * as a11y from "../lib/modalA11y.ts";

// Follow the existing modal tests: run real callbacks and SSR with a small
// hook harness. Keep the API client real and capture its Tauri commands.
const h = {
  t, a11y, values: [] as unknown[], cursor: 0,
  project: { path: "/game", name: "Game", format_id: "rpgmaker-mz", supported_modes: ["replace", "add"] },
  state(initial: unknown) {
    const index = this.cursor++;
    if (!(index in this.values)) this.values[index] = typeof initial === "function" ? initial() : initial;
    return [this.values[index], (value: any) => {
      this.values[index] = typeof value === "function" ? value(this.values[index]) : value;
    }];
  },
};
(globalThis as any).__injectModal = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/components/InjectModal.tsx")) {
      const stubs: Record<string, string> = {
        react: `const h=globalThis.__injectModal;export const useState=initial=>h.state(initial);export const useMemo=fn=>fn();export const useEffect=()=>{};`,
        "react-router-dom": `export const useNavigate=()=>()=>{};`,
        "../lib/i18n": `export const useT=()=>globalThis.__injectModal.t;`,
        "../stores/projectStore": `export const useProjectStore=()=>({project:globalThis.__injectModal.project});`,
        "../stores/toastStore": `export const addToast=()=>{};`,
        "../stores/logStore": `export const addLog=()=>{};`,
        "../lib/modalA11y": `const h=globalThis.__injectModal;
          export const MODAL_BACKDROP_CLASS=h.a11y.MODAL_BACKDROP_CLASS;
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
Object.defineProperty(globalThis, "window", { configurable: true, value: {
  __TAURI_INTERNALS__: { invoke: async (command: string, args: any) => {
    calls.push({ command, params: args?.params });
    if (command === "run_validation") return { validation: { by_kind: {} } };
    if (command === "run_inject") return {
      mode: args.params.direct ? "direct" : args.params.mode,
      languages_processed: ["es"], languages_failed: [], backup_id: "backup",
      reports: { es: { strings_written: stringsWritten, files_modified: stringsWritten ? 1 : 0 } },
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
after(() => {
  hooks.deregister();
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
  h.cursor = 0;
  return InjectModal({ open: true, onClose() {} });
}
function button(label: string) {
  const found = nodes(render()).find(node => node.type === "button" && text(node).trim() === label);
  assert.ok(found, `missing action: ${label}`);
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
  storage.set("locust.inject.autoRegister", autoRegister ? "1" : "0");
  h.values = []; calls.length = 0; stringsWritten = 3;
  setLocale(locale);
  selectMode(mode);
}
async function inject(mode: "add" | "direct" | "replace") {
  if (mode === "replace") {
    const output = nodes(render()).find(node => node.type === "input" && node.props.placeholder === t("inject.outputPlaceholder"));
    output.props.onChange({ target: { value: "/output" } });
  }
  const action = button(t(mode === "direct" ? "inject.directAction" : "inject.injectAction"));
  assert.equal(action.props.disabled, false);
  await action.props.onClick();
  assert.equal(calls.filter(call => call.command === "run_inject").length, 1);
  assert.ok(renderToStaticMarkup(render()).includes(t(stringsWritten ? "inject.injectionComplete" : "inject.injectionEmpty")));
}
const registerCalls = () => calls.filter(call => call.command === "register_lang");
const directHint = {
  en: "Registering a language would change files recorded by Direct injection and block packing or re-injection. Use Add mode to register a selectable language.",
  es: "Registrar un idioma modificaría archivos registrados por la inyección directa e impediría empaquetar o volver a inyectar. Use el modo Añadir para registrar un idioma seleccionable.",
};

test("Direct injection preserves the stored auto-register preference without calling register-lang", async () => {
  reset("en", "direct");
  await inject("direct");
  assert.equal(registerCalls().length, 0);
  assert.equal(storage.get("locust.inject.autoRegister"), "1");
});

test("Add injection with the stored preference registers the selected language exactly once", async () => {
  reset("en", "add");
  await inject("add");
  assert.deepEqual(registerCalls(), [{ command: "register_lang", params: { game_path: "/game", lang: "es", label: "Español" } }]);
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
