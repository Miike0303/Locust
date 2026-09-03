/**
 * When to surface translation spend that the engine already records.
 * Free/mock runs stay at $0 — showing "$0.0000" in the toast would be noise.
 */
export function shouldShowTranslationCost(costUsd: number): boolean {
	return Number.isFinite(costUsd) && costUsd > 0;
}

/** Matches Activity Log / Settings → History precision. */
export function formatUsdCost(costUsd: number): string {
	return costUsd.toFixed(4);
}

export type TranslationCompleteToastKey =
	| "translate.toast.complete"
	| "translate.toast.completeWithCost";

export function translationCompleteToastKey(
	costUsd: number,
): TranslationCompleteToastKey {
	return shouldShowTranslationCost(costUsd)
		? "translate.toast.completeWithCost"
		: "translate.toast.complete";
}

export type EditorStatsKey = "editor.stats" | "editor.statsWithCost";

export function editorStatsKey(costUsd: number): EditorStatsKey {
	return shouldShowTranslationCost(costUsd)
		? "editor.statsWithCost"
		: "editor.stats";
}
