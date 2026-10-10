import assert from "node:assert/strict";
import { after, test } from "node:test";
import { registerHooks } from "node:module";
import { renderToStaticMarkup } from "react-dom/server";
import { setLocale, t } from "./i18n/index.ts";

const backup = {
  id: "before injection", path: "/backups/before injection", source_path: "/games/Demo",
  created_at: "2026-01-01T00:00:00Z", file_count: 3, size_bytes: 100,
};
const restoreReport = {
  destination: backup.source_path,
  replaced: ["game/script.rpy"], recreated: ["game/options.rpy"], identical: ["game/gui.rpy"],
  removed: ["game/tl/es/unchanged.rpy"], removed_directories: ["game/tl/unused"],
  kept: [{ path: "game/tl/es/edited.rpy", reason: "SHA256 differs from recorded Locust output" }],
};
let response = restoreReport;
let failed = false;
const calls: unknown[][] = [];

// Exercise Settings' real restore/confirmation callbacks and API client, using
// the same lifecycle-only hook harness as the modal interaction tests.
const h = {
  t, values: [] as unknown[], cursor: 0, toasts: [] as unknown[][],
  listing: { entries: [backup], unreadable: [] },
  setTauri: (_value: boolean) => {},
  state(initial: unknown) {
    const index = this.cursor++;
    if (!(index in this.values)) this.values[index] = typeof initial === "function" ? initial() : initial;
    return [this.values[index], (value: any) => {
      this.values[index] = typeof value === "function" ? value(this.values[index]) : value;
    }];
  },
};
(globalThis as any).__backupRestore = h;
const hooks = registerHooks({
  load(url, context, nextLoad) {
    if (url.replace(/\\/g, "/").endsWith("/src/lib/runtime.ts")) {
      return { format: "module", shortCircuit: true, source: `
        export let IS_TAURI=true;
        export const isTauri=()=>IS_TAURI;
        globalThis.__backupRestore.setTauri=value=>{IS_TAURI=value;};
      ` };
    }
    return nextLoad(url, context);
  },
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/pages/Settings.tsx")) {
      const stubs: Record<string, string> = {
        react: `const h=globalThis.__backupRestore;export const useState=initial=>h.state(initial);export const useMemo=fn=>fn();export const useEffect=()=>{};`,
        "@tanstack/react-query": `export const useQuery=()=>({data:globalThis.__backupRestore.listing,isPending:false,isError:false,refetch:()=>{}});export const useQueryClient=()=>({});`,
        "react-router-dom": `export const useLocation=()=>({search:'?section=data'});export const useNavigate=()=>()=>{};`,
        "../lib/i18n": `export const useT=()=>globalThis.__backupRestore.t;export const useLocale=()=> 'en';`,
        "../stores/toastStore": `export const addToast=(...args)=>globalThis.__backupRestore.toasts.push(args);`,
        "../components/ConfirmDialog": `export default function ConfirmDialog(){return null;}`,
      };
      if (stubs[specifier]) return { url: `data:text/javascript,${encodeURIComponent(stubs[specifier])}`, shortCircuit: true };
    }
    return nextResolve(specifier, context);
  },
});
const originalFetch = globalThis.fetch;
const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
Object.defineProperty(globalThis, "window", { configurable: true, value: {
  __TAURI_INTERNALS__: { invoke: async (command: string) => {
    assert.equal(command, "get_server_port", "restore uses the HTTP server in desktop mode too");
    return 7842;
  } },
} });
globalThis.fetch = async (url, init) => {
  calls.push([String(url), init?.method ?? "GET"]);
  return new Response(failed ? "restore unavailable" : JSON.stringify(response), { status: failed ? 500 : 200 });
};
const api = await import("./api.ts");
const { default: Settings } = await import("../pages/Settings.tsx");
// Memoize the desktop URL before switching runtime modes (no Vite in Node).
await api.getCurrentProject();
after(() => {
  hooks.deregister();
  globalThis.fetch = originalFetch;
  if (windowDescriptor) Object.defineProperty(globalThis, "window", windowDescriptor);
  else Reflect.deleteProperty(globalThis, "window");
  Reflect.deleteProperty(globalThis, "__backupRestore");
  setLocale("en");
});

function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(nodes);
  return [node, ...nodes(node.props?.children)];
}
function render() {
  h.cursor = 0;
  const section = nodes(Settings()).find(node => typeof node.type === "function");
  assert.ok(section, "Settings must render its Data section");
  return section.type(section.props);
}
function reset(locale: "en" | "es") {
  h.values = []; h.toasts = []; calls.length = 0;
  response = restoreReport; failed = false;
  setLocale(locale);
}
async function restore() {
  const button = nodes(render()).find(node => node.type === "button" && node.props.title === t("settings.data.restore"));
  assert.ok(button, "restore action must be available");
  button.props.onClick();
  const confirmation = nodes(render()).find(node => node.props?.open && node.props?.onConfirm);
  assert.ok(confirmation, "restore must still require confirmation");
  await confirmation.props.onConfirm();
  return renderToStaticMarkup(render());
}

for (const desktop of [true, false]) {
  test(`${desktop ? "desktop" : "browser"} HTTP restore returns the parsed report and encodes its ID`, async () => {
    reset("en");
    h.setTauri(desktop);
    const result: typeof restoreReport = await api.restoreBackup(backup.id);
    assert.deepEqual(result, restoreReport);
    assert.deepEqual(calls, [["http://localhost:7842/api/backups/before%20injection/restore", "POST"]]);
  });
}

for (const [locale, restoredText, keptText, moreText] of [
  ["en", "Backup before injection restored. Restored files: 3.", "Some paths were kept in the game. See the reasons below.", "and 2 more"],
  ["es", "Copia de seguridad before injection restaurada. Archivos restaurados: 3.", "Se conservaron algunas rutas en el juego. Consulte los motivos a continuación.", "y 2 más"],
] as const) {
  test(`${locale} restoring a backup with an edited .rpy shows a visible warning with its reason and count`, async () => {
    reset(locale);
    const html = await restore();
    assert.ok(html.includes('role="alert"'), "kept files need an accessible warning");
    assert.ok(html.includes("text-warning") && html.includes("bg-warning-muted"), "warning uses semantic colors");
    for (const text of [restoredText, keptText, restoreReport.kept[0].path, restoreReport.kept[0].reason]) {
      assert.ok(html.includes(text), `missing restore detail: ${text}`);
    }
    assert.ok(!h.toasts.some(([type]) => type === "success"), "partial cleanup must not be reported as plain success");
  });

  test(`${locale} restoring with nothing kept shows plain success with the actual restored count`, async () => {
    reset(locale);
    await restore();
    h.toasts = [];
    response = { ...restoreReport, kept: [] };
    const html = await restore();
    assert.deepEqual(h.toasts, [["success", restoredText]]);
    assert.ok(!html.includes('role="alert"') && !html.includes(keptText), "a new clean restore clears the old warning");
    assert.ok(!html.includes(restoreReport.kept[0].path));
  });

  test(`${locale} kept paths are bounded to ten, escaped, and report the remaining count`, async () => {
    reset(locale);
    response = { ...restoreReport, kept: Array.from({ length: 12 }, (_, index) => ({
      path: `game/tl/es/edited-${index}<unsafe>.rpy`, reason: `changed <output> ${index}`,
    })) };
    const html = await restore();
    assert.ok(html.includes("game/tl/es/edited-0&lt;unsafe&gt;.rpy"));
    assert.ok(html.includes("changed &lt;output&gt; 0"));
    assert.ok(html.includes("game/tl/es/edited-9&lt;unsafe&gt;.rpy"));
    assert.ok(!html.includes("edited-10") && !html.includes("edited-11"));
    assert.equal((html.match(/<li[ >]/g) ?? []).length, 10);
    assert.ok(html.includes(moreText), "overflow count must be localized");
  });
}

test("a failed restore clears the previous report and keeps the failure visible", async () => {
  reset("en");
  await restore();
  h.toasts = [];
  failed = true;
  const html = await restore();
  assert.ok(!html.includes(restoreReport.kept[0].path), "do not leave a stale completed report after failure");
  assert.equal(h.toasts.length, 1);
  assert.equal(h.toasts[0][0], "error");
  assert.match(String(h.toasts[0][1]), /restore unavailable/);
});
