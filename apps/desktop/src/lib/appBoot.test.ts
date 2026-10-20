import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { readFileSync } from "node:fs";
import type { AppConfig } from "./api.ts";
import { queryClient } from "./queryClient.ts";

const config: AppConfig = {
  providers: {}, default_provider: null, default_source_lang: "ja", default_target_lang: "en",
  default_batch_size: 20, default_cost_limit: null, recent_projects: [],
  ui: { theme: "dark", font_size: 18, show_source_column: true, table_row_height: 36 },
};
afterEach(() => queryClient.clear());

test("boot appearance shares in-flight config and cached data with a second consumer", async () => {
  const { restoreBootAppearance } = await import("./appBoot.ts");
  let calls = 0;
  const loadConfig = async () => { calls++; return config; };
  const applied: unknown[] = [];
  const boot = restoreBootAppearance(loadConfig, ui => { applied.push(ui); });
  const consumer = queryClient.fetchQuery({ queryKey: ["config"], queryFn: loadConfig });
  await boot;
  assert.equal(await consumer, config);
  assert.equal(await queryClient.fetchQuery({ queryKey: ["config"], queryFn: loadConfig }), config);
  assert.equal(calls, 1);
  assert.deepEqual(applied, [config.ui]);
});

test("boot restores cached appearance without a config request", async () => {
  const { restoreBootAppearance } = await import("./appBoot.ts");
  queryClient.setQueryData(["config"], config);
  let applied: unknown;
  await restoreBootAppearance(async () => { throw new Error("cache should be reused"); }, ui => { applied = ui; });
  assert.equal(applied, config.ui);
});

test("boot keeps defaults and resolves silently when the backend is down", async () => {
  const { restoreBootAppearance } = await import("./appBoot.ts");
  queryClient.setQueryDefaults(["config"], { retry: false });
  let applied = false;
  await assert.doesNotReject(restoreBootAppearance(
    async () => { throw new Error("backend unreachable"); },
    () => { applied = true; },
  ));
  assert.equal(applied, false);
});

test("App boot uses the cached appearance helper with the real getConfig", () => {
  const source = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  assert.match(source, /import \{ restoreBootAppearance \} from "\.\/lib\/appBoot"/);
  assert.match(source, /void restoreBootAppearance\(getConfig\)/);
  assert.doesNotMatch(source, /getConfig\(\)/);
});
