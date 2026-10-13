/**
 * Lightweight asserts for providerReadiness (run: npx --yes tsx src/lib/providerReadiness.test.ts).
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import { setLocale } from "./i18n/index.ts";
import {
	formatProviderOptionLabel,
	hasAnyReadyProvider,
	readProviderSetupHintDismissed,
	resolveProviderReadiness,
	saveProviderSetupHintDismissed,
} from "./providerReadiness.ts";

for (const locale of ["en", "es"] as const) test(`provider availability is localized exactly once (${locale})`, () => {
  setLocale(locale);
  const suffix = locale === "en" ? "free" : "gratuito";
  for (const name of ["Google Translate", "Google Translate (free)", "Google Translate (free) (free)"]) {
    assert.equal(formatProviderOptionLabel({ name, is_free: true }), `Google Translate (${suffix})`);
  }
  assert.equal(formatProviderOptionLabel({ name: "Free Speech (preview)", is_free: true }), `Free Speech (preview) (${suffix})`);
  assert.equal(formatProviderOptionLabel({ name: "Custom (free)", is_free: false }), "Custom (free)");
  const needsKey = locale === "en" ? "needs API key" : "falta la clave API";
  assert.equal(formatProviderOptionLabel({ name: "Google Translate (free)", is_free: true, configured: false }), `Google Translate (${suffix}) (${needsKey})`);
  setLocale("en");
});

const providers = [
	{ id: "mock", requires_api_key: false },
	{ id: "deepl", requires_api_key: true },
	{ id: "openai", requires_api_key: true },
] as const;

// Free / no-key providers are always ready
assert.deepEqual(resolveProviderReadiness("mock", providers, undefined), {
	ready: true,
});
assert.deepEqual(resolveProviderReadiness("mock", providers, { providers: {} }), {
	ready: true,
});

// configured=false from server means missing key even without config lookup
assert.deepEqual(
	resolveProviderReadiness(
		"deepl",
		[{ id: "deepl", requires_api_key: true, configured: false }],
		undefined,
	),
	{ ready: false, reason: "missing_key" },
);
assert.deepEqual(
	resolveProviderReadiness(
		"deepl",
		[{ id: "deepl", requires_api_key: true, configured: true }],
		undefined,
	),
	{ ready: false, reason: "missing_key" },
);
assert.deepEqual(
	resolveProviderReadiness(
		"deepl",
		[{ id: "deepl", requires_api_key: true, configured: true }],
		{ providers: { deepl: { api_key: "***" } } },
	),
	{ ready: true },
);

// Key-required providers need a configured key
assert.deepEqual(resolveProviderReadiness("deepl", providers, undefined), {
	ready: false,
	reason: "missing_key",
});
assert.deepEqual(
	resolveProviderReadiness("deepl", providers, { providers: { deepl: { api_key: "" } } }),
	{ ready: false, reason: "missing_key" },
);
assert.deepEqual(
	resolveProviderReadiness("deepl", providers, { providers: { deepl: { api_key: "   " } } }),
	{ ready: false, reason: "missing_key" },
);

// Masked placeholder means a key is already stored server-side
assert.deepEqual(
	resolveProviderReadiness("deepl", providers, { providers: { deepl: { api_key: "***" } } }),
	{ ready: true },
);
assert.deepEqual(
	resolveProviderReadiness("openai", providers, { providers: { openai: { api_key: "sk-test" } } }),
	{ ready: true },
);

// Unknown / empty id
assert.deepEqual(resolveProviderReadiness("", providers, undefined), {
	ready: false,
	reason: "unknown_provider",
});
assert.deepEqual(resolveProviderReadiness("google", providers, undefined), {
	ready: false,
	reason: "unknown_provider",
});
assert.deepEqual(resolveProviderReadiness("deepl", [], undefined), {
	ready: false,
	reason: "unknown_provider",
});
assert.deepEqual(resolveProviderReadiness("deepl", null, undefined), {
	ready: false,
	reason: "unknown_provider",
});

// hasAnyReadyProvider across the list
assert.equal(hasAnyReadyProvider(providers, undefined), true);
assert.equal(
	hasAnyReadyProvider(
		[{ id: "deepl", requires_api_key: true }],
		{ providers: {} },
	),
	false,
);
assert.equal(
	hasAnyReadyProvider(
		[{ id: "deepl", requires_api_key: true }],
		{ providers: { deepl: { api_key: "***" } } },
	),
	true,
);
assert.equal(hasAnyReadyProvider([], undefined), false);
assert.equal(hasAnyReadyProvider(null, undefined), false);

// Dismissible Welcome hint persistence
const storage = new Map<string, string>();
const memStorage = {
	getItem: (k: string) => storage.get(k) ?? null,
	setItem: (k: string, v: string) => {
		storage.set(k, v);
	},
	removeItem: (k: string) => {
		storage.delete(k);
	},
};
assert.equal(readProviderSetupHintDismissed(memStorage), false);
saveProviderSetupHintDismissed(true, memStorage);
assert.equal(readProviderSetupHintDismissed(memStorage), true);
saveProviderSetupHintDismissed(false, memStorage);
assert.equal(readProviderSetupHintDismissed(memStorage), false);

console.log("providerReadiness.test.ts: ok");
