import assert from "node:assert/strict";
import { test } from "node:test";
import { registerHooks } from "node:module";
import { ApiError } from "./apiError";
import { setLocale, t } from "./i18n";

const h = { setTauri: (_value: boolean) => {}, calls: [] as any[], reject: false };
(globalThis as any).__guardedApi = h;
registerHooks({
  load(url, context, nextLoad) {
    if (url.replace(/\\/g, "/").endsWith("/src/lib/runtime.ts")) {
      return { format: "module", shortCircuit: true, source: `
        export let IS_TAURI=true;export const isTauri=()=>IS_TAURI;
        globalThis.__guardedApi.setTauri=value=>{IS_TAURI=value;};` };
    }
    return nextLoad(url, context);
  },
});
Object.defineProperty(globalThis, "window", { configurable: true, value: {
  __TAURI_INTERNALS__: { invoke: async (command: string, args: any) => {
    if (command === "get_server_port") return 7842;
    h.calls.push([command, args]);
    if (h.reject) throw "translation changed since it was loaded";
    return { translation: args?.data?.translation };
  } },
} });
globalThis.fetch = async (url, init) => {
  h.calls.push([String(url), init?.method, init?.body ? JSON.parse(String(init.body)) : null]);
  return h.reject
    ? new Response("translation changed since it was loaded", { status: 409 })
    : new Response(JSON.stringify({ translation: "saved" }));
};
const api = await import("./api");
// Resolve base URL using the desktop port seam before switching transports.
await api.getCurrentProject();
for (const tauri of [true, false]) {
  test(`${tauri ? "Tauri" : "HTTP"} preserves text/null/absent guards and legacy status-only requests`, async () => {
    h.setTauri(tauri); h.calls = []; h.reject = false;
    const bodies = [
      { translation: "draft", expected_translation: "V0" },
      { translation: "draft", expected_translation: null },
      { translation: "draft", expected_translation: "" },
      { translation: "legacy" },
      { status: "approved" as const },
    ];
    for (const body of bodies) await api.patchString("row/id", body);
    assert.deepEqual(h.calls, bodies.map(body => tauri
      ? ["patch_string", { id: "row/id", data: body }]
      : ["http://localhost:7842/api/strings/row%2Fid", "PATCH", body]));
  });
  test(`${tauri ? "Tauri" : "HTTP"} gives conflicts a stable key and EN/ES messages`, async () => {
    h.setTauri(tauri); h.reject = true;
    for (const locale of ["en", "es"] as const) {
      setLocale(locale);
      await assert.rejects(api.patchString("row", { translation: "draft", expected_translation: "V0" }), error => {
        assert.ok(error instanceof ApiError);
        assert.equal(error.key, "api.error.translationConflict");
        assert.equal(error.message, t("api.error.translationConflict"));
        return true;
      });
    }
  });
}
