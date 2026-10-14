import assert from "node:assert/strict";
import { after, test } from "node:test";
import { registerHooks } from "node:module";
import { setLocale, t } from "../lib/i18n";
import type { ProjectInfo } from "../lib/api";

const project: ProjectInfo = {
  path: "synthetic-game", format_id: "renpy", name: "Fixture",
  database_path: "synthetic/source.db", supported_modes: ["replace"],
};
const h = {
  t, states: [] as any[], cursor: 0, closed: 0, project,
  setTauri: (_value: boolean) => {}, calls: [] as any[],
  toasts: [] as any[], logs: [] as any[], changed: [] as any[],
  refusal: "", output: "synthetic/pivot.db" as string | null,
};
(globalThis as any).__pivotModal = h;
const hooks = registerHooks({
  load(url, context, nextLoad) {
    if (url.replace(/\\/g, "/").endsWith("/src/lib/runtime.ts")) return {
      format: "module", shortCircuit: true, source: `
        export let IS_TAURI=true;
        export const isTauri=()=>IS_TAURI;
        globalThis.__pivotModal.setTauri=value=>{IS_TAURI=value;};`,
    };
    return nextLoad(url, context);
  },
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/components/PivotModal.tsx")) {
      const stubs: Record<string, string> = {
        react: `export const useEffect=()=>{};
          export const useState=initial=>{const i=h.cursor++;if(!(i in h.states))h.states[i]=initial;return [h.states[i],value=>{h.states[i]=value;}];};`,
        "@tanstack/react-query": "export const useQueryClient=()=>({});",
        "../lib/i18n": "export const useT=()=>h.t;",
        "../stores/projectStore": "export const useProjectStore=()=>({project:h.project,setProject:value=>h.changed.push(value)});",
        "../stores/toastStore": "export const addToast=(...args)=>h.toasts.push(args);",
        "../stores/logStore": "export const addLog=(...args)=>h.logs.push(args);",
        "../lib/modalA11y": "export const useModalA11y=()=>({});export const MODAL_BACKDROP_CLASS='';export const MODAL_FOOTER_CLASS='';export const modalPanelClass=()=>'';",
      };
      if (stubs[specifier]) return {
        shortCircuit: true,
        url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__pivotModal;" + stubs[specifier])}`,
      };
    }
    return nextResolve(specifier, context);
  },
});
const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
const originalFetch = globalThis.fetch;
const originalPrompt = globalThis.prompt;
Object.defineProperty(globalThis, "window", { configurable: true, value: {
  __TAURI_INTERNALS__: { invoke: async (command: string, args: any) => {
    if (command === "get_server_port") return 7842;
    if (command === "plugin:dialog|save") return h.output;
    assert.equal(command, "run_pivot", "no open-db call during creation or refusal");
    h.calls.push([command, args]);
    if (h.refusal) throw h.refusal;
    return { database_path: h.output, entries: 2 };
  } },
} });
globalThis.prompt = () => h.output;
globalThis.fetch = async (input, init) => {
  const url = new URL(String(input));
  if (url.pathname === "/api/project/current") return new Response("null");
  assert.equal(url.pathname, "/api/pivot");
  assert.equal(init?.method, "POST");
  h.calls.push(["/api/pivot", JSON.parse(String(init?.body))]);
  return h.refusal
    ? new Response(h.refusal, { status: 400 })
    : new Response(JSON.stringify({ database_path: h.output, entries: 2 }));
};
const { default: PivotModal } = await import("./PivotModal");
await (await import("../lib/api")).getCurrentProject();
after(() => {
  hooks.deregister();
  globalThis.fetch = originalFetch;
  globalThis.prompt = originalPrompt;
  if (windowDescriptor) Object.defineProperty(globalThis, "window", windowDescriptor);
  else Reflect.deleteProperty(globalThis, "window");
  Reflect.deleteProperty(globalThis, "__pivotModal");
  setLocale("en");
});
function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(nodes);
  return [node, ...nodes(node.props?.children)];
}
function hasText(node: any, text: string): boolean {
  if (node === text) return true;
  if (Array.isArray(node)) return node.some(child => hasText(child, text));
  return !!node && typeof node === "object" && hasText(node.props?.children, text);
}
function render(count: number | null = 1, totalEntries: number | null = count === null ? null : 2) {
  h.cursor = 0;
  return PivotModal({ open: true, onClose() { h.closed++; }, carryOverCount: count, totalEntries });
}
function reset(locale: "en" | "es", tauri: boolean) {
  h.states = []; h.calls = []; h.toasts = []; h.logs = []; h.changed = []; h.closed = 0;
  h.refusal = ""; h.output = "synthetic/pivot.db";
  setLocale(locale); h.setTauri(tauri);
}
async function create(count: number | null = 1) {
  const button = nodes(render(count)).find(n => n.type === "button" && hasText(n, t("pivot.create")));
  assert.ok(button);
  assert.equal(button.props.disabled, false);
  button.props.onClick();
  await new Promise(resolve => setImmediate(resolve));
}
function assertStrict(tauri: boolean) {
  assert.deepEqual(h.calls, [tauri
    ? ["run_pivot", { outputPath: h.output, requireComplete: true }]
    : ["/api/pivot", { output_path: h.output, require_complete: true }]]);
}
function assertStatusCount(count: number, locale: "en" | "es", totalEntries = 2) {
  const view = render(count, totalEntries);
  const expected = locale === "en"
    ? `Entries marked translated, reviewed or approved: ${count} of ${totalEntries}.`
    : `Entradas marcadas como traducidas, revisadas o aprobadas: ${count} de ${totalEntries}.`;
  assert.ok(hasText(view, expected));
  const copy = nodes(view).filter(n => n.type === "p").map(n => n.props.children).join(" ");
  assert.doesNotMatch(copy, /will carry over|Se transferir[áa]n?/i);
}
for (const locale of ["en", "es"] as const) {
  test(`${locale}: explains all rows must be translated, current and control-valid instead of skipped`, () => {
    reset(locale, false);
    const expected = locale === "en"
      ? "All entries must have a non-blank translation, be translated, reviewed or approved, match the current source, and preserve protected controls. If any entry fails these checks, no pivot database is created."
      : "Todas las entradas deben tener una traducción no vacía, estar traducidas, revisadas o aprobadas, corresponder al texto fuente actual y conservar los controles protegidos. Si alguna entrada no cumple estas condiciones, no se crea la base de datos puente.";
    assert.ok(nodes(render()).some(n => n.props?.children === expected));
  });
  test(`${locale}: status counts report their total without promising carry-over or validation`, () => {
    reset(locale, false);
    for (const count of [0, 1, 2]) {
      assertStatusCount(count, locale);
      const button = nodes(render(count)).find(n => n.type === "button" && hasText(n, t("pivot.create")));
      assert.equal(button.props.disabled, false, "status counts cannot decide strict validity");
    }
    assertStatusCount(2, locale, 7);
  });
  test(`${locale}: unavailable status count or total hides the count without blocking strict validation`, () => {
    reset(locale, false);
    for (const [count, totalEntries] of [[null, 2], [1, null], [null, null]]) {
      const view = render(count, totalEntries);
      const prefix = locale === "en" ? "Entries marked" : "Entradas marcadas";
      assert.ok(!nodes(view).some(n => n.type === "p" && String(n.props.children).startsWith(prefix)));
      const button = nodes(view).find(n => n.type === "button" && hasText(n, t("pivot.create")));
      assert.equal(button.props.disabled, false);
    }
  });
  for (const tauri of [true, false]) {
    const label = `${locale} ${tauri ? "Tauri" : "HTTP"}`;
    for (const count of [1, 2]) {
      test(`${label}: refusal with ${count} of 2 status-marked rows stays on source without a carry-over promise`, async () => {
        reset(locale, tauri);
        const reason = count === 1 ? "missing or blank translation" : "changed protected controls";
        h.refusal = `incomplete pivot: entry 'z' has ${reason}`;
        await create(count);
        assertStrict(tauri);
        assertStatusCount(count, locale);
        assert.equal(h.changed.length, 0);
        assert.equal(h.closed, 0);
        assert.equal(h.toasts.length, 1);
        assert.equal(h.toasts[0][0], "error");
        assert.ok(nodes(render(count)).some(n => n.type === "p" && String(n.props.children).includes(reason)));
        assert.ok(!hasText(render(count), t("pivot.openNow")));
        assert.ok(h.logs[0][2].includes(h.refusal), "refusal remains in existing diagnostic wrapper");
      });
    }
    test(`${label}: complete success requests strict even when count is unknown and offers explicit open`, async () => {
      reset(locale, tauri);
      await create(null);
      assertStrict(tauri);
      assert.equal(h.toasts[0][0], "success");
      assert.ok(hasText(render(), t("pivot.openNow")));
      assert.ok(nodes(render()).some(n => n.props?.children === t("pivot.stay")));
      assert.equal(h.changed.length, 0, "creation never replaces current project automatically");
      assert.equal(h.closed, 0);
    });
    test(`${label}: cancelling the destination never dispatches`, async () => {
      reset(locale, tauri); h.output = null;
      await create();
      assert.equal(h.calls.length, 0);
      assert.equal(h.toasts.length, 0);
    });
  }
}
test("zero status-marked rows still dispatch strict validation instead of using an approximate guard", async () => {
  reset("en", false);
  h.refusal = "incomplete pivot: entry 'z' has a missing or blank translation";
  await create(0);
  assertStrict(false);
  assertStatusCount(0, "en");
  assert.equal(h.toasts[0][0], "error");
  assert.equal(h.changed.length, 0);
});
