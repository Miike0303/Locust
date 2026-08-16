/**
 * Lightweight asserts for translationDefaults (run: npx --yes tsx src/lib/translationDefaults.test.ts).
 */
import assert from "node:assert/strict";
import {
  buildTranslationStartParams,
  coerceProviderId,
  resolveTranslationDefaults,
} from "./translationDefaults.ts";

// No config, no last-used → sane fallbacks
assert.deepEqual(resolveTranslationDefaults(undefined, undefined), {
  providerId: "",
  sourceLang: "auto",
  targetLang: "es",
  batchSize: 40,
  maxConcurrent: 3,
  costLimit: "",
});
assert.deepEqual(resolveTranslationDefaults(null, null), {
  providerId: "",
  sourceLang: "auto",
  targetLang: "es",
  batchSize: 40,
  maxConcurrent: 3,
  costLimit: "",
});

// Config only → Settings → Translation Defaults win over fallbacks
assert.deepEqual(
  resolveTranslationDefaults(
    {
      default_provider: "deepl",
      default_source_lang: "ja",
      default_target_lang: "en",
      default_batch_size: 25,
      default_cost_limit: 1.5,
    },
    undefined
  ),
  {
    providerId: "deepl",
    sourceLang: "ja",
    targetLang: "en",
    batchSize: 25,
    maxConcurrent: 3,
    costLimit: "1.5",
  }
);

// Config with null provider / null cost limit / empty langs → fallbacks fill in
assert.deepEqual(
  resolveTranslationDefaults(
    {
      default_provider: null,
      default_source_lang: "",
      default_target_lang: "",
      default_batch_size: 25,
      default_cost_limit: null,
    },
    undefined
  ),
  {
    providerId: "",
    sourceLang: "auto",
    targetLang: "es",
    batchSize: 25,
    maxConcurrent: 3,
    costLimit: "",
  }
);

// Last-used wins over config
assert.deepEqual(
  resolveTranslationDefaults(
    {
      default_provider: "deepl",
      default_source_lang: "ja",
      default_target_lang: "en",
      default_batch_size: 25,
      default_cost_limit: 1.5,
    },
    {
      provider: "ollama",
      source: "zh-CN",
      target: "fr",
      batchSize: 10,
      maxConcurrent: 1,
      costLimit: "0.25",
    }
  ),
  {
    providerId: "ollama",
    sourceLang: "zh-CN",
    targetLang: "fr",
    batchSize: 10,
    maxConcurrent: 1,
    costLimit: "0.25",
  }
);

// Partial last-used → per-field merge (missing fields fall through to config)
assert.deepEqual(
  resolveTranslationDefaults(
    {
      default_provider: "deepl",
      default_source_lang: "ja",
      default_target_lang: "en",
      default_batch_size: 25,
      default_cost_limit: 1.5,
    },
    { target: "pt-BR" }
  ),
  {
    providerId: "deepl",
    sourceLang: "ja",
    targetLang: "pt-BR",
    batchSize: 25,
    maxConcurrent: 3,
    costLimit: "1.5",
  }
);

// Last-used "" cost limit is an explicit "no limit" and beats the config limit
assert.equal(
  resolveTranslationDefaults(
    {
      default_provider: null,
      default_source_lang: "ja",
      default_target_lang: "en",
      default_batch_size: 40,
      default_cost_limit: 2,
    },
    { costLimit: "" }
  ).costLimit,
  ""
);

// Invalid batch sizes are skipped down the chain
assert.equal(
  resolveTranslationDefaults({ default_batch_size: 25 }, { batchSize: 0 }).batchSize,
  25
);
assert.equal(
  resolveTranslationDefaults({ default_batch_size: Number.NaN }, { batchSize: Number.NaN }).batchSize,
  40
);
assert.equal(
  resolveTranslationDefaults(undefined, { batchSize: 12.9 }).batchSize,
  12
);

// Empty-string last-used langs/provider are ignored (not real selections)
assert.deepEqual(
  resolveTranslationDefaults(
    { default_provider: "deepl", default_source_lang: "ja", default_target_lang: "en" },
    { provider: "", source: "", target: "" }
  ),
  {
    providerId: "deepl",
    sourceLang: "ja",
    targetLang: "en",
    batchSize: 40,
    maxConcurrent: 3,
    costLimit: "",
  }
);

// buildTranslationStartParams: queue must send fallbacks, concurrency, cost
{
  const p = buildTranslationStartParams({
    providerId: "grok-sub",
    fallbackIds: ["mock", "grok-sub", ""],
    sourceLang: "ja",
    targetLang: "es",
    batchSize: 20,
    maxConcurrent: 2,
    costLimit: "1.25",
    gameContext: "  VN  ",
  });
  assert.equal(p.provider_id, "grok-sub");
  assert.deepEqual(p.fallback_provider_ids, ["mock"]);
  assert.equal(p.options.max_concurrent, 2);
  assert.equal(p.options.cost_limit_usd, 1.25);
  assert.equal(p.options.game_context, "VN");
  assert.equal(p.options.batch_size, 20);
}
// empty cost / zero concurrency clamps
{
  const p = buildTranslationStartParams({
    providerId: "mock",
    fallbackIds: [],
    sourceLang: "en",
    targetLang: "es",
    batchSize: 0,
    maxConcurrent: 0,
    costLimit: "",
    gameContext: "",
  });
  assert.equal(p.fallback_provider_ids, undefined);
  assert.equal(p.options.max_concurrent, 1);
  assert.equal(p.options.cost_limit_usd, null);
  assert.equal(p.options.batch_size, 40);
  assert.equal(p.options.game_context, null);
}
// Negative: hardcoding max_concurrent:3 with null cost must not be the only path
assert.notDeepEqual(
  buildTranslationStartParams({
    providerId: "mock",
    fallbackIds: ["deepl"],
    sourceLang: "ja",
    targetLang: "es",
    batchSize: 10,
    maxConcurrent: 1,
    costLimit: "5",
    gameContext: "",
  }).options,
  { max_concurrent: 3, cost_limit_usd: null }
);

// coerceProviderId: keep a known ready id, replace unknown/unready with first ready provider
const providers = [
	{ id: "mock", requires_api_key: false },
	{ id: "deepl", requires_api_key: true, configured: false },
];
assert.equal(coerceProviderId("deepl", providers), "mock");
assert.equal(coerceProviderId("mock", providers), "mock");
assert.equal(
	coerceProviderId(
		"deepl",
		[{ id: "deepl", requires_api_key: true, configured: true }],
		{ providers: { deepl: { api_key: "***" } } },
	),
	"deepl",
);
assert.equal(coerceProviderId("google", providers), "mock");
assert.equal(coerceProviderId("", providers), "mock");
// Without a provider list there is nothing to coerce against
assert.equal(coerceProviderId("google", undefined), "google");
assert.equal(coerceProviderId("google", []), "google");

console.log("translationDefaults.test.ts: ok");
