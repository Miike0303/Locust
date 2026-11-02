import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, afterEach, beforeEach, test } from "node:test";
import { setLocale, t } from "../lib/i18n";
import { useEditorStore } from "../stores/editorStore";
import type { StringFilter } from "../lib/api";

// Exercise authored handlers/effects with controlled hooks, real store and fake timers.
const h = {
  slots: [] as any[], cursor: 0, effects: [] as (() => void)[], dirty: false,
  updates: [] as Partial<StringFilter>[], useEditorStore, t,
  useState(initial: any) {
    const index = h.cursor++, slots = h.slots;
    if (!(index in slots)) slots[index] = typeof initial === "function" ? initial() : initial;
    return [slots[index], (value: any) => {
      const next = typeof value === "function" ? value(slots[index]) : value;
      if (!Object.is(next, slots[index])) { slots[index] = next; h.dirty = true; }
    }];
  },
  useRef(initial: any) { return h.slots[h.cursor++] ??= { current: initial }; },
  useEffect(effect: () => void | (() => void), deps?: unknown[]) {
    const index = h.cursor++, previous = h.slots[index];
    if (previous && deps && deps.length === previous.deps?.length && deps.every((dep, i) => Object.is(dep, previous.deps[i]))) return;
    const slot = { deps, cleanup: undefined as void | (() => void) };
    h.slots[index] = slot;
    h.effects.push(() => { previous?.cleanup?.(); slot.cleanup = effect(); });
  },
};
(globalThis as any).__filterBar = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/components/FilterBar.tsx")) {
      const stubs: Record<string, string> = {
        react: "export const useState=h.useState;export const useRef=h.useRef;export const useEffect=h.useEffect;",
        "../lib/i18n": "export const useT=()=>h.t;",
        "../stores/editorStore": "export const useEditorStore=selector=>selector?selector(h.useEditorStore.getState()):h.useEditorStore.getState();",
        "../stores/projectStore": "export const useProjectStore=selector=>selector({project:null});",
        "@tanstack/react-query": "export const useQuery=()=>({data:undefined});",
      };
      if (stubs[specifier]) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__filterBar;" + stubs[specifier])}` };
    }
    return nextResolve(specifier, context);
  },
});
const { default: FilterBar } = await import("./FilterBar");
const originalSetFilter = useEditorStore.getState().setFilter;
const unsubscribe = useEditorStore.subscribe(() => { h.dirty = true; });
let props = { total: 350, showing: 100 };
function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  return Array.isArray(node) ? node.flatMap(nodes) : [node, ...nodes(node.props?.children)];
}
function render() {
  let tree: any[] = [];
  let passes = 0;
  do {
    assert.ok(++passes < 20, "render/effects settle");
    h.dirty = false; h.cursor = 0; h.effects = [];
    tree = nodes(FilterBar(props));
    h.effects.splice(0).forEach(effect => effect());
  } while (h.dirty);
  return tree;
}
function unmount() {
  h.slots.forEach(slot => slot?.cleanup?.());
  h.slots = []; h.effects = []; h.cursor = 0;
}
function search() { return render().find(node => node.type === "input" && "data-search-input" in node.props); }
function typeSearch(value: string) {
  search().props.onChange({ target: { value } });
  render();
}
function pageButton(direction: "previous" | "next") {
  const label = t(direction === "previous" ? "filter.previousPage" : "filter.nextPage");
  const button = render().find(node => node.type === "button" && node.props["aria-label"] === label);
  assert.ok(button, label);
  return button;
}
beforeEach(ctx => {
  assert.ok("mock" in ctx);
  ctx.mock.timers.enable({ apis: ["setTimeout"] });
  h.updates = []; props = { total: 350, showing: 100 }; setLocale("en");
  useEditorStore.setState({ filter: { limit: 100, offset: 200 }, setFilter(patch) {
    h.updates.push(patch);
    originalSetFilter(patch);
  } });
});
afterEach(() => { unmount(); useEditorStore.setState({ setFilter: originalSetFilter }); setLocale("en"); });
after(() => { unsubscribe(); hooks.deregister(); Reflect.deleteProperty(globalThis, "__filterBar"); });

for (const searchText of [undefined, "saved search"]) {
  test(`mount and remount retain offset 200 with ${searchText ? "stored" : "no"} search`, ctx => {
    useEditorStore.setState({ filter: { limit: 100, offset: 200, search: searchText } });
    for (let mount = 0; mount < 2; mount++) {
      assert.equal(search().props.value, searchText || "");
      ctx.mock.timers.tick(1000); render();
      assert.equal(useEditorStore.getState().filter.offset, 200);
      assert.deepEqual(h.updates, []);
      unmount();
    }
  });
}

test("a changed search resets offset exactly once after 300 ms", ctx => {
  typeSearch("new search");
  ctx.mock.timers.tick(299); render();
  assert.equal(useEditorStore.getState().filter.offset, 200);
  assert.deepEqual(h.updates, []);
  ctx.mock.timers.tick(1); render();
  assert.deepEqual(h.updates, [{ search: "new search", offset: 0 }]);
  assert.equal(useEditorStore.getState().filter.offset, 0);
  ctx.mock.timers.tick(1000); render();
  assert.equal(h.updates.length, 1);
});

test("rapid typing restarts the debounce and sends only the latest search", ctx => {
  typeSearch("n"); ctx.mock.timers.tick(200);
  typeSearch("ne"); ctx.mock.timers.tick(200);
  typeSearch("new"); ctx.mock.timers.tick(299); render();
  assert.deepEqual(h.updates, []);
  ctx.mock.timers.tick(1); render();
  assert.deepEqual(h.updates, [{ search: "new", offset: 0 }]);
});

test("unmount cancels a pending search", ctx => {
  typeSearch("unsent"); ctx.mock.timers.tick(299);
  unmount(); ctx.mock.timers.tick(1000);
  assert.deepEqual(h.updates, []);
  assert.equal(useEditorStore.getState().filter.offset, 200);
});

test("typing back to the stored search cancels the pending change", ctx => {
  typeSearch("temporary"); ctx.mock.timers.tick(200);
  typeSearch(""); ctx.mock.timers.tick(1000); render();
  assert.deepEqual(h.updates, []);
  assert.equal(useEditorStore.getState().filter.offset, 200);
});

test("erasing an existing search commits undefined once after the debounce", ctx => {
  useEditorStore.setState({ filter: { limit: 100, offset: 200, search: "old" } });
  typeSearch(""); ctx.mock.timers.tick(300); render();
  assert.deepEqual(h.updates, [{ search: undefined, offset: 0 }]);
  ctx.mock.timers.tick(1000); render();
  assert.equal(h.updates.length, 1);
});

for (const externalSearch of [undefined, "external"]) {
  test(`external ${externalSearch ? "search" : "clear"} replaces pending input without a stale write`, ctx => {
    useEditorStore.setState({ filter: { limit: 100, offset: 200, search: "stored" } });
    typeSearch("stale draft"); ctx.mock.timers.tick(200);
    originalSetFilter({ search: externalSearch, offset: 100 });
    assert.equal(search().props.value, externalSearch || "");
    ctx.mock.timers.tick(1000); render();
    assert.deepEqual(h.updates, []);
    assert.equal(useEditorStore.getState().filter.offset, 100);
    typeSearch("fresh draft"); ctx.mock.timers.tick(300); render();
    assert.deepEqual(h.updates, [{ search: "fresh draft", offset: 0 }]);
  });
}

test("Clear cancels pending search and does not schedule a second filter update", ctx => {
  useEditorStore.setState({ filter: { limit: 100, offset: 200, search: "stored", status: "pending", file_path: "story.json" } });
  typeSearch("stale draft"); ctx.mock.timers.tick(200);
  render().find(node => node.type === "button" && Array.isArray(node.props.children) && node.props.children.includes(t("common.clear"))).props.onClick();
  render(); ctx.mock.timers.tick(1000); render();
  assert.deepEqual(h.updates, [{ status: undefined, search: undefined, file_path: undefined, tag: undefined, offset: 0 }]);
  assert.equal(search().props.value, "");
});

for (const locale of ["en", "es"] as const) {
  test(`${locale}: Previous remains enabled when the last filtered page empties`, ctx => {
    setLocale(locale);
    const filter = { limit: 100, offset: 100, status: "pending", search: "dialogue", file_path: "story.json", tag: "quest" };
    useEditorStore.setState({ filter }); props = { total: 101, showing: 1 }; render();
    props = { total: 100, showing: 0 };
    assert.equal(pageButton("previous").props.disabled, false);
    assert.equal(pageButton("next").props.disabled, true);
    pageButton("previous").props.onClick(); render(); ctx.mock.timers.tick(1000); render();
    assert.deepEqual(h.updates, [{ offset: 0 }]);
    assert.deepEqual(useEditorStore.getState().filter, { ...filter, offset: 0 });
  });
}

for (const total of [0, 100, 101]) {
  test(`Previous recovers to a valid page after total shrinks to ${total}`, () => {
    props = { total, showing: 0 };
    useEditorStore.setState({ filter: { limit: 100, offset: 400 } });
    assert.equal(pageButton("previous").props.disabled, false);
    assert.equal(pageButton("next").props.disabled, true);
    pageButton("previous").props.onClick(); render();
    assert.deepEqual(h.updates, [{ offset: total > 100 ? 100 : 0 }]);
  });
}

for (const limit of [undefined, 50]) {
  test(`normal pagination preserves boundaries with ${limit ? "custom" : "default"} page size`, () => {
    const pageSize = limit ?? 100;
    useEditorStore.setState({ filter: { limit, offset: 0 } });
    props = { total: pageSize * 2 + 1, showing: pageSize };
    assert.equal(pageButton("previous").props.disabled, true);
    assert.equal(pageButton("next").props.disabled, false);
    pageButton("next").props.onClick(); render();
    assert.equal(useEditorStore.getState().filter.offset, pageSize);
    assert.equal(pageButton("previous").props.disabled, false);
    assert.equal(pageButton("next").props.disabled, false);
    pageButton("next").props.onClick(); render();
    assert.equal(useEditorStore.getState().filter.offset, pageSize * 2);
    assert.equal(pageButton("next").props.disabled, true);
    pageButton("previous").props.onClick(); render();
    assert.equal(useEditorStore.getState().filter.offset, pageSize);
    pageButton("previous").props.onClick(); render();
    assert.equal(useEditorStore.getState().filter.offset, 0);
    assert.equal(pageButton("previous").props.disabled, true);
  });
}

test("a single first page has no pagination controls", () => {
  useEditorStore.setState({ filter: { limit: 100, offset: 0 } });
  for (const total of [0, 1, 100]) {
    props = { total, showing: total };
    const tree = render();
    assert.ok(!tree.some(node => node.props["aria-label"] === t("filter.previousPage") || node.props["aria-label"] === t("filter.nextPage")));
  }
});
