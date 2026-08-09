/**
 * Lightweight asserts (run: npx --yes tsx src/lib/registerLangPrefs.test.ts).
 */
import assert from "node:assert/strict";
import {
  loadRegLabelOverride,
  rememberRegLabelOverride,
  type StringStorage,
} from "./registerLangPrefs.ts";

function memStorage(): StringStorage & { data: Map<string, string> } {
  const data = new Map<string, string>();
  return {
    data,
    getItem(key) {
      return data.has(key) ? data.get(key)! : null;
    },
    setItem(key, value) {
      data.set(key, value);
    },
    removeItem(key) {
      data.delete(key);
    },
  };
}

const store = memStorage();
assert.equal(loadRegLabelOverride(store), "");
assert.equal(loadRegLabelOverride(null), "");

rememberRegLabelOverride("  Español  ", store);
assert.equal(loadRegLabelOverride(store), "Español");
assert.equal(store.data.get("locust.inject.regLabel"), "Español");

rememberRegLabelOverride("Português BR", store);
assert.equal(loadRegLabelOverride(store), "Português BR");

// Clear on empty / whitespace
rememberRegLabelOverride("   ", store);
assert.equal(loadRegLabelOverride(store), "");
assert.equal(store.data.has("locust.inject.regLabel"), false);

rememberRegLabelOverride("日本語", store);
assert.equal(loadRegLabelOverride(store), "日本語");
rememberRegLabelOverride("", store);
assert.equal(loadRegLabelOverride(store), "");

console.log("registerLangPrefs.test.ts: ok");
