import { useEffect, useState, type Dispatch, type SetStateAction } from "react";
import {
	resolveProviderReadiness,
	type ProviderReadinessConfig,
	type ProviderReadinessMeta,
} from "./providerReadiness";
import { coerceProviderId, readTranslationFallbacks } from "./translationDefaults";

interface TranslationSettingsOptions {
	providerId: string;
	setProviderId: Dispatch<SetStateAction<string>>;
	providers: readonly ProviderReadinessMeta[] | undefined;
	config: ProviderReadinessConfig | undefined;
	initialFallbackIds?: () => string[];
}

/**
 * Shared provider coercion/readiness and fallback editing. Call after the
 * caller's defaults effect to preserve the order of the two effects.
 * Default initialization and reset policy remain with each form.
 */
export function useTranslationSettings({
	providerId,
	setProviderId,
	providers,
	config,
	initialFallbackIds = readTranslationFallbacks,
}: TranslationSettingsOptions) {
	const [fallbackIds, setFallbackIds] = useState<string[]>(initialFallbackIds);
	const [fallbackPick, setFallbackPick] = useState("");

	// Keep the provider <select> on a listed, ready id when possible.
	useEffect(() => {
		if (!providers || providers.length === 0) return;
		setProviderId((prev) => coerceProviderId(prev, providers, config));
	}, [providers, config]);

	const addFallback = () => {
		if (
			!fallbackPick ||
			fallbackPick === providerId ||
			fallbackIds.includes(fallbackPick)
		)
			return;
		setFallbackIds((prev) => [...prev, fallbackPick]);
		setFallbackPick("");
	};

	const removeFallback = (id: string) => {
		setFallbackIds((prev) => prev.filter((x) => x !== id));
	};

	const providerReadiness = resolveProviderReadiness(providerId, providers, config);

	return {
		fallbackIds,
		setFallbackIds,
		fallbackPick,
		setFallbackPick,
		addFallback,
		removeFallback,
		providerReadiness,
	};
}
