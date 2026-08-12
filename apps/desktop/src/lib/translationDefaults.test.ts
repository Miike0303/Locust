/**
 * Lightweight asserts for translationDefaults (run: npx --yes tsx src/lib/translationDefaults.test.ts).
 */
import assert from "node:assert/strict";
import {
  coerceProviderId,
  resolveTranslationDefaults,
} from "./translationDefaults.ts";

// No config, no last-used → sane fallbacks
assert.deepEqual(resolveTranslationDefaults(undefined, undefined), {
  providerId: "",
  sourceLang: "auto",
  targetLang: "es",
  batchSize: 40,
  costLimit: "",
});
assert.deepEqual(resolveTranslationDefaults(null, null), {
  providerId: "",
  sourceLang: "auto",
  targetLang: "es",
  batchSize: 40,
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
  { providerId: "deepl", sourceLang: "ja", targetLang: "en", batchSize: 25, costLimit: "1.5" }
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
  { providerId: "", sourceLang: "auto", targetLang: "es", batchSize: 25, costLimit: "" }
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
    { provider: "ollama", source: "zh-CN", target: "fr", batchSize: 10, costLimit: "0.25" }
  ),
  { providerId: "ollama", sourceLang: "zh-CN", targetLang: "fr", batchSize: 10, costLimit: "0.25" }
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
  { providerId: "deepl", sourceLang: "ja", targetLang: "pt-BR", batchSize: 25, costLimit: "1.5" }
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
  { providerId: "deepl", sourceLang: "ja", targetLang: "en", batchSize: 40, costLimit: "" }
);

// coerceProviderId: keep a known id, replace an unknown one with the first available
const providers = [{ id: "mock" }, { id: "deepl" }];
assert.equal(coerceProviderId("deepl", providers), "deepl");
assert.equal(coerceProviderId("google", providers), "mock");
assert.equal(coerceProviderId("", providers), "mock");
// Without a provider list there is nothing to coerce against
assert.equal(coerceProviderId("google", undefined), "google");
assert.equal(coerceProviderId("google", []), "google");

console.log("translationDefaults.test.ts: ok");
