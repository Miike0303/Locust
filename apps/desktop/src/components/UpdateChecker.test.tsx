import assert from "node:assert/strict";
import { after, beforeEach, test } from "node:test";
import { registerHooks } from "node:module";
import { setLocale, t } from "../lib/i18n";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const h = {
  t, states: [] as any[], cursor: 0, effects: [] as (() => void)[], logs: [] as any[],
  checks: 0, downloads: 0, relaunches: 0,
  check: null as null | (() => Promise<any>), install: null as null | (() => Promise<void>),
  restart: null as null | (() => Promise<void>), event: null as null | ((event: any) => void),
  state(initial: any) {
    const i = this.cursor++;
    if (!(i in this.states)) this.states[i] = typeof initial === "function" ? initial() : initial;
    return [this.states[i], (value: any) => { this.states[i] = typeof value === "function" ? value(this.states[i]) : value; }];
  },
  effect(fn: () => void, deps: unknown[]) {
    const i = this.cursor++;
    if (!(i in this.states)) { this.states[i] = deps; this.effects.push(fn); }
  },
};
(globalThis as any).__updater = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/components/UpdateChecker.tsx")) {
      const stubs: Record<string, string> = {
        react: "export const useState=initial=>h.state(initial);export const useRef=initial=>h.state(()=>({current:initial}))[0];export const useEffect=(fn,deps)=>h.effect(fn,deps);",
        "../lib/runtime": "export const IS_TAURI=true;",
        "../lib/i18n": "export const t=h.t;export const useT=()=>h.t;",
        "../stores/logStore": "export const addLog=(...args)=>h.logs.push(args);",
        "@tauri-apps/plugin-updater": "export const check=async()=>{h.checks++;return h.check?h.check():{version:'9.0.0',body:'Release notes',downloadAndInstall:async event=>{h.downloads++;h.event=event;await h.install?.();}};};",
        "@tauri-apps/plugin-process": "export const relaunch=async()=>{h.relaunches++;await h.restart?.();};",
      };
      if (stubs[specifier]) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__updater;" + stubs[specifier])}` };
    }
    return nextResolve(specifier, context);
  },
});
const { default: UpdateChecker, triggerUpdateCheck } = await import("./UpdateChecker");
after(() => { hooks.deregister(); Reflect.deleteProperty(globalThis, "__updater"); setLocale("en"); });
beforeEach(() => {
  h.states = []; h.cursor = 0; h.effects = []; h.logs = [];
  h.checks = 0; h.downloads = 0; h.relaunches = 0;
  h.check = null; h.install = null; h.restart = null; h.event = null;
  setLocale("en");
});
function render(): any {
  h.cursor = 0;
  const tree = UpdateChecker();
  const effects = h.effects; h.effects = [];
  effects.forEach(effect => effect());
  return tree;
}
function nodes(tree: any): any[] {
  if (tree == null || typeof tree !== "object") return [];
  if (Array.isArray(tree)) return tree.flatMap(nodes);
  return [tree, ...nodes(tree.props?.children)];
}
function text(tree: any): string {
  if (tree == null || typeof tree === "boolean") return "";
  if (typeof tree === "string" || typeof tree === "number") return String(tree);
  if (Array.isArray(tree)) return tree.map(text).join("");
  return text(tree.props?.children);
}
function button(tree: any, label: string) {
  const node = nodes(tree).find(node => node.type === "button" && text(node) === label);
  assert.ok(node, `button exists: ${label}`);
  return node;
}
async function flush() {
  for (let i = 0; i < 4; i++) await new Promise(resolve => setImmediate(resolve));
}
async function available() { render(); await flush(); return render(); }

test("repeated clicks synchronously claim one operation before the second check resolves", async () => {
  const tree = await available();
  const pending = deferred<any>();
  h.check = () => pending.promise;
  const click = button(tree, t("update.download")).props.onClick;
  const operations = [click(), click(), click()];
  await flush();
  const checks = h.checks;
  const busyText = text(render());
  pending.resolve({ downloadAndInstall: async () => { h.downloads++; } });
  await Promise.all(operations);
  assert.equal(checks, 2, "one mount check and one download check");
  assert.ok(busyText.includes(t("update.downloading")));
  assert.equal(h.downloads, 1);
  assert.equal(h.relaunches, 1);
});

test("download Finished means installing; only a resolved installation means installed", async () => {
  const pending = deferred<void>();
  const restarting = deferred<void>();
  h.install = () => pending.promise;
  h.restart = () => restarting.promise;
  const operation = button(await available(), t("update.download")).props.onClick();
  await flush();
  h.event!({ event: "Started", data: { contentLength: 10 } });
  h.event!({ event: "Progress", data: { chunkLength: 10 } });
  assert.ok(!text(render()).includes(t("update.installed")));
  h.event!({ event: "Finished" });
  const duringInstall = text(render());
  pending.resolve();
  await flush();
  const afterInstall = text(render());
  restarting.resolve();
  await operation;
  assert.ok(!duringInstall.includes(t("update.installed")));
  assert.ok(duringInstall.includes(t("update.installing" as any)));
  assert.ok(afterInstall.includes(t("update.installed")));
});

test("an install rejection after Finished never relaunches", async () => {
  const pending = deferred<void>(); h.install = () => pending.promise;
  const operation = button(await available(), t("update.download")).props.onClick();
  await flush(); h.event!({ event: "Finished" });
  pending.reject(new Error("installer permission denied"));
  await operation;
  assert.equal(h.relaunches, 0);
  assert.ok(!text(render()).includes(t("update.installed")));
  assert.equal(h.logs[0]?.[2], "installer permission denied");
});

for (const locale of ["en", "es"] as const) {
  for (const phase of ["check", "install"] as const) {
    test(`${phase} failures use ${locale} guidance, log diagnostics and allow retry`, async () => {
      setLocale(locale);
      const tree = await available();
      const raw = `${phase} opaque plugin failure`;
      if (phase === "check") h.check = async () => { throw new Error(raw); };
      else h.install = async () => { throw new Error(raw); };
      await button(tree, t("update.download")).props.onClick();
      const failed = render();
      const guidance = locale === "es"
        ? "No se pudo instalar la actualización. Revise los detalles de actividad y vuelva a intentarlo."
        : "Could not install the update. Review the activity details and retry.";
      assert.ok(text(failed).includes(guidance));
      assert.ok(!text(failed).includes(raw));
      assert.equal(h.logs[0]?.[2], raw);
      assert.equal(h.relaunches, 0);
      h.check = null; h.install = null;
      await button(failed, t("common.retry")).props.onClick();
      assert.equal(h.relaunches, 1);
    });
  }
  test(`manual update check failures localize in ${locale} and log details`, async () => {
    setLocale(locale);
    const raw = "raw update service failure";
    h.check = async () => { throw new Error(raw); };
    const result = await triggerUpdateCheck();
    assert.equal(result.kind, "error");
    assert.ok(!JSON.stringify(result).includes(raw));
    assert.equal(h.logs[0]?.[2], raw);
  });
}

test("silent check failures stay in the activity log and a later check can succeed", async () => {
  h.check = async () => { throw new Error("offline"); };
  render(); await flush();
  assert.equal(render(), null);
  assert.equal(h.logs[0]?.[2], "offline");
  h.check = null;
  assert.equal((await triggerUpdateCheck()).kind, "available");
});

test("an update disappearing during recheck releases ownership and reports up to date", async () => {
  const tree = await available();
  h.check = async () => null;
  await button(tree, t("update.download")).props.onClick();
  assert.ok(text(render()).includes(t("update.upToDate")));
  assert.equal(h.downloads, 0);
  assert.equal(h.relaunches, 0);
});

test("a restart failure retries only relaunch, without reinstalling", async () => {
  h.restart = async () => { throw new Error("restart blocked"); };
  await button(await available(), t("update.download")).props.onClick();
  assert.equal(h.logs[0]?.[2], "restart blocked");
  h.restart = null;
  await button(render(), t("update.restart" as any)).props.onClick();
  assert.equal(h.downloads, 1);
  assert.equal(h.relaunches, 2);
});
