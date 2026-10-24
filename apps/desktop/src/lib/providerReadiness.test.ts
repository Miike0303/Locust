/**
 * Lightweight asserts for providerReadiness (run: npx --yes tsx src/lib/providerReadiness.test.ts).
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import { getLocale, setLocale, t } from "./i18n";
import {
	formatProviderOptionLabel,
	hasAnyReadyProvider,
	readProviderSetupHintDismissed,
	resolveProviderReadiness,
	saveProviderSetupHintDismissed,
} from "./providerReadiness.ts";

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

for (const locale of ["en", "es"] as const) {
	for (const suffix of ["", " (free)", " (FREE)", " (gratuito)", " (GrAtUiTo)", " (free) (gratuito) (FREE)  ", "(free)(gratuito)"]) {
		test(`provider label normalizes trailing owned free suffixes (${locale}, ${suffix || "no suffix"})`, () => {
			const previous = getLocale();
			try {
				setLocale(locale);
				const name = `Google Translate${suffix}`;
				assert.equal(
					formatProviderOptionLabel({ name, is_free: true }),
					`Google Translate (${t("provider.free")})`,
				);
			} finally {
				setLocale(previous);
			}
		});
	}
	test(`provider label preserves non-free names and API-key guidance (${locale})`, () => {
		const previous = getLocale();
		try {
			setLocale(locale);
			for (const name of ["DeepL", "Paid (free)", "Paid (gratuito) (free)"]) {
				assert.equal(formatProviderOptionLabel({ name, is_free: false }), name);
			}
			assert.equal(formatProviderOptionLabel({ name: "Example (free) Plus", is_free: true }), `Example (free) Plus (${t("provider.free")})`);
			assert.equal(formatProviderOptionLabel({ name: "OpenAI", configured: false }), `OpenAI (${t("provider.needsApiKeySuffix")})`);
			assert.equal(formatProviderOptionLabel({ name: "Google (FREE)", is_free: true, configured: false }), `Google (${t("provider.free")}) (${t("provider.needsApiKeySuffix")})`);
			assert.equal(formatProviderOptionLabel({ name: "Carefree (free tier)", is_free: true }), `Carefree (free tier) (${t("provider.free")})`);
		} finally {
			setLocale(previous);
		}
	});
}

console.log("providerReadiness.test.ts: ok");
