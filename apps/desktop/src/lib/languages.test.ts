/**
 * Lightweight asserts for languages (run: npx --yes tsx src/lib/languages.test.ts).
 */
import assert from "node:assert/strict";
import { LANGUAGES, languageLabel } from "./languages.ts";

// Non-empty
assert.ok(LANGUAGES.length > 0, "language list must not be empty");

// No duplicate codes
const codes = LANGUAGES.map((l) => l.code);
assert.equal(new Set(codes).size, codes.length, "duplicate language codes");

// BCP-47-ish: lowercase primary subtag, optional title/upper region subtag (e.g. zh-CN, pt-BR)
const BCP47ISH = /^[a-z]{2,3}(-[A-Z][A-Za-z]{1,3})?$/;
for (const { code, label } of LANGUAGES) {
  assert.match(code, BCP47ISH, `code not BCP-47-ish: ${code}`);
  assert.ok(label.trim().length > 0, `empty label for ${code}`);
}

// Label lookup falls back to the code
assert.equal(languageLabel("es"), "Español");
assert.equal(languageLabel("xx"), "xx");

console.log("languages.test.ts: ok");
