import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const css = readFileSync(new URL("../index.css", import.meta.url), "utf8");
const colorTokens = [
  "surface", "surface-muted", "border", "text", "text-muted",
  "accent", "accent-hover", "accent-muted", "accent-fg",
  "danger", "danger-muted", "warning", "warning-muted", "success", "success-muted",
] as const;
const typeTokens = {
  caption: [".75rem", "1rem"],
  body: [".875rem", "1.25rem"],
  section: ["1rem", "1.5rem"],
  page: ["1.25rem", "1.75rem"],
} as const;

// Intentionally inspect the authored CSS, not generated/minified build output.
// A shared selector block contributes to both modes (e.g. mode-independent type).
function declarations(selector: string): Map<string, string> {
  const values = new Map<string, string>();
  const source = css.replace(/\/\*[\s\S]*?\*\//g, "");
  for (const [, prelude, body] of source.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    // Semicolon-only directives (@import/@custom-variant) can precede the first rule.
    const selectors = prelude.slice(prelude.lastIndexOf(";") + 1);
    if (!selectors.split(",").some(value => value.trim() === selector)) continue;
    for (const [, name, value] of body.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) {
      values.set(name, value.trim());
    }
  }
  return values;
}

for (const selector of [":root", ".dark"]) {
  test(`${selector} defines every semantic color and exposes it through @theme inline`, () => {
    const palette = declarations(selector);
    const theme = declarations("@theme inline");
    for (const token of colorTokens) {
      assert.ok(palette.get(`--${token}`), `${selector} is missing --${token}`);
      assert.equal(theme.get(`--color-${token}`), `var(--${token})`, `missing ${token} utility bridge`);
    }
  });

  test(`${selector} keeps typography rem-based and exposes size and line-height utilities`, () => {
    const palette = declarations(selector);
    const theme = declarations("@theme inline");
    for (const [token, [size, lineHeight]] of Object.entries(typeTokens)) {
      assert.equal(palette.get(`--type-${token}`), size);
      assert.equal(palette.get(`--type-${token}-line-height`), lineHeight);
      assert.equal(theme.get(`--text-${token}`), `var(--type-${token})`);
      assert.equal(theme.get(`--text-${token}--line-height`), `var(--type-${token}-line-height)`);
    }
  });

  test(`${selector} text and status colors meet WCAG AA on their surfaces`, () => {
    const palette = declarations(selector);
    const pairs = [
      ["text", "surface"], ["text-muted", "surface"], ["text-muted", "surface-muted"],
      ["accent-fg", "surface"], ["accent-fg", "accent-muted"],
      ["danger", "surface"], ["danger", "danger-muted"],
      ["warning", "surface"], ["warning", "warning-muted"],
      ["success", "surface"], ["success", "success-muted"],
    ];
    for (const [foreground, background] of pairs) {
      const ratio = contrast(palette.get(`--${foreground}`), palette.get(`--${background}`));
      assert.ok(ratio >= 4.5, `${selector} ${foreground} on ${background}: ${ratio.toFixed(2)}:1`);
    }
    for (const token of ["accent", "accent-hover"]) {
      const ratio = contrast("#ffffff", palette.get(`--${token}`));
      assert.ok(ratio >= 4.5, `${selector} white button text on ${token}: ${ratio.toFixed(2)}:1`);
    }
  });
}

function luminance(value: string | undefined): number {
  assert.ok(value && /^#[0-9a-f]{6}$/i.test(value), `expected an explicit sRGB palette color, got ${value}`);
  const channels = [1, 3, 5].map(offset => {
    const channel = parseInt(value.slice(offset, offset + 2), 16) / 255;
    return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
  });
  return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
}

function contrast(foreground: string | undefined, background: string | undefined): number {
  const values = [luminance(foreground), luminance(background)];
  return (Math.max(...values) + 0.05) / (Math.min(...values) + 0.05);
}
