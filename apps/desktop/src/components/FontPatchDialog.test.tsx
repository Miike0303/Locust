import assert from "node:assert/strict";
import { after, beforeEach, test } from "node:test";
import { registerHooks } from "node:module";
import { setLocale, t } from "../lib/i18n";
import * as a11y from "../lib/modalA11y";
import type { ProjectInfo } from "../lib/api";

const project: ProjectInfo = { path: "C:/fixture/game-a", name: "Project A", format_id: "html", database_path: "C:/fixture/project-a.db" };
const other: ProjectInfo = { ...project, path: "C:/fixture/game-b", name: "Project B", database_path: "C:/fixture/project-b.db" };
const h = {
  t, a11y, project, native: false, defaultGamePath: project.path,
  states: [] as any[], cursor: 0, effects: [] as (() => void)[], changed: false,
  bodies: [] as Record<string, unknown>[], failure: "", closed: 0, used: [] as string[][],
  state(initial: any) {
    const index = this.cursor++;
    if (!(index in this.states)) this.states[index] = typeof initial === "function" ? initial() : initial;
    return [this.states[index], (value: any) => {
      const next = typeof value === "function" ? value(this.states[index]) : value;
      if (!Object.is(next, this.states[index])) this.changed = true;
      this.states[index] = next;
    }];
  },
  effect(fn: () => void | (() => void), deps: unknown[]) {
    const index = this.cursor++;
    const previous = this.states[index] as { deps: unknown[]; cleanup?: () => void } | undefined;
    if (!previous || deps.some((value, i) => !Object.is(value, previous.deps[i]))) {
      const effect = { deps, cleanup: undefined as (() => void) | undefined };
      this.states[index] = effect;
      this.effects.push(() => { previous?.cleanup?.(); effect.cleanup = fn() || undefined; });
    }
  },
};
(globalThis as any).__fontPatchDialog = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    let source: string | undefined;
    if (context.parentURL?.endsWith("/components/FontPatchDialog.tsx")) {
      const stubs: Record<string, string> = {
        react: "export const useState=initial=>h.state(initial);export const useEffect=(fn,deps)=>h.effect(fn,deps);",
        "../lib/i18n": "export const useT=()=>h.t;",
        "../stores/projectStore": "export const useProjectStore=()=>({project:h.project});",
        "../lib/modalA11y": "export const useModalA11y=()=>({dialogRef:{current:null},dialogProps:h.a11y.buildModalDialogProps({titleId:'title'}),titleProps:h.a11y.buildModalTitleProps('title')});export const MODAL_BACKDROP_CLASS=h.a11y.MODAL_BACKDROP_CLASS;export const MODAL_HEADER_CLASS=h.a11y.MODAL_HEADER_CLASS;export const MODAL_BODY_CLASS=h.a11y.MODAL_BODY_CLASS;export const MODAL_FOOTER_CLASS=h.a11y.MODAL_FOOTER_CLASS;export const modalPanelClass=h.a11y.modalPanelClass;",
      };
      source = stubs[specifier];
    }
    if (context.parentURL?.endsWith("/lib/api.ts")) {
      if (specifier === "./runtime") source = "export const IS_TAURI=true;";
      if (specifier === "@tauri-apps/api/core") source = "export const invoke=async()=>7842;";
    }
    if (source) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__fontPatchDialog;" + source)}` };
    return nextResolve(specifier, context);
  },
});
const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
const originalFetch = globalThis.fetch;
function setNative(native: boolean) {
  h.native = native;
  Object.defineProperty(globalThis, "window", { configurable: true, value: native ? { __TAURI_INTERNALS__: {} } : {} });
}
setNative(false);
globalThis.fetch = async (input, init) => {
  const url = new URL(String(input));
  if (url.pathname === "/api/runs") return new Response("[]");
  assert.equal(url.pathname, "/api/patch/font");
  assert.equal(init?.method, "POST");
  const body = JSON.parse(String(init?.body));
  h.bodies.push(body);
  return h.failure ? new Response(h.failure, { status: 409 }) : new Response(JSON.stringify({ output_path: body.output_path, report: {} }));
};
const { default: FontPatchDialog } = await import("./FontPatchDialog");
beforeEach(() => {
  h.states = []; h.effects = []; h.bodies = []; h.failure = ""; h.closed = 0; h.used = [];
  h.project = project; h.defaultGamePath = project.path; setLocale("en");
});
after(() => {
  hooks.deregister(); globalThis.fetch = originalFetch;
  if (windowDescriptor) Object.defineProperty(globalThis, "window", windowDescriptor);
  else Reflect.deleteProperty(globalThis, "window");
  Reflect.deleteProperty(globalThis, "__fontPatchDialog"); setLocale("en");
});
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
function render(open = true): any {
  for (let i = 0; i < 4; i++) {
    h.cursor = 0; h.changed = false;
    const tree = FontPatchDialog({ open, defaultGamePath: h.defaultGamePath, onClose() { h.closed++; }, onUsePatch(...args) { h.used.push(args); } });
    for (const effect of h.effects.splice(0)) effect();
    if (!h.changed) return tree;
  }
  throw new Error("Hook harness did not settle");
}
function button(key: "fontPatch.create" | "fontPatch.use") {
  const found = nodes(render()).find(node => node.type === "button" && text(node) === t(key));
  assert.ok(found, key);
  return found;
}
function fill() {
  for (const [index, value] of ["C:/fixture/clean-copy", "C:/fixture/replacement.ttf", "fonts/body.ttf", "es", "", "C:/fixture/font.zip"].entries()) {
    const input = nodes(render()).filter(node => node.type === "input")[index];
    input.props.onChange({ target: { value } });
  }
}
const flush = () => new Promise<void>(resolve => setImmediate(resolve));
async function submit() {
  assert.equal(button("fontPatch.create").props.disabled, false);
  button("fontPatch.create").props.onClick();
  await flush();
}
for (const native of [true, false]) {
  const mode = native ? "Tauri" : "browser";
  test(`${mode}: sends the opening project's identity with an independently selected clean game`, async () => {
    setNative(native); render(); fill(); await submit();
    assert.equal(h.bodies.length, 1);
    assert.equal(h.bodies[0].expected_database, project.database_path);
    assert.equal(h.bodies[0].game_path, "C:/fixture/clean-copy");
    button("fontPatch.use").props.onClick();
    assert.deepEqual(h.used, [["C:/fixture/font.zip", "C:/fixture/clean-copy"]]);
  });
  test(`${mode}: keeps the opening identity and form when the project changes while open`, async () => {
    setNative(native); render(); fill();
    h.project = other; h.defaultGamePath = other.path; render();
    await submit();
    assert.equal(h.bodies[0].expected_database, project.database_path);
    assert.equal(h.bodies[0].game_path, "C:/fixture/clean-copy");
  });
  test(`${mode}: reopening captures the newly selected project's identity`, async () => {
    setNative(native); render(); render(false);
    h.project = other; h.defaultGamePath = other.path; render(); fill(); await submit();
    assert.equal(h.bodies[0].expected_database, other.database_path);
  });
  test(`${mode}: sends the memory identity when the project has no database path`, async () => {
    setNative(native); h.project = { ...project, database_path: undefined }; render(); fill(); await submit();
    assert.equal(h.bodies[0].expected_database, ":memory:");
  });
  for (const locale of ["en", "es"] as const) {
    test(`${mode} ${locale}: a backend project conflict shows an inline error and keeps the dialog open`, async () => {
      setNative(native); setLocale(locale); render(); fill();
      h.failure = "project changed before font patch; retry";
      await submit();
      assert.equal(h.bodies.length, 1);
      assert.equal(h.bodies[0].expected_database, project.database_path);
      assert.ok(nodes(render()).some(node => node.props?.role === "alert" && text(node) === t("fontPatch.projectChanged")));
      assert.equal(h.closed, 0); assert.deepEqual(h.used, []);
      assert.equal(nodes(render()).filter(node => node.props?.role === "status").length, 0);
      assert.equal(button("fontPatch.create").props.disabled, false);
    });
  }
}
