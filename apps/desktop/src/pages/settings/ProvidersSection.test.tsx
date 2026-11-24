import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import { setLocale, t, type TranslateKey } from "../../lib/i18n/index.ts";

const h = {
  t,
  config: { providers: {} as Record<string, Record<string, string>> },
  providers: [{ id: "openai", name: "OpenAI", is_free: false, requires_api_key: true, configured: true }],
  saves: [] as unknown[],
  health: async (_id: string): Promise<{ ok: boolean; message: string }> => ({ ok: true, message: "" }),
  states: [] as any[], cursor: 0, changed: false,
  state(initial: any) {
    const index = this.cursor++;
    if (!(index in this.states)) this.states[index] = typeof initial === "function" ? initial() : initial;
    return [this.states[index], (value: any) => {
      const next = typeof value === "function" ? value(this.states[index]) : value;
      if (!Object.is(next, this.states[index])) this.changed = true;
      this.states[index] = next;
    }];
  },
  query(options: any) {
    const data = options.queryKey[0] === "config" ? h.config : h.providers;
    return { data, isError: false, isPending: false, error: null, refetch: async () => {} };
  },
};
(globalThis as any).__providersSection = h;
// Same hook seam as the other Settings section tests; child cards are not under test.
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/settings/ProvidersSection.tsx")) {
      const stubs: Record<string, string> = {
        react: "export const useState=x=>h.state(x);",
        "@tanstack/react-query": "export const useQuery=o=>h.query(o);",
        "../../lib/i18n": "export const useT=()=>h.t;",
        "../../lib/api": "export const checkProviderHealth=id=>h.health(id);export const getConfig=()=>{};export const getProviders=()=>{};",
        "../../lib/xaiAuth": "export const GROK_SUB_PROVIDER_ID='grok-sub';",
        "../../components/BufferedSetting": "export const SETTINGS_INPUT_CLASS='input';",
        "./useSettingsSave": "export default ()=>async partial=>{h.saves.push(partial);return {};};",
        "./GrokSubCard": "export default ()=>null;",
        "./ModelPolicySettings": "export default ()=>null;",
        "./SectionState": "export default ()=>null;",
      };
      if (stubs[specifier]) {
        const source = "const h=globalThis.__providersSection;" + stubs[specifier];
        return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(source)}` };
      }
    }
    return nextResolve(specifier, context);
  },
});
const { default: ProvidersSection } = await import("./ProvidersSection.tsx");

beforeEach(() => {
  h.config = { providers: {} }; h.saves = []; h.states = [];
  h.health = async () => ({ ok: true, message: "" });
  setLocale("en");
});
after(() => { hooks.deregister(); Reflect.deleteProperty(globalThis, "__providersSection"); });

function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(nodes);
  return [node, ...nodes(node.props?.children)];
}
function text(node: any): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(text).join("");
  return node && typeof node === "object" ? text(node.props?.children) : "";
}
function render(): any[] {
  for (let i = 0; i < 4; i++) {
    h.cursor = 0; h.changed = false;
    const tree = nodes(ProvidersSection());
    if (!h.changed) return tree;
  }
  throw new Error("Hook harness did not settle");
}
const keyInput = () => render().find(node => node.type === "input" && node.props.type === "password");
const removeButton = () => render().find(node => node.type === "button" && text(node) === t("settings.providers.removeKey" as TranslateKey));
const testButton = () => render().find(node => node.type === "button" && node.props["data-provider-test"] === "openai");
const flush = () => new Promise<void>(resolve => setImmediate(resolve));

test("leaving a saved key's empty field keeps the key", async () => {
  h.config.providers.openai = { api_key: "***" };
  const input = keyInput();
  assert.equal(input.props.placeholder, t("settings.providers.savedKeyPlaceholder" as TranslateKey));
  await input.props.onBlur({ target: { value: "" } });
  await input.props.onBlur({ target: { value: "   " } });
  assert.deepEqual(h.saves, []);
});

test("a typed key is saved trimmed and the field clears", async () => {
  h.config.providers.openai = { api_key: "***" };
  const target = { value: "  sk-new  " };
  await keyInput().props.onBlur({ target });
  assert.deepEqual(h.saves, [{ providers: { openai: { api_key: "sk-new" } } }]);
  assert.equal(target.value, "");
});

test("Remove key clears the saved key explicitly", async () => {
  h.config.providers.openai = { api_key: "***" };
  await removeButton().props.onClick();
  assert.deepEqual(h.saves, [{ providers: { openai: { api_key: "" } } }]);
});

test("without a saved key there is nothing to remove", () => {
  assert.equal(removeButton(), undefined);
  assert.equal(keyInput().props.placeholder, t("settings.providers.enterApiKey"));
});

test("labels name their inputs", () => {
  const tree = render();
  const label = tree.find(node => node.type === "label" && text(node) === t("settings.providers.apiKey"));
  assert.ok(label?.props.htmlFor);
  assert.equal(label.props.htmlFor, keyInput().props.id);
});

test("Test is disabled while testing and shows the whole error", async () => {
  let fail!: (error: Error) => void;
  h.health = () => new Promise((_, reject) => { fail = reject; });
  const pending = testButton().props.onClick();
  assert.equal(testButton().props.disabled, true);
  const message = "Provider returned 401 Unauthorized: the API key was rejected by the remote service";
  fail(new Error(message));
  await pending; await flush();
  assert.equal(testButton().props.disabled, false);
  assert.ok(render().some(node => text(node) === message), "full error message is shown");
});
