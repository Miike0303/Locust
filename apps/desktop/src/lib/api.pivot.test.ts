import assert from "node:assert/strict";
import { after, test } from "node:test";
import { registerHooks } from "node:module";

const h = { setTauri: (_value: boolean) => {}, calls: [] as unknown[][] };
(globalThis as any).__pivotApi = h;
const hooks = registerHooks({
  load(url, context, nextLoad) {
    if (url.replace(/\\/g, "/").endsWith("/src/lib/runtime.ts")) {
      return { format: "module", shortCircuit: true, source: `
        export let IS_TAURI=true;
        export const isTauri=()=>IS_TAURI;
        globalThis.__pivotApi.setTauri=value=>{IS_TAURI=value;};` };
    }
    return nextLoad(url, context);
  },
});
const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
const originalFetch = globalThis.fetch;
const result = { database_path: "synthetic/pivot.db", entries: 2 };
Object.defineProperty(globalThis, "window", { configurable: true, value: {
  __TAURI_INTERNALS__: { invoke: async (command: string, args: unknown) => {
    if (command === "get_server_port") return 7842;
    h.calls.push([command, args]);
    return result;
  } },
} });
globalThis.fetch = async (url, init) => {
  h.calls.push([String(url), init?.method, JSON.parse(String(init?.body ?? "null"))]);
  return new Response(JSON.stringify(result));
};
const api = await import("./api");
await api.getCurrentProject();
after(() => {
  hooks.deregister();
  globalThis.fetch = originalFetch;
  if (windowDescriptor) Object.defineProperty(globalThis, "window", windowDescriptor);
  else Reflect.deleteProperty(globalThis, "window");
  Reflect.deleteProperty(globalThis, "__pivotApi");
});
for (const tauri of [true, false]) {
  for (const strict of [undefined, false, true]) {
    test(`${tauri ? "Tauri" : "HTTP"} pivot sends ${String(strict)} with correct casing and legacy omission`, async () => {
      h.setTauri(tauri);
      h.calls = [];
      // Exercise the actual API and actual Tauri invoke/fetch transport boundary.
      const value = strict === undefined
        ? await api.runPivot("synthetic/pivot.db")
        : await api.runPivot("synthetic/pivot.db", strict);
      assert.deepEqual(value, result);
      const args: Record<string, unknown> = tauri
        ? { outputPath: "synthetic/pivot.db" }
        : { output_path: "synthetic/pivot.db" };
      if (strict !== undefined) args[tauri ? "requireComplete" : "require_complete"] = strict;
      assert.deepEqual(h.calls, [tauri
        ? ["run_pivot", args]
        : ["http://localhost:7842/api/pivot", "POST", args]]);
    });
  }
}
