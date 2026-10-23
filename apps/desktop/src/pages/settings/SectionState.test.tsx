import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, before, beforeEach, test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider, QueryObserver } from "@tanstack/react-query";
import { setLocale, t } from "../../lib/i18n/index.ts";

const config = {
  providers: { ollama: { model: "saved-model", base_url: "http://saved-host:11434" } },
  default_provider: "ollama", default_source_lang: "ja", default_target_lang: "es",
  default_batch_size: 23, default_cost_limit: 4,
  ui: { theme: "dark", font_size: 16, show_source_column: true, table_row_height: 40 },
};
const providers = [{ id: "ollama", name: "Ollama", is_free: true, requires_api_key: false, configured: true }];
const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity, staleTime: Infinity } } });
const h = {
  t, calls: [] as string[],
  query(options: any) {
    const key = options.queryKey[0] as "config" | "providers";
    return new QueryObserver(client, { ...options, queryFn: async () => {
      h.calls.push(key);
      return key === "config" ? config : providers;
    } }).getCurrentResult();
  },
};
(globalThis as any).__sectionStateTest = h;
// Follow the existing Settings hook seam while retaining real query state and refetch.
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (/\/(AppearanceSection|DefaultsSection|ProvidersSection|SectionState)\.tsx$/.test(context.parentURL ?? "")) {
      const stubs: Record<string, string> = {
        "@tanstack/react-query": "export const useQuery=o=>globalThis.__sectionStateTest.query(o);",
        react: "export const useState=x=>[typeof x==='function'?x():x,()=>{}];",
        "../../lib/i18n": "export const useT=()=>globalThis.__sectionStateTest.t; export const useLocale=()=>({locale:'en',setLocale:()=>{}});",
        "./useSettingsSave": "export default ()=>async()=>true;",
      };
      if (stubs[specifier]) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(stubs[specifier])}` };
    }
    return nextResolve(specifier, context);
  },
});

let sections: Record<string, () => any>;
before(async () => {
  sections = {
    appearance: (await import("./AppearanceSection.tsx")).default,
    defaults: (await import("./DefaultsSection.tsx")).default,
    providers: (await import("./ProvidersSection.tsx")).default,
  };
});
beforeEach(() => { client.clear(); h.calls = []; setLocale("en"); });
after(() => { hooks.deregister(); client.clear(); Reflect.deleteProperty(globalThis, "__sectionStateTest"); setLocale("en"); });

function render(section: string) {
  let tree = sections[section]();
  if (typeof tree?.type === "function" && tree.type.name === "SectionState") tree = tree.type(tree.props);
  const html = renderToStaticMarkup(createElement(QueryClientProvider, { client }, tree));
  return { tree, html };
}
function nodes(tree: any): any[] {
  if (!tree || typeof tree !== "object") return [];
  if (Array.isArray(tree)) return tree.flatMap(nodes);
  return [tree, ...nodes(tree.props?.children)];
}
function seed() { client.setQueryData(["config"], config); client.setQueryData(["providers"], providers); }
function fail(key: string, cached = false) {
  if (!cached) client.removeQueries({ queryKey: [key] });
  const query = client.getQueryCache().build(client, { queryKey: [key] });
  query.setState({ status: "error", error: new Error(`${key} unavailable <offline>`) });
}
function assertCard(html: string, section: string) {
  assert.ok(html.includes(t(`settings.${section}.title` as Parameters<typeof t>[0])));
  assert.ok(html.includes("border border-border bg-surface rounded-xl p-5"));
  assert.doesNotMatch(html, /<(input|select|textarea)\b/);
}

for (const section of ["appearance", "defaults", "providers"]) {
  const required = section === "appearance" ? ["config"] : ["config", "providers"];
  for (const key of required) {
    test(`${section}: pending ${key} keeps title and loading status without editable controls`, () => {
      seed(); client.removeQueries({ queryKey: [key] });
      const { html } = render(section);
      assertCard(html, section);
      assert.match(html, /<p[^>]*role="status"/);
      assert.ok(html.includes(t("common.loading")));
      assert.doesNotMatch(html, /role="alert"/);
    });
    for (const cached of [false, true]) {
      test(`${section}: ${key} error${cached ? " with cached data" : ""} alerts and Retry refetches then restores controls`, async () => {
        seed(); fail(key, cached);
        const { tree, html } = render(section);
        assertCard(html, section);
        assert.match(html, /role="alert"/);
        assert.ok(html.includes("Settings could not be loaded"));
        assert.ok(html.includes(`${key} unavailable &lt;offline&gt;`));
        const retry = nodes(tree).find(node => node.type === "button" && node.props.children === t("common.retry"));
        assert.ok(retry, "a visible Retry button is required");
        retry.props.onClick();
        await new Promise(resolve => setImmediate(resolve));
        assert.ok(h.calls.includes(key), `Retry must refetch ${key}`);
        assert.match(render(section).html, /<(input|select)\b/);
        assert.doesNotMatch(render(section).html, /role="alert"/);
      });
    }
  }
  test(`${section}: loaded content retains saved values and controls`, () => {
    seed();
    const { html } = render(section);
    assert.doesNotMatch(html, /role="(status|alert)"/);
    for (const value of section === "appearance" ? ['name="theme"', 'value="16"', 'value="40"']
      : section === "defaults" ? ['value="ja"', 'value="es"', 'value="23"', 'value="4"', '>Ollama</option>']
        : ['value="saved-model"', 'value="http://saved-host:11434"', t("settings.providers.testConnection")]) {
      assert.ok(html.includes(value), `retained control: ${value}`);
    }
  });
}

test("Providers: empty list offers guidance without editable controls", () => {
  seed(); client.setQueryData(["providers"], []);
  const { html } = render("providers");
  assertCard(html, "providers");
  assert.ok(html.includes("No providers are available. Check your provider setup and refresh the list."));
});

test("Defaults: an empty providers list retains the loaded config and None option", () => {
  seed(); client.setQueryData(["providers"], []);
  const { html } = render("defaults");
  assert.ok(html.includes(t("common.none")));
  assert.ok(html.includes('value="ja"'));
});

for (const section of ["defaults", "providers"]) {
  test(`${section}: a failed dependency takes precedence over another pending query`, () => {
    fail("providers");
    const { html } = render(section);
    assertCard(html, section);
    assert.match(html, /role="alert"/);
    assert.ok(html.includes("providers unavailable"));
  });
}
