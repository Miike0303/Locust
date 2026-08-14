/**
 * Pure provider readiness checks for TranslationModal, QueuePanel, and Welcome.
 *
 * Uses structural types so tests don't import api.ts. Field names match
 * ProviderInfo.requires_api_key and AppConfig.providers[id].api_key.
 */

import { t } from "./i18n";

export interface ProviderReadinessMeta {
	id: string;
	requires_api_key: boolean;
	/** When false, the provider is listed but not registered (no API key yet). */
	configured?: boolean;
}

export interface ProviderReadinessConfig {
	providers?: Record<string, { api_key?: string | null } | undefined>;
}

export type ProviderReadinessReason = "missing_key" | "unknown_provider";

export interface ProviderReadinessResult {
	ready: boolean;
	reason?: ProviderReadinessReason;
}

function hasConfiguredApiKey(
	providerConfig?: { api_key?: string | null } | null,
): boolean {
	const key = providerConfig?.api_key;
	if (typeof key !== "string") return false;
	if (key === "***") return true;
	return key.trim().length > 0;
}

export function resolveProviderReadiness(
	providerId: string,
	providers: readonly ProviderReadinessMeta[] | null | undefined,
	config?: ProviderReadinessConfig | null,
): ProviderReadinessResult {
	if (!providerId) {
		return { ready: false, reason: "unknown_provider" };
	}
	const meta = providers?.find((p) => p.id === providerId);
	if (!meta) {
		return { ready: false, reason: "unknown_provider" };
	}
	if (!meta.requires_api_key) {
		return { ready: true };
	}
	if (meta.configured === false) {
		return { ready: false, reason: "missing_key" };
	}
	if (hasConfiguredApiKey(config?.providers?.[providerId])) {
		return { ready: true };
	}
	return { ready: false, reason: "missing_key" };
}

export function hasAnyReadyProvider(
	providers: readonly ProviderReadinessMeta[] | null | undefined,
	config?: ProviderReadinessConfig | null,
): boolean {
	if (!providers || providers.length === 0) return false;
	return providers.some(
		(p) => resolveProviderReadiness(p.id, providers, config).ready,
	);
}

export function formatProviderOptionLabel(meta: {
	name: string;
	is_free?: boolean;
	configured?: boolean;
}): string {
	let label = meta.name;
	if (meta.is_free) label += ` (${t("provider.free")})`;
	if (meta.configured === false) label += ` (${t("provider.needsApiKeySuffix")})`;
	return label;
}

const HINT_DISMISSED_KEY = "locust.providerReadiness.hintDismissed";

export type ProviderReadinessStorage = Pick<
	Storage,
	"getItem" | "setItem" | "removeItem"
>;

function browserStorage(): ProviderReadinessStorage | null {
	try {
		return typeof localStorage === "undefined" ? null : localStorage;
	} catch {
		return null;
	}
}

export function readProviderSetupHintDismissed(
	storage?: ProviderReadinessStorage | null,
): boolean {
	const target = storage === undefined ? browserStorage() : storage;
	if (!target) return false;
	try {
		return target.getItem(HINT_DISMISSED_KEY) === "1";
	} catch {
		return false;
	}
}

export function saveProviderSetupHintDismissed(
	dismissed: boolean,
	storage?: ProviderReadinessStorage | null,
): void {
	const target = storage === undefined ? browserStorage() : storage;
	if (!target) return;
	try {
		if (dismissed) target.setItem(HINT_DISMISSED_KEY, "1");
		else target.removeItem(HINT_DISMISSED_KEY);
	} catch {
		/* Storage can be unavailable in private or restricted browser contexts. */
	}
}
