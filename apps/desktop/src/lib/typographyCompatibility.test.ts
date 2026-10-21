import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { test } from "node:test";
import { compile, type Config } from "tailwindcss";
import postcss, { type Rule } from "postcss";

const require = createRequire(import.meta.url);
const typography = require("@tailwindcss/typography");

// Exercise the public plugin/config/compiler APIs, not parser internals. In parser 7,
// insertion during container iteration changed; Typography removes common trailing
// pseudo-elements and prepends them into a new selector. These cases protect the
// resulting CSS rather than requiring the parser's old iteration semantics.
export const compatibilityConfig: Config = {
  theme: {
    extend: {
      typography: {
        compat: {
          css: {
            "h2::before, h3::before": { content: '"common"' },
            "h2::before::marker, h3::before::marker": { color: "rebeccapurple" },
            "h2::before, h3::after": { content: '"different"' },
            "a:hover::after, a:focus::after": { content: '"state"' },
            '[data-label="a,b"]::before, .escaped\\:name::before': { content: '"escaped"' },
            "> p:first-child::before": { content: '"child"' },
            ":is(h2, h3)::after": { content: '"nested"' },
            "h4::before, h5": { content: '"uneven"' },
          },
        },
      },
    },
  },
};

const elements = [
  "headings", "h1", "h2", "h3", "h4", "h5", "h6", "p", "a", "blockquote",
  "figure", "figcaption", "strong", "em", "kbd", "code", "pre", "ol", "ul", "li",
  "dl", "dt", "dd", "table", "thead", "tr", "th", "td", "img", "picture", "video", "hr", "lead",
];
const colors = [
  "slate", "gray", "zinc", "neutral", "stone", "red", "orange", "amber", "yellow", "lime",
  "green", "emerald", "teal", "cyan", "sky", "blue", "indigo", "violet", "purple", "fuchsia", "pink", "rose",
];
export const typographyInput = '@import "tailwindcss/theme.css";\n@tailwind utilities;\n@plugin "@tailwindcss/typography";\n@config "typography-compatibility";\n';
export const typographyCandidates = [
  "prose", "prose-sm", "prose-base", "prose-lg", "prose-xl", "prose-2xl", "prose-invert", "prose-compat",
  ...colors.map(color => `prose-${color}`),
  ...elements.map(element => `prose-${element}:underline`),
  "hover:prose-a:underline", "prose-a:hover:underline", "dark:prose-invert", "md:prose-lg",
];

export async function generateTypographyCss(options: { className?: string; target?: string } = {}) {
  const compiler = await compile(typographyInput, {
    async loadStylesheet(id) {
      assert.equal(id, "tailwindcss/theme.css");
      return { path: id, base: "", content: readFileSync(require.resolve(id), "utf8") };
    },
    async loadModule(id) {
      assert.ok(id === "@tailwindcss/typography" || id === "typography-compatibility");
      return {
        path: id, base: "",
        module: id === "@tailwindcss/typography" ? typography(options) : compatibilityConfig,
      };
    },
  });
  const name = options.className ?? "prose";
  return compiler.build(typographyCandidates.map(candidate => candidate.replaceAll("prose", name)));
}

const generated = generateTypographyCss();
const exclusion = ':not(:where([class~="not-prose"],[class~="not-prose"] *))';
function rule(css: string, selector: string): Rule {
  let found: Rule | undefined;
  postcss.parse(css).walkRules(value => { if (value.selector === selector) found = value; });
  assert.ok(found, `missing generated selector ${selector}`);
  return found;
}
function declaration(value: Rule, property: string) {
  return value.nodes.find(node => node.type === "decl" && node.prop === property)?.toString();
}

test("Typography generates default, every size, palette and inverse declarations", async () => {
  const css = await generated;
  assert.equal(declaration(rule(css, ".prose"), "max-width"), "max-width: 65ch");
  for (const [size, font] of [["sm", "0.875rem"], ["base", "1rem"], ["lg", "1.125rem"], ["xl", "1.25rem"], ["2xl", "1.5rem"]]) {
    assert.equal(declaration(rule(css, `.prose-${size}`), "font-size"), `font-size: ${font}`);
  }
  for (const color of colors) assert.ok(declaration(rule(css, `.prose-${color}`), "--tw-prose-links"));
  assert.equal(declaration(rule(css, ".prose-invert"), "--tw-prose-body"), "--tw-prose-body: var(--tw-prose-invert-body)");
  rule(css, ".dark\\:prose-invert");
  rule(css, ".md\\:prose-lg");
});

test("Typography keeps low specificity, not-prose exclusion and element variants", async () => {
  const css = await generated;
  rule(css, `:where(p)${exclusion}`);
  rule(css, `:where(.prose > ul > li p)${exclusion}`);
  rule(css, `:where(ul > li)${exclusion}::marker`);
  rule(css, `:where(code)${exclusion}::before`);
  rule(css, `:where(code)${exclusion}::after`);
  for (const element of elements) {
    const variant = rule(css, `.prose-${element}\\:underline`);
    assert.match(variant.toString(), /text-decoration-line: underline/);
    assert.ok(variant.toString().includes(exclusion));
  }
  rule(css, ".hover\\:prose-a\\:underline");
  rule(css, ".prose-a\\:hover\\:underline");
});

test("public theme selectors preserve common, chained and non-common pseudo tails", async () => {
  const css = await generated;
  for (const [selector, tail] of [
    ["h2, h3", "::before"], ["h2, h3", "::before::marker"],
    ["h2::before, h3::after", ""], ["a:hover, a:focus", "::after"],
    ['[data-label="a,b"], .escaped\\:name', "::before"],
    [".prose-compat > p:first-child", "::before"], [":is(h2, h3)", "::after"],
    ["h4, h5", "::before"],
  ]) {
    rule(css, `:where(${selector})${exclusion}${tail}`);
  }
});

test("custom className transforms selectors and opt-out without leaking prose", async () => {
  const css = await generateTypographyCss({ className: "article" });
  rule(css, ".article");
  rule(css, ':where(.article > ul > li p):not(:where([class~="not-article"],[class~="not-article"] *))');
  rule(css, ".article-a\\:underline");
  assert.ok(!css.includes("not-prose"));
});

test("legacy target preserves unwrapped selectors and element variant generation", async () => {
  const css = await generateTypographyCss({ target: "legacy" });
  rule(css, ".prose");
  rule(css, "h2::before, h3::before");
  rule(css, ".prose-a\\:underline");
  assert.ok(!css.includes("not-prose"));
});
