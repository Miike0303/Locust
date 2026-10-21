import assert from "node:assert/strict";
import { after, test } from "node:test";
import { registerHooks } from "node:module";
import { ApiError } from "./apiError";

const h = { setTauri: (_value: boolean) => {}, calls: [] as unknown[][], estimateStatus: 200 };
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
  if (String(url).endsWith("/translate/estimate")) {
    return h.estimateStatus === 200
      ? new Response(JSON.stringify(estimateResult))
      : new Response("no project open", { status: h.estimateStatus });
  }
  return new Response(JSON.stringify(result));
};
const estimateResult = {
  provider_id: "fixture", effective_model: "model",
  context_window: "18446744073709551615", approximate_input_allowance: "13835058055282163712",
  max_batch_tokens: "13835058055282163712", batch_size: 40, entry_count: 2, source_bytes: 11,
  estimated_cost_usd: null, cost_known: false, approximate: true,
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
  test(`${tauri ? "Tauri embedded" : "HTTP"} estimate uses HTTP only and preserves lossless metadata/options`, async () => {
    h.setTauri(tauri);
    h.estimateStatus = 200;
    for (const cap of [undefined, null, 300]) {
      const params = { provider_id: "fixture", options: {
        source_lang: "ja", target_lang: "es", cost_limit_usd: null, game_context: null,
        use_glossary: true, use_memory: true, skip_approved: true,
        ...(cap === undefined ? {} : { max_batch_tokens: cap }),
      } };
      h.calls = [];
      const value: import("./api").TranslationEstimate = await api.estimateTranslation(params);
      assert.deepEqual(value, estimateResult);
      assert.deepEqual(h.calls, [["http://localhost:7842/api/translate/estimate", "POST", params]]);
    }
    h.estimateStatus = 400;
    await assert.rejects(api.estimateTranslation({ provider_id: "fixture", options: {
      source_lang: "ja", target_lang: "es", cost_limit_usd: null, game_context: null,
      use_glossary: true, use_memory: true, skip_approved: true,
    } }), error => {
      assert.ok(error instanceof ApiError);
      assert.equal(error.key, "api.error.noProjectOpen");
      return true;
    });
    h.estimateStatus = 200;
  });
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
