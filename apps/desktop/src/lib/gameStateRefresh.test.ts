import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { QueryObserver } from "@tanstack/react-query";
import { queryClient } from "./queryClient.ts";

const originalFetch = globalThis.fetch;
const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
let api: typeof import("./api.ts");
let fail = false;
let finish: (() => void) | undefined;
let injectCalls = 0;
function completeMutation() {
  assert.ok(finish, "the real API mutation must have started");
  finish();
}
before(async () => {
  Object.defineProperty(globalThis, "window", { configurable: true, value: {
    __TAURI_INTERNALS__: { invoke: async (command: string) => {
      if (command === "get_server_port") return 7842;
      assert.equal(command, "run_inject");
      injectCalls++;
      await new Promise<void>(resolve => { finish = resolve; });
      if (fail) throw new Error("interrupted injection");
      return { mode: "direct", languages_processed: ["es"], languages_failed: [] };
    } },
  } });
  globalThis.fetch = async () => {
    await new Promise<void>(resolve => { finish = resolve; });
    return new Response(fail ? "interrupted restore" : "{}", { status: fail ? 409 : 200 });
  };
  api = await import("./api.ts");
});
after(() => {
  queryClient.clear();
  globalThis.fetch = originalFetch;
  if (windowDescriptor) Object.defineProperty(globalThis, "window", windowDescriptor);
  else Reflect.deleteProperty(globalThis, "window");
});

for (const operation of ["inject", "restore", "recover"] as const) {
  for (const rejects of [false, true]) {
    test(`${operation} settlement refreshes the editor badge (failure=${rejects})`, async () => {
      fail = rejects;
      finish = undefined;
      const key = ["patchStatus", "game", 4]; // same key prefix used by Editor's badge
      queryClient.setQueryData(key, { status: "not_patched" });
      let reads = 0;
      const observer = new QueryObserver(queryClient, {
        queryKey: key, staleTime: Infinity,
        queryFn: async () => { reads++; return { status: "not_patched" }; },
      });
      const unsubscribe = observer.subscribe(() => {});
      try {
        const pending = operation === "inject"
          ? api.inject({ project_path: "game", format_id: "html-game", languages: ["es"], direct: true })
          : operation === "restore" ? api.restoreBackup("backup") : api.recoverInjection("game", "transaction");
        // Attach rejection handling before allowing the simulated backend to settle.
        const settled = pending.then(() => null, error => error);
        for (let i = 0; !finish && i < 20; i++) await new Promise(resolve => setTimeout(resolve, 0));
        assert.ok(finish, "the real API mutation must have started");
        assert.equal(reads, 0, "do not refresh before the game mutation finishes");
        completeMutation();
        const error = await settled;
        assert.equal(error instanceof Error, rejects);
        for (let i = 0; reads === 0 && i < 20; i++) await new Promise(resolve => setTimeout(resolve, 0));
        assert.equal(reads, 1, "the badge query must refetch when the mutation settles");
        if (operation === "inject") assert.ok(injectCalls > 0);
      } finally { unsubscribe(); queryClient.clear(); }
    });
  }
}
