import assert from "node:assert/strict";
import { after, test } from "node:test";
import { registerHooks } from "node:module";

const h = { tauri: true, shellOpens: [] as string[], windowOpens: [] as unknown[][] };
(globalThis as any).__externalUrl = h;
(globalThis as any).window = { open: (...args: unknown[]) => { h.windowOpens.push(args); return null; } };
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.includes("/lib/externalUrl.ts")) {
      const stubs: Record<string, string> = {
        // Resolved per import, so each query-string copy gets its own runtime.
        "./runtime": `export const IS_TAURI=${h.tauri};`,
        "@tauri-apps/plugin-shell": "export const open=async url=>{globalThis.__externalUrl.shellOpens.push(url);};",
      };
      if (stubs[specifier]) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(stubs[specifier])}` };
    }
    return nextResolve(specifier, context);
  },
});
// One module copy per runtime: the query string makes Node load it twice.
type ExternalUrl = typeof import("./externalUrl");
const modulePath = "./externalUrl.ts";
const desktop = (await import(`${modulePath}?desktop`)) as ExternalUrl;
h.tauri = false;
const browser = (await import(`${modulePath}?browser`)) as ExternalUrl;
after(() => {
  hooks.deregister();
  Reflect.deleteProperty(globalThis, "__externalUrl");
  Reflect.deleteProperty(globalThis, "window");
});

test("the desktop app opens external pages through the shell plugin, not window.open", async () => {
  h.shellOpens = []; h.windowOpens = [];
  await desktop.openExternalUrl("https://fonts.google.com/noto/specimen/Noto+Sans+JP");
  assert.deepEqual(h.shellOpens, ["https://fonts.google.com/noto/specimen/Noto+Sans+JP"]);
  assert.deepEqual(h.windowOpens, []);
});

test("the browser UI opens external pages in a new tab without an opener", async () => {
  h.shellOpens = []; h.windowOpens = [];
  await browser.openExternalUrl("https://example.org/font");
  assert.deepEqual(h.windowOpens, [["https://example.org/font", "_blank", "noopener,noreferrer"]]);
  assert.deepEqual(h.shellOpens, []);
});
