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

// Invalid budgets must never silently become an unlimited API run.
const budgetForm = { providerId: "grok-sub", fallbackIds: [], sourceLang: "en", targetLang: "es",
  batchSize: 10, maxConcurrent: 10, costLimit: "0", gameContext: "" };
assert.equal(buildTranslationStartParams(budgetForm).options.cost_limit_usd, 0);
for (const costLimit of ["-1", "NaN", "Infinity", "2oops", "1,5"]) {
  assert.throws(() => buildTranslationStartParams({ ...budgetForm, costLimit }), /cost limit/);
}

// --- project-aware languages (cycle 116 QA: an English game opened as ja -> en) ---
import { detectSourceLanguage } from "./translationDefaults.ts";
const english = ["New Game", "Load Game", "Options", "You really haven't aged a day.", "Where are we going tonight?"];
const japanese = ["こんにちは、世界。", "今日はいい天気ですね。", "ニューゲーム"];
assert.equal(detectSourceLanguage(english.concat(english, english)), "en");
assert.equal(detectSourceLanguage(japanese), "ja");
assert.equal(detectSourceLanguage(Array(3).fill(["안녕하세요, 반갑습니다.", "새 게임"]).flat()), "ko");
assert.equal(detectSourceLanguage(Array(3).fill(["你好，世界。", "今天天气很好。", "新游戏开始"]).flat()), "zh-CN");
assert.equal(detectSourceLanguage(Array(3).fill(["Привет, мир.", "Новая игра"]).flat()), "ru");
// Accented Latin text is not assumed to be English; tiny samples decide nothing.
assert.equal(detectSourceLanguage(["¿Adónde vamos esta noche?", "Configuración", "Canción de la mañana"]), null);
assert.equal(detectSourceLanguage(["OK"]), null);
assert.equal(detectSourceLanguage([]), null);

const factory = { default_source_lang: "ja", default_target_lang: "en" };
// The project's own text beats the global source default and a previous project's last-used source.
assert.equal(resolveTranslationDefaults(factory, undefined, { detectedSource: "en", uiLocale: "es" }).sourceLang, "en");
assert.equal(resolveTranslationDefaults(factory, { source: "ja" }, { detectedSource: "en", uiLocale: "es" }).sourceLang, "en");
// Target never equals the source: fall back to the UI language, then English, then Spanish.
assert.equal(resolveTranslationDefaults(factory, undefined, { detectedSource: "en", uiLocale: "es" }).targetLang, "es");
assert.equal(resolveTranslationDefaults(factory, undefined, { detectedSource: "en", uiLocale: "en" }).targetLang, "es");
assert.equal(resolveTranslationDefaults({ default_target_lang: "es" }, undefined, { detectedSource: "es", uiLocale: "es" }).targetLang, "en");
// A target the user chose that differs from the source is kept.
assert.equal(resolveTranslationDefaults(factory, { target: "fr" }, { detectedSource: "en", uiLocale: "es" }).targetLang, "fr");
// Without detection nothing changes.
assert.deepEqual(resolveTranslationDefaults(factory, undefined, { detectedSource: null, uiLocale: "es" }),
  resolveTranslationDefaults(factory, undefined));
