/**
 * Run: npx --yes tsx src/lib/translationCost.test.ts
 */
import assert from "node:assert/strict";
import {
	editorStatsKey,
	formatUsdCost,
	shouldShowTranslationCost,
	translationCompleteToastKey,
} from "./translationCost.ts";

assert.equal(shouldShowTranslationCost(0), false);
assert.equal(shouldShowTranslationCost(-0.01), false);
assert.equal(shouldShowTranslationCost(Number.NaN), false);
assert.equal(shouldShowTranslationCost(0.0001), true);
assert.equal(shouldShowTranslationCost(1.25), true);

assert.equal(formatUsdCost(0), "0.0000");
assert.equal(formatUsdCost(0.01234), "0.0123");
assert.equal(formatUsdCost(1.2), "1.2000");

assert.equal(translationCompleteToastKey(0), "translate.toast.complete");
assert.equal(
	translationCompleteToastKey(0.05),
	"translate.toast.completeWithCost",
);
// Negative: free runs must not pick the with-cost key (would show $0.0000).
assert.notEqual(
	translationCompleteToastKey(0),
	"translate.toast.completeWithCost",
);

assert.equal(editorStatsKey(0), "editor.stats");
assert.equal(editorStatsKey(2), "editor.statsWithCost");

console.log("translationCost.test.ts: ok");
