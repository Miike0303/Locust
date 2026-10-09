import assert from "node:assert/strict";
import { after, test } from "node:test";
import { registerHooks } from "node:module";

const harness = { setTauri: (_value: boolean) => {} };
(globalThis as any).__backupListing = harness;
const hooks = registerHooks({
  load(url, context, nextLoad) {
    if (url.replace(/\\/g, "/").endsWith("/src/lib/runtime.ts")) {
      return { format: "module", shortCircuit: true, source: `
        export let IS_TAURI=true;
        export const isTauri=()=>IS_TAURI;
        globalThis.__backupListing.setTauri=value=>{IS_TAURI=value;};
      ` };
    }
    return nextLoad(url, context);
  },
});
const calls: unknown[][] = [];
let response: unknown = null;
let failure = false;
const originalFetch = globalThis.fetch;
const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
Object.defineProperty(globalThis, "window", { configurable: true, value: {
  __TAURI_INTERNALS__: { invoke: async (command: string, args: unknown) => {
    calls.push([command, args]);
    if (command === "get_server_port") return 7842;
    if (failure) throw new Error("backup root unavailable");
    return response;
  } },
} });
globalThis.fetch = async (url, init) => {
  calls.push([String(url), init?.method ?? "GET"]);
  return new Response(failure ? "backup root unavailable" : JSON.stringify(response), { status: failure ? 500 : 200 });
};
const api = await import("./api.ts");
// Cache the desktop server URL so browser tests need no Vite environment.
await api.getCurrentProject();
after(() => {
  hooks.deregister();
  globalThis.fetch = originalFetch;
  if (windowDescriptor) Object.defineProperty(globalThis, "window", windowDescriptor);
  else Reflect.deleteProperty(globalThis, "window");
  Reflect.deleteProperty(globalThis, "__backupListing");
});

for (const tauri of [true, false]) {
  for (const entries of [[], [{ id: "healthy", source_path: "/game", file_count: 1 }]]) {
    test(`${tauri ? "Tauri" : "HTTP"} backup report preserves ${entries.length ? "mixed" : "all-damaged"} entries`, async () => {
      harness.setTauri(tauri);
      failure = false;
      response = { entries, unreadable: [{ id: "damaged", error: "manifest is unreadable" }] };
      calls.length = 0;
      assert.deepEqual(await api.getBackupsReport(), response);
      assert.deepEqual(calls, tauri
        ? [["get_backups", { report: true }]]
        : [["http://localhost:7842/api/backups?report=true", "GET"]]);
    });
  }
  test(`${tauri ? "Tauri" : "HTTP"} genuinely empty reports and legacy arrays retain their contracts`, async () => {
    harness.setTauri(tauri);
    failure = false;
    response = { entries: [], unreadable: [] };
    assert.deepEqual(await api.getBackupsReport(), response);
    response = [{ id: "healthy" }];
    calls.length = 0;
    assert.deepEqual(await api.getBackups(), response);
    assert.deepEqual(calls, tauri
      ? [["get_backups", {}]]
      : [["http://localhost:7842/api/backups", "GET"]]);
  });
  test(`${tauri ? "Tauri" : "HTTP"} fatal report failures reject instead of becoming empty`, async () => {
    harness.setTauri(tauri);
    failure = true;
    await assert.rejects(api.getBackupsReport(), /backup root unavailable/);
    failure = false;
  });
}
