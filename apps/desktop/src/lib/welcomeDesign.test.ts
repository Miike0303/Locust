import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

// Cover conditional branches (including the format picker) without a browser.
const source = readFileSync(new URL("../pages/Welcome.tsx", import.meta.url), "utf8");

test("Welcome has no loose gray palette utilities", () => {
  const literals = source.match(/[\w:/-]*gray-[\w/.-]+/g) ?? [];
  assert.equal(literals.length, 0, `Welcome still uses loose grays: ${literals.slice(0, 5).join(", ")}`);
});

test("Welcome uses scalable type utilities instead of arbitrary pixel text sizes", () => {
  const literals = source.match(/text-\[\d+(?:\.\d+)?px\]/g) ?? [];
  assert.equal(literals.length, 0, `Welcome still uses pixel text sizes: ${literals.join(", ")}`);
});

for (const name of ["PatchModal", "InjectionRecoveryModal"]) {
  test(`Welcome lazy-loads ${name} instead of importing it on Home load`, () => {
    assert.doesNotMatch(source, new RegExp(`import\\s+${name}\\s+from`));
    assert.match(source, new RegExp(`const ${name} = lazy\\(\\(\\) => import\\("\\.\\./components/${name}"\\)\\)`));
  });
}

test("Welcome only mounts modal Suspense boundaries while open, preserving props and handlers", () => {
  assert.match(source, /\{showPatchModal && \(\s*<Suspense fallback=\{null\}>\s*<PatchModal\s+open=\{showPatchModal\}\s+onClose=\{\(\) => setShowPatchModal\(false\)\}\s+allowPack=\{false\}\s*\/>\s*<\/Suspense>\s*\)\}/);
  assert.match(source, /\{recoveryPath !== null && \(\s*<Suspense fallback=\{null\}>\s*<InjectionRecoveryModal defaultGamePath=\{recoveryPath\} onClose=\{\(\) => setRecoveryPath\(null\)\} \/>\s*<\/Suspense>\s*\)\}/);
  assert.match(source, /onClick=\{\(\) => setShowPatchModal\(true\)\}/);
  assert.match(source, /setRecoveryPath\(path\)/);
});

test("the HTML entry declares an SVG favicon instead of a missing favicon.ico", () => {
  const html = readFileSync(new URL("../../index.html", import.meta.url), "utf8");
  assert.match(html, /<link\b[^>]*rel="icon"[^>]*type="image\/svg\+xml"[^>]*href="data:image\/svg\+xml,[^"]+"/);
  assert.ok(html.includes("%23047857"), "icon uses the existing accent color");
});
