import assert from "node:assert/strict";
import { after, test } from "node:test";
import { registerHooks } from "node:module";
import { renderToStaticMarkup } from "react-dom/server";
import { setLocale, t } from "../../lib/i18n";

// Exercise the real API serialization and tab callbacks with a desktop port.
const hooks = registerHooks({
  load(url, context, nextLoad) {
    if (url.replace(/\\/g, "/").endsWith("/src/lib/runtime.ts")) {
      return { format: "module", shortCircuit: true, source: "export const IS_TAURI=true;export const isTauri=()=>true;" };
    }
    return nextLoad(url, context);
  },
});
const originalFetch = globalThis.fetch;
const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
const storageDescriptor = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
const saved = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => saved.get(key) ?? null,
  setItem: (key: string, value: string) => { saved.set(key, value); },
} });
Object.defineProperty(globalThis, "window", { configurable: true, value: {
  __TAURI_INTERNALS__: { invoke: async (command: string) => {
    assert.equal(command, "get_server_port");
    return 7842;
  } },
} });
const bodies: any[] = [];
globalThis.fetch = async (_url, init) => {
  bodies.push(JSON.parse(String(init?.body)));
  return new Response(JSON.stringify({ patch_id: "test" }));
};
const { default: PatchPackTab, PatchPackActions } = await import("./PatchPackTab");
const api = await import("../../lib/api");
after(() => {
  api.setPatchCompression("default");
  hooks.deregister();
  globalThis.fetch = originalFetch;
  if (windowDescriptor) Object.defineProperty(globalThis, "window", windowDescriptor);
  else Reflect.deleteProperty(globalThis, "window");
  if (storageDescriptor) Object.defineProperty(globalThis, "localStorage", storageDescriptor);
  else Reflect.deleteProperty(globalThis, "localStorage");
  setLocale("en");
});

function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(nodes);
  return [node, ...nodes(node.props?.children)];
}
const noop = () => {};
const params = { game_path: "/synthetic", output_path: "/synthetic.zip" };
function tab() {
  return PatchPackTab({
    t, outputPath: params.output_path, setOutputPath: noop, pickOutputZip: async () => {},
    languages: "es", setLanguages: noop, recordings: ["es"], pristine: true, setPristine: noop,
    pristineRecordings: [], initialBackupId: undefined, gamePath: params.game_path, defaultGamePath: undefined,
    pristinePath: "", setPristinePath: noop, pickPristineFolder: async () => {},
    rjCode: "", rjEdited: { current: false }, setRjCode: noop, detectId: true, setDetectId: noop,
    gameVersion: "", setGameVersion: noop, createEntry: false, setCreateEntry: noop,
    entryPath: "", setEntryPathOverride: noop, pickEntry: async () => {},
    handlePack: async () => { await api.patchPack(params); }, loading: false, applying: false,
    packResult: null, patchValue: (value: string) => value, copyPublishCommand: async () => {},
  });
}

test("compression control labels and size/speed hint render in EN and ES", () => {
  for (const locale of ["en", "es"] as const) {
    setLocale(locale);
    const markup = renderToStaticMarkup(tab());
    assert.ok(markup.includes(t("patch.compression")));
    for (const label of ["patch.compressionDefault", "patch.compressionFast", "patch.compressionStore", "patch.compressionHint"] as const) {
      assert.ok(markup.includes(t(label)));
    }
    const select = nodes(tab()).find(node => node.type === "select");
    assert.equal(select.props["aria-describedby"], "patch-compression-hint");
    assert.deepEqual(select.props.children.map((option: any) => option.props.value), ["default", "fast", "store"]);
  }
});

test("Pack submits default and each selected policy; explicit API policy wins", async () => {
  api.setPatchCompression("default");
  const action = PatchPackActions({ t, recordings: ["es"], languages: "es", loading: false, applying: false,
    handlePack: async () => { await api.patchPack(params); },
  });
  assert.equal(nodes(tab()).find(node => node.type === "select").props.defaultValue, "default");
  await action.props.onClick();
  assert.equal(bodies[bodies.length - 1].compression, "default");
  for (const policy of ["fast", "store", "default"] as const) {
    await nodes(tab()).find(node => node.type === "select").props.onChange({ target: { value: policy } });
    await action.props.onClick();
    assert.equal(bodies[bodies.length - 1].compression, policy);
    assert.equal(saved.get("locust.patch.compression"), policy);
    assert.equal(nodes(tab()).find(node => node.type === "select").props.defaultValue, policy);
  }
  api.setPatchCompression("fast");
  await api.patchPack({ ...params, compression: "default" });
  assert.equal(bodies[bodies.length - 1].compression, "default");
});

test("selection remains usable when optional preference storage fails", async () => {
  Object.defineProperty(globalThis, "localStorage", { configurable: true, get() { throw new Error("unavailable"); } });
  await nodes(tab()).find(node => node.type === "select").props.onChange({ target: { value: "store" } });
  await api.patchPack(params);
  assert.equal(bodies[bodies.length - 1].compression, "store");
  api.setPatchCompression("default");
});
