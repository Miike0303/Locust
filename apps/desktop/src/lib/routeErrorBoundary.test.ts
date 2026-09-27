/**
 * Route crash screen: a failed lazy chunk or render error must show a
 * recoverable alert instead of a blank window
 * (run: npx --yes tsx src/lib/routeErrorBoundary.test.ts).
 */
import assert from "node:assert/strict";
import { createElement, isValidElement, type ReactElement, type ReactNode } from "react";
import RouteErrorBoundary from "../components/RouteErrorBoundary.tsx";
import { en } from "./i18n/en.ts";
import { es } from "./i18n/es.ts";
import { setLocale } from "./i18n/index.ts";

type AnyProps = { [key: string]: unknown; children?: ReactNode };

function walk(node: ReactNode, visit: (el: ReactElement<AnyProps>) => void): void {
  if (Array.isArray(node)) {
    for (const child of node) walk(child, visit);
    return;
  }
  if (!isValidElement<AnyProps>(node)) return;
  visit(node);
  walk(node.props.children, visit);
}

function find(node: ReactNode, pred: (el: ReactElement<AnyProps>) => boolean): ReactElement<AnyProps>[] {
  const out: ReactElement<AnyProps>[] = [];
  walk(node, (el) => {
    if (pred(el)) out.push(el);
  });
  return out;
}

function textOf(node: ReactNode): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (isValidElement<AnyProps>(node)) return textOf(node.props.children);
  return "";
}

const CRASH_KEYS = [
  "route.crash.title",
  "route.crash.body",
  "route.crash.reload",
  "route.crash.sidebarHint",
  "route.crash.details",
] as const;

// Both catalogs carry the crash copy; Spanish is actually translated.
for (const key of CRASH_KEYS) {
  assert.ok(en[key]?.trim(), `en has ${key}`);
  assert.ok(es[key]?.trim(), `es has ${key}`);
  assert.notEqual(es[key], en[key], `es ${key} is translated`);
}
assert.equal(es["route.crash.title"], "Esta pantalla dejó de funcionar");

// A rejected dynamic import becomes an error state.
const chunkError = new Error("Failed to fetch dynamically imported module");
const derived = RouteErrorBoundary.getDerivedStateFromError(chunkError);
assert.equal(derived.error, chunkError, "error state keeps the thrown error");
const wrapped = RouteErrorBoundary.getDerivedStateFromError("boom");
assert.ok(wrapped.error instanceof Error, "non-Error throws are normalised");
assert.equal(wrapped.error?.message, "boom");

// No error: children render unchanged.
const child = createElement("p", null, "page");
const healthy = new RouteErrorBoundary({ children: child });
assert.equal(healthy.render(), child, "healthy boundary renders its children");

// Error: an alert with a working Reload button and the technical message.
setLocale("es");
let reloads = 0;
const crashed = new RouteErrorBoundary({ children: child, reload: () => { reloads += 1; } });
crashed.state = derived;
const tree = crashed.render();
assert.notEqual(tree, child, "crashed boundary does not render the failed page");

const alerts = find(tree, (el) => el.props.role === "alert");
assert.equal(alerts.length, 1, "exactly one role=alert container");
const alertText = textOf(alerts[0]);
assert.ok(alertText.includes(es["route.crash.title"]), "Spanish title shown");
assert.ok(alertText.includes(es["route.crash.body"]), "Spanish body shown");
assert.ok(alertText.includes(chunkError.message), "error message available for bug reports");

const details = find(tree, (el) => el.type === "details");
assert.equal(details.length, 1, "technical details are collapsible");

const buttons = find(tree, (el) => el.type === "button");
assert.equal(buttons.length, 1, "one Reload button");
assert.ok(textOf(buttons[0]).includes(es["route.crash.reload"]));
const onClick = buttons[0].props.onClick;
assert.equal(typeof onClick, "function", "Reload button has a handler");
(onClick as () => void)();
assert.equal(reloads, 1, "Reload calls the injected reload exactly once");

// componentDidCatch logs instead of swallowing silently.
const originalError = console.error;
const logged: unknown[][] = [];
console.error = (...args: unknown[]) => { logged.push(args); };
try {
  crashed.componentDidCatch(chunkError, { componentStack: "\n    at Editor" });
} finally {
  console.error = originalError;
}
assert.equal(logged.length, 1, "crash is logged once");
assert.ok(logged[0].includes(chunkError), "log carries the error");

setLocale("en");
console.log("routeErrorBoundary.test.ts: ok");
