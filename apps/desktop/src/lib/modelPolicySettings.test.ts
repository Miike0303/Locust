import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { ownValue, policyDraft, policyPatch } from "./modelPolicySettings.ts";
import { setLocale, t } from "./i18n/index.ts";
import type { AppConfig, ModelPolicy } from "./api.ts";

const base = { context_window: 8192, input_usd_per_million: 1, output_usd_per_million: 2 };
function patch(policy: ModelPolicy | undefined, changes: Partial<ReturnType<typeof policyDraft>>) {
  return policyPatch("Custom", " Model/X ", policy, { ...policyDraft(policy), ...changes });
}
test("exact case-sensitive keys survive without trimming; blank identities are rejected", () => {
  assert.deepEqual(patch(undefined, { inputPrice: "0", outputPrice: "2" }).patch,
    { model_overrides: { Custom: { " Model/X ": { input_usd_per_million: 0, output_usd_per_million: 2 } } } });
  for (const [provider, model] of [[" ", "M"], ["P", "\t"]]) {
    assert.equal(policyPatch(provider, model, undefined, policyDraft()).error, "identity");
  }
  assert.deepEqual(policyPatch("__proto__", "constructor", undefined,
    { ...policyDraft(), inputPrice: "0", outputPrice: "0" }).patch?.model_overrides?.["__proto__"]?.constructor,
    { input_usd_per_million: 0, output_usd_per_million: 0 });
});
test("sparse patch contains only deliberately changed fields, never cached policy or other keys", () => {
  const policy = { ...base, future_field: "keep" };
  assert.deepEqual(patch(policy, { inputPrice: "3" }).patch,
    { model_overrides: { Custom: { " Model/X ": { input_usd_per_million: 3 } } } });
  assert.deepEqual(patch(policy, { outputPrice: "0" }).patch,
    { model_overrides: { Custom: { " Model/X ": { output_usd_per_million: 0 } } } });
  assert.equal(patch(policy, {}).patch, undefined);
  assert.deepEqual(patch(policy, { context: "4000", contextChanged: true }).patch,
    { model_overrides: { Custom: { " Model/X ": { context_window: 4000 } } } });
});
test("prices require a finite nonnegative pair; zero is valid and clearing resets both", () => {
  for (const prices of [["1", ""], ["", "2"], ["-1", "2"], ["Infinity", "2"], ["NaN", "2"], ["1e309", "0"]]) {
    assert.equal(patch(base, { inputPrice: prices[0], outputPrice: prices[1] }).error, "prices");
  }
  assert.deepEqual(patch(base, { inputPrice: "", outputPrice: "" }).patch,
    { model_overrides: { Custom: { " Model/X ": { input_usd_per_million: null, output_usd_per_million: null } } } });
  assert.deepEqual(patch(undefined, { inputPrice: "0", outputPrice: "0" }).patch,
    { model_overrides: { Custom: { " Model/X ": { input_usd_per_million: 0, output_usd_per_million: 0 } } } });
});
test("context accepts only positive safe decimal digits, not fractions, signs or exponents", () => {
  for (const context of ["0", "-1", "+1", "1e3", "1.5", "9007199254740992", "Infinity", " 12 "]) {
    assert.equal(patch(base, { context, contextChanged: true }).error, "context", context);
  }
  assert.deepEqual(patch(base, { context: "9007199254740991", contextChanged: true }).patch,
    { model_overrides: { Custom: { " Model/X ": { context_window: 9007199254740991 } } } });
});
test("unsafe u64 is never shown as exact or resent; safe replacement and explicit reset work", () => {
  const unsafe = { ...base, context_window: Number("18446744073709551615") };
  assert.equal(policyDraft(unsafe).context, "");
  assert.deepEqual(patch(unsafe, { inputPrice: "4" }).patch,
    { model_overrides: { Custom: { " Model/X ": { input_usd_per_million: 4 } } } });
  assert.deepEqual(patch(unsafe, { context: "4096", contextChanged: true }).patch,
    { model_overrides: { Custom: { " Model/X ": { context_window: 4096 } } } });
  assert.deepEqual(patch(unsafe, { contextChanged: true }).patch,
    { model_overrides: { Custom: { " Model/X ": { context_window: null } } } });
});
test("policy lookup excludes inherited keys but preserves exact own identities", () => {
  assert.equal(ownValue({}, "constructor"), undefined);
  const map = JSON.parse('{"__proto__":{"context_window":42}," P ":{"context_window":12}}');
  assert.deepEqual(ownValue(map, "__proto__"), { context_window: 42 });
  assert.equal(ownValue(map, " p "), undefined);
  assert.deepEqual(policyPatch(" P ", "M", undefined, { ...policyDraft(), context: "12", contextChanged: true }).patch,
    { model_overrides: { " P ": { M: { context_window: 12 } } } });
});
test("optional blank context is omitted unless deliberately cleared/reset", () => {
  assert.equal(patch(undefined, {}).patch, undefined);
  assert.deepEqual(patch(base, { context: "", contextChanged: true }).patch,
    { model_overrides: { Custom: { " Model/X ": { context_window: null } } } });
  assert.deepEqual(patch(undefined, { contextChanged: true }).patch,
    { model_overrides: { Custom: { " Model/X ": { context_window: null } } } });
});

// Follow the existing public-element/hook harness: drive the returned form's
// events and rerender its state, without DOM dependencies or source inspection.
const h = { t, slots: [] as any[], cursor: 0, pending: false, result: {} as AppConfig | null | false,
  fail: false, calls: [] as any[], finish: undefined as undefined | (() => void),
  async save(payload: any) {
    h.calls.push(payload);
    if (h.pending) await new Promise<void>(resolve => { h.finish = resolve; });
    if (h.fail) throw Error("write failed");
    return h.result;
  } };
(globalThis as any).__policyTest = h;
const hooks = registerHooks({ resolve(specifier, context, nextResolve) {
  if (specifier === "../../lib/i18n" && context.parentURL?.endsWith("/settings/ModelPolicySettings.tsx")) {
    return { shortCircuit: true, url: "data:text/javascript,export const useT=()=>globalThis.__policyTest.t;" };
  }
  if (specifier === "react" && context.parentURL?.endsWith("/settings/ModelPolicySettings.tsx")) {
    return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(`const h=globalThis.__policyTest;
      export const useState=initial=>{const i=h.cursor++;if(!(i in h.slots))h.slots[i]=typeof initial==='function'?initial():initial;return [h.slots[i],v=>{h.slots[i]=typeof v==='function'?v(h.slots[i]):v;}];};
      export const useRef=initial=>{const i=h.cursor++;return h.slots[i]??={current:initial};};`)}` };
  }
  return nextResolve(specifier, context);
} });
const { default: ModelPolicySettings } = await import("../pages/settings/ModelPolicySettings.tsx");
let config: AppConfig;
function tree() { h.cursor = 0; return ModelPolicySettings({ providers: [{ id: "openai" }], config, onSave: h.save }); }
function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  return Array.isArray(node) ? node.flatMap(nodes) : [node, ...nodes(node.props?.children)];
}
function control(id: string) {
  const found = nodes(tree()).find(n => n.props?.id === `model-policy-${id}`);
  assert.ok(found, `missing public control ${id}`); return found;
}
function change(id: string, value: string) { control(id).props.onChange({ target: { value } }); }
function click(id: string) { return control(id).props.onClick(); }
function submit() { return nodes(tree()).find(n => n.type === "form").props.onSubmit({ preventDefault() {} }); }
function html() { return renderToStaticMarkup(tree()); }
function begin(provider = "Custom", model = " Model/X ") {
  change("provider", provider); change("model", model); click("edit");
}
beforeEach(() => {
  h.slots = []; h.calls = []; h.pending = false; h.result = {} as AppConfig; h.fail = false; h.finish = undefined;
  config = { providers: { configured: { model: "configured-model" } }, model_overrides: {
    Custom: { " Model/X ": { ...base }, "model/x": { context_window: 42 } },
  } } as unknown as AppConfig;
  setLocale("en");
});
after(() => { hooks.deregister(); Reflect.deleteProperty(globalThis, "__policyTest"); setLocale("en"); });
for (const locale of ["en", "es"] as const) {
  test(`${locale}: public form labels, discovery/custom keys, exact text and explicit edit baseline`, async () => {
    setLocale(locale);
    const markup = html();
    for (const id of ["openai", "Custom", "configured"]) assert.ok(markup.includes(`value="${id}"`));
    for (const id of ["provider", "model"]) assert.ok(markup.includes(`for="model-policy-${id}"`));
    begin();
    assert.equal(control("context").props.value, "8192");
    assert.equal(control("provider").props.disabled, true);
    assert.equal(control("model").props.disabled, true);
    config.model_overrides!.Custom[" Model/X "].context_window = 10;
    change("inputPrice", "3"); await submit();
    assert.deepEqual(h.calls, [{ model_overrides: { Custom: { " Model/X ": { input_usd_per_million: 3 } } } }]);
    assert.equal(control("edit").props.disabled, false, "success closes editor");
    assert.ok(!nodes(tree()).some(n => n.props?.id === "model-policy-context"));
  });
  test(`${locale}: approximate/user pricing and unsafe precision explanations are localized`, () => {
    setLocale(locale); config.model_overrides!.Custom[" Model/X "].context_window = Number("18446744073709551615");
    begin(); const markup = html();
    for (const key of ["hint", "unsafe", "contextHint", "priceHint"] as const) {
      const label = t(`settings.modelPolicy.${key}` as any);
      assert.ok(!label.startsWith("settings.")); assert.ok(markup.includes(renderToStaticMarkup(label)));
    }
    assert.ok(!markup.includes("18446744073709552000"));
    assert.equal(control("context").props.value, "");
    for (const id of ["context", "inputPrice", "outputPrice"]) assert.ok(markup.includes(`for="model-policy-${id}"`));
  });
}
test("pending disables every control, guards duplicate saves and preserves draft on null/false failure", async () => {
  begin(); change("inputPrice", "7"); h.pending = true; h.result = null;
  const saving = submit();
  for (const node of nodes(tree()).filter(n => ["input", "select", "button"].includes(n.type))) {
    assert.equal(node.props.disabled, true, node.props.id);
  }
  await submit(); assert.equal(h.calls.length, 1);
  h.finish!(); await saving;
  assert.equal(control("inputPrice").props.value, "7");
  assert.match(html(), /role="alert"/);
  h.pending = false; h.result = false; await submit();
  assert.equal(control("inputPrice").props.value, "7");
  h.fail = true; await submit();
  assert.equal(control("inputPrice").props.value, "7");
  h.fail = false; h.result = {} as AppConfig; await submit();
  assert.equal(control("edit").props.disabled, false);
});
test("cancel explicitly discards draft; selecting another exact key uses its own baseline", () => {
  begin(); change("context", "99"); click("cancel");
  begin("Custom", "model/x"); assert.equal(control("context").props.value, "42");
  click("cancel"); begin(); assert.equal(control("context").props.value, "8192");
  assert.equal(h.calls.length, 0);
});
test("public form validates without writes, resets prices together and unsafe context only deliberately", async () => {
  config.model_overrides!.Custom[" Model/X "].context_window = Number("18446744073709551615");
  begin(); change("inputPrice", ""); await submit(); assert.equal(h.calls.length, 0);
  change("outputPrice", ""); await submit();
  assert.deepEqual(h.calls[0], { model_overrides: { Custom: { " Model/X ": { input_usd_per_million: null, output_usd_per_million: null } } } });
  begin(); change("context", "1e3"); await submit(); assert.equal(h.calls.length, 1);
  click("resetContext"); await submit();
  assert.deepEqual(h.calls[1], { model_overrides: { Custom: { " Model/X ": { context_window: null } } } });
});
test("new exact model, unchanged optional fields and precision-safe replacement through the form", async () => {
  click("edit"); assert.match(html(), /role="alert"/); assert.equal(h.calls.length, 0);
  begin("openai", "unlisted-new-model");
  assert.equal(control("save").props.disabled, true);
  change("inputPrice", "0"); change("outputPrice", "0"); await submit();
  assert.deepEqual(h.calls[0], { model_overrides: { openai: { "unlisted-new-model": { input_usd_per_million: 0, output_usd_per_million: 0 } } } });
  config.model_overrides!.Custom[" Model/X "].context_window = Number("18446744073709551615");
  begin(); change("context", "9007199254740991"); await submit();
  assert.deepEqual(h.calls[1], { model_overrides: { Custom: { " Model/X ": { context_window: 9007199254740991 } } } });
});
test("focus enters context on edit and returns to Edit after Cancel or successful Save", async () => {
  begin(); assert.equal(control("context").props.autoFocus, true);
  let focused = 0;
  click("cancel"); control("edit").props.ref({ focus() { focused++; } });
  assert.equal(focused, 1);
  begin(); change("inputPrice", "8"); await submit();
  control("edit").props.ref({ focus() { focused++; } });
  assert.equal(focused, 2);
});
