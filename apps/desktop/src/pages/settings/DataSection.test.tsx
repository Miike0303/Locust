import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import type { BackupRestoreReport } from "../../lib/api.ts";
import { setLocale, t, type TranslateKey } from "../../lib/i18n/index.ts";

const backup = {
  id: "before injection", path: "/backups/before injection", source_path: "/games/Demo",
  created_at: "2026-01-01T00:00:00Z", file_count: 3, size_bytes: 100,
};
const report: BackupRestoreReport = {
  destination: backup.source_path, replaced: ["changed.bin"], recreated: ["missing.bin"],
  identical: ["same.bin"], removed: ["tl/clean.rpy"], removed_directories: ["tl/empty"],
  kept: [{ path: "save.dat", reason: "no recorded Locust output" }],
};
const label = (key: string, params?: Record<string, string | number>) => t(key as TranslateKey, params);
const h = {
  t, slots: [] as any[], cursor: 0, calls: [] as { url: string; method: string }[], toasts: [] as unknown[][],
  preview: report, failure: "", restoreFailure: "", wait: null as Promise<void> | null,
  setTauri: (_value: boolean) => {},
  state(initial: any) {
    const index = h.cursor++;
    if (!(index in h.slots)) h.slots[index] = typeof initial === "function" ? initial() : initial;
    return [h.slots[index], (value: any) => {
      h.slots[index] = typeof value === "function" ? value(h.slots[index]) : value;
    }];
  },
};
(globalThis as any).__dataPreviewTest = h;
const hooks = registerHooks({
  load(url, context, nextLoad) {
    if (url.replace(/\\/g, "/").endsWith("/src/lib/runtime.ts")) {
      return { format: "module", shortCircuit: true, source: `
        export let IS_TAURI=true;export const isTauri=()=>IS_TAURI;
        globalThis.__dataPreviewTest.setTauri=value=>{IS_TAURI=value;};
      ` };
    }
    return nextLoad(url, context);
  },
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/settings/DataSection.tsx")) {
      const stubs: Record<string, string> = {
        react: "export const useState=h.state;export const useRef=x=>h.state(()=>({current:x}))[0];",
        "@tanstack/react-query": `export const useQuery=()=>({data:{entries:[${JSON.stringify(backup)}],unreadable:[]},isPending:false,isError:false,refetch:()=>{}});`,
        "../../lib/i18n": "export const useT=()=>h.t;",
        "../../stores/toastStore": "export const addToast=(...args)=>h.toasts.push(args);",
      };
      if (stubs[specifier]) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__dataPreviewTest;" + stubs[specifier])}` };
    }
    return nextResolve(specifier, context);
  },
});
const originalFetch = globalThis.fetch;
const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
Object.defineProperty(globalThis, "window", { configurable: true, value: {
  __TAURI_INTERNALS__: { invoke: async (command: string) => {
    assert.equal(command, "get_server_port", "desktop restore preview must use HTTP too");
    return 7842;
  } },
} });
globalThis.fetch = async (url, init) => {
  h.calls.push({ url: String(url), method: init?.method ?? "GET" });
  const preview = h.preview, failure = h.failure;
  if (String(url).endsWith("?dry_run=true")) {
    if (h.wait) await h.wait;
    return new Response(failure || JSON.stringify(preview), { status: failure ? 409 : 200 });
  }
  return new Response(h.restoreFailure || JSON.stringify(report), { status: h.restoreFailure ? 409 : 200 });
};
const api = await import("../../lib/api.ts");
const { default: DataSection } = await import("./DataSection.tsx");
await api.getCurrentProject(); // Cache the desktop base URL before the browser-mode tests.

function tree() { h.cursor = 0; return DataSection(); }
function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(nodes);
  return [node, ...nodes(node.props?.children)];
}
function open() {
  const button = nodes(tree()).find(node => node.type === "button" && node.props.title === t("settings.data.restore"));
  assert.ok(button);
  return button.props.onClick();
}
function dialog() {
  const result = nodes(tree()).find(node => node.props?.open && node.props?.onConfirm);
  assert.ok(result, "restore confirmation must be open");
  return result;
}
function markup() { return renderToStaticMarkup(tree()); }
function confirmDisabled(html = markup()) {
  return /<button[^>]* disabled=""[^>]*>/.test(html);
}
function previewCalls() { return h.calls.filter(call => call.url.endsWith("?dry_run=true")); }
function writes() { return h.calls.filter(call => call.url.endsWith("/restore")); }

beforeEach(() => {
  h.slots = []; h.calls = []; h.toasts = []; h.preview = report; h.failure = ""; h.restoreFailure = ""; h.wait = null;
  h.setTauri(true); setLocale("en");
});
after(() => {
  hooks.deregister(); globalThis.fetch = originalFetch;
  if (windowDescriptor) Object.defineProperty(globalThis, "window", windowDescriptor);
  else Reflect.deleteProperty(globalThis, "window");
  Reflect.deleteProperty(globalThis, "__dataPreviewTest"); setLocale("en");
});

for (const [locale, desktop] of [["en", true], ["es", false]] as const) {
  test(`${locale} opening restore fetches an HTTP preview and bounds every category to 20 escaped paths`, async () => {
    setLocale(locale); h.setTauri(desktop);
    const paths = (category: string) => Array.from({ length: 23 }, (_, index) => `${category}-${index}<unsafe>`);
    h.preview = {
      destination: "/resolved/Demo", replaced: paths("replace"), recreated: paths("recreate"),
      identical: paths("identical"), removed: paths("remove"), removed_directories: paths("directory"),
      kept: paths("keep").map(path => ({ path, reason: "changed <output>" })),
    };
    await open();
    assert.deepEqual(h.calls, [{ url: "http://localhost:7842/api/backups/before%20injection/restore?dry_run=true", method: "POST" }]);
    const html = markup();
    assert.ok(html.includes('role="dialog"') && html.includes('aria-modal="true"'));
    assert.ok(html.includes("/resolved/Demo"), "show the destination returned by core");
    for (const key of ["replaced", "recreated", "identical", "removed", "removedDirectories", "kept"]) {
      assert.ok(html.includes(label(`settings.data.preview.${key}`, { count: 23 })));
    }
    for (const category of ["replace", "recreate", "identical", "remove", "directory", "keep"]) {
      assert.ok(html.includes(`${category}-0&lt;unsafe&gt;`) && html.includes(`${category}-19&lt;unsafe&gt;`));
      assert.ok(!html.includes(`${category}-20`) && !html.includes(`${category}-22`));
    }
    assert.equal((html.match(/<li[ >]/g) ?? []).length, 120);
    assert.equal(html.split(label("settings.data.restoreKeptMore", { count: 3 })).length - 1, 6);
    assert.ok(html.includes("changed &lt;output&gt;"));
    assert.ok(html.includes(label("settings.data.preview.advisory")));
    assert.equal(writes().length, 0);
  });
}

test("loading disables confirmation and Cancel also blocks an awaiting confirmation callback", async () => {
  let release = () => {};
  h.wait = new Promise<void>(resolve => { release = resolve; });
  const opening = open();
  assert.ok(markup().includes(label("settings.data.preview.loading")));
  assert.ok(confirmDisabled());
  const confirming = dialog().props.onConfirm();
  assert.equal(writes().length, 0);
  dialog().props.onCancel();
  release(); await opening; await confirming;
  assert.equal(writes().length, 0);
  assert.ok(!nodes(tree()).some(node => node.props?.open));
});

for (const loading of [false, true]) {
  test(`Cancel writes nothing (preview loading=${loading}) and late responses keep the dialog closed`, async () => {
    let release = () => {};
    if (loading) h.wait = new Promise<void>(resolve => { release = resolve; });
    const opening = open();
    if (!loading) await opening;
    dialog().props.onCancel();
    release(); await opening;
    assert.equal(writes().length, 0);
    assert.ok(!nodes(tree()).some(node => node.props?.open));
  });
}

test("Confirm sends the normal restore request after preview so core revalidates", async () => {
  await open();
  assert.ok(!confirmDisabled());
  await dialog().props.onConfirm();
  assert.equal(previewCalls().length, 1);
  assert.deepEqual(writes(), [{ url: "http://localhost:7842/api/backups/before%20injection/restore", method: "POST" }]);
  assert.ok(!nodes(tree()).some(node => node.props?.open));
});

test("a successful advisory preview does not hide a failure from the real restore", async () => {
  await open();
  h.restoreFailure = "invalid restore destination";
  await dialog().props.onConfirm();
  assert.equal(previewCalls().length, 1);
  assert.equal(writes().length, 1);
  assert.equal(h.toasts.length, 1);
  assert.equal(h.toasts[0][0], "error");
  assert.ok(String(h.toasts[0][1]).includes(h.restoreFailure));
  assert.ok(!nodes(tree()).some(node => node.props?.open));
});

test("an empty preview shows zero counts and enables confirmation", async () => {
  h.preview = { destination: backup.source_path, replaced: [], recreated: [], identical: [], removed: [], removed_directories: [], kept: [] };
  await open();
  const html = markup();
  for (const category of ["replaced", "recreated", "identical", "removed", "removedDirectories", "kept"]) {
    assert.ok(html.includes(label(`settings.data.preview.${category}`, { count: 0 })));
  }
  assert.ok(!confirmDisabled(html));
  await dialog().props.onConfirm();
  assert.equal(writes().length, 1);
});

for (const locale of ["en", "es"] as const) {
  test(`${locale} preview errors block confirmation, show recovery guidance, and allow retry`, async () => {
    setLocale(locale); h.failure = "game folder unavailable";
    await open();
    const html = markup();
    assert.ok(html.includes('role="alert"') && html.includes(h.failure));
    assert.ok(html.includes(label("settings.data.preview.recovery")));
    assert.ok(confirmDisabled(html));
    await dialog().props.onConfirm();
    assert.equal(writes().length, 0);
    h.failure = "";
    const retry = dialog().props.onRetry;
    assert.equal(typeof retry, "function", "the preview needs an explicit retry action");
    await retry();
    assert.equal(previewCalls().length, 2);
    assert.ok(!markup().includes('role="alert"'));
    await dialog().props.onConfirm();
    assert.equal(writes().length, 1);
  });
}

test("a cancelled preview cannot replace the report fetched on reopening", async () => {
  let release = () => {};
  h.wait = new Promise<void>(resolve => { release = resolve; });
  const oldOpening = open();
  // Let the first request enter fetch before opening a second preview.
  for (let attempt = 0; previewCalls().length === 0 && attempt < 10; attempt++) await Promise.resolve();
  assert.equal(previewCalls().length, 1);
  dialog().props.onCancel();
  h.wait = null; h.preview = { ...report, replaced: ["new-preview.bin"] };
  await open();
  release(); await oldOpening;
  assert.ok(markup().includes("new-preview.bin"));
  assert.equal(writes().length, 0);
});
