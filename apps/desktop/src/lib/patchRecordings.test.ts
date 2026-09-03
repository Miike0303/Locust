/**
 * Lightweight asserts for pack vs injection recordings
 * (run: npx --yes tsx src/lib/patchRecordings.test.ts).
 */
import {
	canPackFromRecordings,
	isPackLangSelected,
	packLangFromRecording,
	preferredPackLang,
} from "./patchRecordings.ts";

const assert = {
	equal(actual: unknown, expected: unknown, message?: string) {
		if (actual !== expected)
			throw new Error(message ?? `${actual} !== ${expected}`);
	},
	ok(cond: unknown, message?: string) {
		if (!cond) throw new Error(message ?? "expected truthy");
	},
};

assert.equal(packLangFromRecording("es"), "es");
assert.equal(packLangFromRecording(null), "");

assert.ok(isPackLangSelected("es", "es"));
assert.ok(isPackLangSelected("", null));
assert.equal(isPackLangSelected("es", null), false);
assert.equal(isPackLangSelected("fr", "es"), false);

assert.equal(canPackFromRecordings([], "es"), false);
assert.equal(canPackFromRecordings([], ""), false);
assert.ok(canPackFromRecordings(["es"], "es"));
assert.ok(canPackFromRecordings(["es"], ""));
assert.ok(canPackFromRecordings([null], ""));
assert.equal(canPackFromRecordings(["es", "fr"], ""), false);
assert.ok(canPackFromRecordings(["es", "fr"], "es"));
// Negative: inventing a language that was never injected must not pack.
assert.equal(canPackFromRecordings(["es"], "ja"), false);

assert.equal(preferredPackLang(["es"], "fr", ""), "es");
assert.equal(preferredPackLang(["es", "fr"], "fr", ""), "fr");
assert.equal(preferredPackLang(["es", "fr"], "de", ""), "");
assert.equal(preferredPackLang(["es"], "fr", "kept"), "kept");
assert.equal(preferredPackLang([null], undefined, ""), "");

console.log("patchRecordings.test.ts: ok");
