import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import ts from "typescript";

const forms = [
  { file: "TranslationModal", prefix: "translation", fields: ["provider", "fallbacks", "source", "target", "gameContext", "batchSize", "parallelRequests", "costLimit"] },
  { file: "QueuePanel", prefix: "queue", fields: ["provider", "fallbacks", "source", "target", "gameContext", "batchSize", "maxConcurrent", "costLimit"] },
];

for (const { file, prefix, fields } of forms) {
  const source = readFileSync(new URL(`../components/${file}.tsx`, import.meta.url), "utf8");
  const tree = ts.createSourceFile(`${file}.tsx`, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const nodes: Array<ts.JsxOpeningElement | ts.JsxSelfClosingElement> = [];
  function walk(node: ts.Node): void {
    if (ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) nodes.push(node);
    ts.forEachChild(node, walk);
  }
  walk(tree);
  function attribute(node: typeof nodes[number], name: string): string | undefined {
    const attr = node.attributes.properties.find((p): p is ts.JsxAttribute => ts.isJsxAttribute(p) && p.name.getText(tree) === name);
    return attr?.initializer && ts.isStringLiteral(attr.initializer) ? attr.initializer.text : undefined;
  }
  for (const field of fields) {
    test(`${file}: visible ${field} label names exactly one control`, () => {
      const key = `${prefix === "translation" ? "translate" : "queue"}.${field}`;
      const label = nodes.find((node) => node.tagName.getText(tree) === "label" && node.parent.getText(tree).includes(`t("${key}")`));
      assert.ok(label, `visible ${key} label remains present`);
      const id = attribute(label, "htmlFor");
      assert.ok(id, `${key} must be associated with its control`);
      assert.ok(id.startsWith(`${prefix}-`), "dialog-scoped IDs avoid cross-dialog collisions");
      const controls = nodes.filter((node) => attribute(node, "id") === id);
      assert.equal(controls.length, 1, "label target exists exactly once");
      assert.ok(["input", "select", "textarea"].includes(controls[0].tagName.getText(tree)));
    });
  }
}
