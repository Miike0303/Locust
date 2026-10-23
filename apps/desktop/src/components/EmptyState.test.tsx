import assert from "node:assert/strict";
import { test } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router-dom";
import EmptyState from "./EmptyState.tsx";

test("navigation action is directly after the description in the centered group", () => {
  const html = renderToStaticMarkup(<MemoryRouter><EmptyState title="No project" description="Open a project." actionLabel="Go to Home" actionTo="/" /></MemoryRouter>);
  assert.match(html, /justify-center/);
  assert.match(html, /<p[^>]*>No project<\/p><p[^>]*>Open a project\.<\/p><a [^>]*href="\/"[^>]*>Go to Home<\/a><\/div>$/);
  assert.match(html, /focus-visible:ring-2/);
  assert.match(html, /focus-visible:ring-accent-fg/);
});

test("callback actions retain their handler and keyboard focus styling", () => {
  let calls = 0;
  const tree = EmptyState({ title: "Empty", actionLabel: "Add", onAction: () => { calls++; } });
  const button = tree.props.children.flat().find((node: any) => node?.type === "button");
  assert.ok(button);
  button.props.onClick();
  assert.equal(calls, 1);
  assert.equal(button.props.type, "button");
  assert.match(button.props.className, /focus-visible:ring-2/);
});

test("no action is rendered without an actionable label", () => {
  const html = renderToStaticMarkup(<EmptyState title="Empty" actionLabel="Unavailable" />);
  assert.doesNotMatch(html, /<(a|button)\b/);
});


test("optional icon and action slot stay inside the centered empty state", () => {
  const html = renderToStaticMarkup(<EmptyState title="Not checked yet" description="Check the project." icon={<svg aria-hidden="true" />} action={<button type="button" disabled className="bg-accent text-accent-fg">Run validation</button>} />);
  assert.match(html, /justify-center/);
  assert.match(html, /<svg[^>]*><\/svg><p[^>]*>Not checked yet<\/p>/);
  assert.match(html, /<p[^>]*>Check the project\.<\/p><button[^>]*disabled=""[^>]*class="bg-accent text-accent-fg"[^>]*>Run validation<\/button><\/div>$/);
});
