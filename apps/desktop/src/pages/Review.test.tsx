import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import type { StringEntry } from "../lib/api.ts";
import { setLocale, t } from "../lib/i18n/index.ts";

type Patch = Partial<Pick<StringEntry, "translation" | "status">>;
type Effect = { deps?: unknown[]; cleanup?: () => void };
const row = (id: string): StringEntry => ({
  id, source: `Source ${id}`, translation: `Translation ${id}`, status: "translated",
  file_path: "dialogue.json", context: null, tags: [], provider_used: null,
  char_limit: null, created_at: "", translated_at: null, reviewed_at: null,
});

// Follow the existing component tests' hook seam, but run dependency-aware
// effects as well so navigation exercises the real textarea reset behavior.
const h = {
  values: [] as any[], cursor: 0, dirty: false,
  effects: [] as (() => void)[], pending: [] as Promise<void>[], errors: [] as unknown[],
  rows: {} as Record<string, StringEntry>, writes: [] as { id: string; patch: Patch }[],
  failure: null as unknown, waitForSave: null as Promise<void> | null, invalidations: 0,
  keydown: null as ((event: any) => void) | null, navigations: [] as string[],
  t,
  state(initial: any) {
    const index = this.cursor++;
    if (!(index in this.values)) this.values[index] = typeof initial === "function" ? initial() : initial;
    return [this.values[index], (value: any) => {
      const next = typeof value === "function" ? value(this.values[index]) : value;
      if (!Object.is(next, this.values[index])) this.dirty = true;
      this.values[index] = next;
    }];
  },
  ref(initial: unknown) { return this.state(() => ({ current: initial }))[0]; },
  effect(callback: () => void | (() => void), deps?: unknown[]) {
    const index = this.cursor++;
    const previous = this.values[index] as Effect | undefined;
    if (previous && deps && previous.deps?.length === deps.length &&
      deps.every((dep, i) => Object.is(dep, previous.deps![i]))) return;
    const effect: Effect = { deps };
    this.values[index] = effect;
    this.effects.push(() => { previous?.cleanup?.(); effect.cleanup = callback() || undefined; });
  },
  callback(callback: (...args: any[]) => any, deps: unknown[]) {
    const index = this.cursor++;
    const previous = this.values[index];
    if (previous && deps.every((dep, i) => Object.is(dep, previous.deps[i]))) return previous.callback;
    const wrapped = (...args: any[]) => {
      const result = callback(...args);
      if (result instanceof Promise) {
        this.pending.push(result.then(() => {}, error => { this.errors.push(error); }));
      }
      return result;
    };
    this.values[index] = { deps, callback: wrapped };
    return wrapped;
  },
  speakers: [] as { id: string; name: string; count: number; entry_ids: string[]; note: string }[],
  format: "renpy", contextFailure: false, contextWaits: {} as Record<string, Promise<void> | undefined>,
  focused: 0,
  reads: [] as { status?: string; file_path?: string; limit?: number; offset?: number }[],
  async getStrings(filter: { status?: string; file_path?: string; limit?: number; offset?: number }) {
    this.reads.push(filter);
    if (filter.file_path) {
      if (this.contextWaits[filter.file_path]) await this.contextWaits[filter.file_path];
      if (this.contextFailure) throw new Error("Context unavailable");
    }
    const entries = Object.values(this.rows).filter(entry =>
      (!filter.status || entry.status === filter.status) && (!filter.file_path || entry.file_path === filter.file_path));
    const offset = filter.offset ?? 0;
    return { entries: entries.slice(offset, offset + (filter.limit ?? entries.length)), total: entries.length,
      offset, limit: filter.limit ?? entries.length };
  },
  async patchString(id: string, patch: Patch) {
    this.writes.push({ id, patch });
    if (this.waitForSave) await this.waitForSave;
    if (this.failure !== null) throw this.failure;
    this.rows[id] = { ...this.rows[id], ...patch };
    return this.rows[id];
  },
};
(globalThis as any).__reviewTest = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/pages/Review.tsx")) {
      const stubs: Record<string, string> = {
        react: `const h=globalThis.__reviewTest;export const useState=x=>h.state(x);export const useRef=x=>h.ref(x);export const useEffect=(fn,deps)=>h.effect(fn,deps);export const useCallback=(fn,deps)=>h.callback(fn,deps);`,
        "react-router-dom": `export const useNavigate=()=>path=>globalThis.__reviewTest.navigations.push(path);`,
        "@tanstack/react-query": `export const useQueryClient=()=>({invalidateQueries:async()=>{globalThis.__reviewTest.invalidations++;}});`,
        "../stores/projectStore": `export const useProjectStore=select=>select({project:{path:'/game',database_path:'/game.db',format_id:globalThis.__reviewTest.format}});`,
        "../lib/i18n": `export const useT=()=>globalThis.__reviewTest.t;`,
        "../lib/api": `const h=globalThis.__reviewTest;export const getStrings=filter=>h.getStrings(filter);export const patchString=(id,patch)=>h.patchString(id,patch);export const getSpeakers=async()=>({groups:h.speakers,unassigned_entry_ids:[]});`,
      };
      if (stubs[specifier]) return { url: `data:text/javascript,${encodeURIComponent(stubs[specifier])}`, shortCircuit: true };
    }
    return nextResolve(specifier, context);
  },
});
const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
const previousDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
Object.defineProperty(globalThis, "window", {
  configurable: true, value: {
    addEventListener(_type: string, handler: typeof h.keydown) { h.keydown = handler; },
    removeEventListener() { h.keydown = null; },
  },
});
Object.defineProperty(globalThis, "document", {
  configurable: true, value: { documentElement: { lang: "en" }, querySelector(selector: string) {
    return selector === "#review-textarea" ? { focus() { h.focused++; } } : null;
  } },
});
const { default: Review, reviewApprovalPatch } = await import("./Review.tsx");
after(() => {
  hooks.deregister();
  if (previousWindow) Object.defineProperty(globalThis, "window", previousWindow);
  else Reflect.deleteProperty(globalThis, "window");
  if (previousDocument) Object.defineProperty(globalThis, "document", previousDocument);
  else Reflect.deleteProperty(globalThis, "document");
  Reflect.deleteProperty(globalThis, "__reviewTest");
  setLocale("en");
});

function render() {
  for (let attempt = 0; attempt < 20; attempt++) {
    h.cursor = 0;
    h.dirty = false;
    const tree = Review();
    for (const effect of h.effects.splice(0)) effect();
    if (!h.dirty) return tree;
  }
  throw new Error("Review did not settle");
}
function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(nodes);
  return [node, ...nodes(node.props?.children)];
}
function text(node: any): string {
  if (typeof node === "string") return node;
  if (Array.isArray(node)) return node.map(text).join("");
  return node && typeof node === "object" ? text(node.props?.children) : "";
}
function button(label: string) {
  const found = nodes(render()).find(node => node.type === "button" && text(node).trim().startsWith(label));
  assert.ok(found, `missing action: ${label}`);
  return found;
}
function textarea() { return nodes(render()).find(node => node.type === "textarea"); }
function edit(value: string) { textarea().props.onChange({ target: { value } }); }
function progress(current: number, approved: number, total = 2) {
  assert.ok(text(render()).includes(t("review.progress", { current, total, approved })), text(render()));
}
async function settle() {
  for (let round = 0; round < 12; round++) {
    await Promise.all(h.pending.splice(0));
    render();
  }
}
async function approve() {
  button(t("review.approve")).props.onClick();
  await settle();
}
beforeEach(async () => {
  setLocale("en");
  h.values = []; h.effects = []; h.pending = []; h.errors = [];
  h.rows = { A: row("A"), B: row("B") }; h.writes = [];
  h.failure = null; h.waitForSave = null; h.invalidations = 0;
  h.keydown = null; h.navigations = []; h.speakers = []; h.reads = [];
  h.format = "renpy"; h.contextFailure = false; h.contextWaits = {}; h.focused = 0;
  render();
  await settle();
  assert.equal(textarea().props.value, "Translation A");
});

test("the approval decision preserves approved status and distinguishes cleared text from no change", () => {
  assert.deepEqual(reviewApprovalPatch(row("A"), "Translation A"), { status: "approved" });
  assert.deepEqual(reviewApprovalPatch({ ...row("A"), status: "reviewed" }, "Translation A"), { status: "approved" });
  const approved = { ...row("A"), status: "approved" as const };
  assert.equal(reviewApprovalPatch(approved, "Translation A"), null);
  assert.deepEqual(reviewApprovalPatch(approved, "Revised A"), { translation: "Revised A", status: "approved" });
  assert.deepEqual(reviewApprovalPatch(approved, ""), { translation: "", status: "approved" });
  assert.equal(reviewApprovalPatch({ ...approved, translation: null }, ""), null);
});

test("approve A, previous, edit and approve persists the revision and counts A once", async () => {
  await approve();
  progress(2, 1);
  button(t("review.previous")).props.onClick();
  edit("Revised A");
  await approve();
  assert.equal(h.rows.A.translation, "Revised A");
  assert.equal(h.rows.A.status, "approved");
  progress(2, 1);
  button(t("review.previous")).props.onClick();
  assert.equal(textarea().props.value, "Revised A", "the local cache must retain the saved revision");
});

test("approving an unchanged previously approved row advances without a write", async () => {
  await approve();
  button(t("review.previous")).props.onClick();
  h.writes = [];
  await approve();
  assert.equal(h.writes.length, 0);
  progress(2, 1);
});

for (const revisited of [false, true]) {
  test(`${revisited ? "repeated" : "first"} approval failure shows the error, stays on A and keeps the edit for retry`, async () => {
    if (revisited) {
      await approve();
      button(t("review.previous")).props.onClick();
    }
    edit("Unsaved A");
    h.failure = revisited ? "Cannot save revision" : new Error("Cannot save revision");
    const invalidations = h.invalidations;
    await approve();
    assert.equal(textarea().props.value, "Unsaved A");
    progress(1, revisited ? 1 : 0);
    assert.equal(h.rows.A.translation, "Translation A");
    assert.equal(h.invalidations, invalidations);
    assert.ok(text(render()).includes("Cannot save revision"), "save errors must be visible alongside the edit");
    assert.deepEqual(h.errors, [], "approval must handle save rejections");
    h.failure = null;
    await approve();
    assert.equal(h.rows.A.translation, "Unsaved A");
    assert.equal(h.rows.A.status, "approved");
    progress(2, 1);
    assert.ok(!text(render()).includes("Cannot save revision"), "successful retry clears the error");
  });
}

test("a revised approval waits for persistence before advancing", async () => {
  await approve();
  button(t("review.previous")).props.onClick();
  edit("Delayed A");
  let finish!: () => void;
  h.waitForSave = new Promise<void>(resolve => { finish = resolve; });
  button(t("review.approve")).props.onClick();
  try {
    progress(1, 1);
    assert.equal(textarea().props.value, "Delayed A");
    assert.equal(h.rows.A.translation, "Translation A");
  } finally {
    finish();
    await settle();
  }
  assert.equal(h.rows.A.translation, "Delayed A");
  progress(2, 1);
});

async function threeRows() {
  h.values = []; h.effects = []; h.pending = [];
  h.rows = { A: row("A"), B: row("B"), C: row("C") };
  render();
  await settle();
}

test("two approvals in the same render submit A once and land on B with one approval", async () => {
  await threeRows();
  let finish!: () => void;
  h.waitForSave = new Promise<void>(resolve => { finish = resolve; });
  const click = button(t("review.approve")).props.onClick;
  click();
  click();
  finish();
  await settle();
  assert.deepEqual(h.writes.map(write => write.id), ["A"]);
  assert.equal(textarea().props.value, "Translation B");
  progress(2, 1, 3);
  assert.equal(h.rows.B.status, "translated");
});

test("pending approval disables actions and synchronously blocks stale clicks and hotkeys", async () => {
  await threeRows();
  button(t("review.skip")).props.onClick(); // B lets Previous exercise a real move.
  const actions = ["review.approve", "review.skip", "review.previous", "common.exit"] as const;
  const staleClicks = actions.map(key => button(t(key)).props.onClick);
  let finish!: () => void;
  h.waitForSave = new Promise<void>(resolve => { finish = resolve; });
  staleClicks[0]();
  try {
    for (const click of staleClicks) click();
    for (const key of ["a", "s", "p", "e", "Escape", "Enter"]) {
      h.keydown?.({ key, ctrlKey: key === "Enter", target: null, preventDefault() {} });
    }
    for (const key of actions) assert.equal(button(t(key)).props.disabled, true, key);
    assert.equal(textarea().props.disabled, true, "pending edits must not be lost on advance");
    assert.equal(textarea().props.value, "Translation B");
    assert.deepEqual(h.navigations, []);
    assert.deepEqual(h.writes.map(write => write.id), ["B"]);
    progress(2, 0, 3);
  } finally {
    finish();
    await settle();
  }
  assert.equal(textarea().props.value, "Translation C");
  progress(3, 1, 3);
  for (const key of actions) assert.equal(button(t(key)).props.disabled, false, key);
});

test("a failed delayed approval releases the guard and retries the same row", async () => {
  let finish!: () => void;
  h.waitForSave = new Promise<void>(resolve => { finish = resolve; });
  h.failure = new Error("Delayed failure");
  button(t("review.approve")).props.onClick();
  finish();
  await settle();
  progress(1, 0);
  assert.equal(button(t("review.approve")).props.disabled, false);
  h.failure = null;
  await approve();
  progress(2, 1);
});

test("Review header uses the shared status badge and success progress color", async () => {
  const { StatusBadge } = await import("../lib/statusStyles.ts");
  const header = () => render().props.children[0];
  const badge = () => nodes(header()).find(node => node.type === StatusBadge);
  assert.equal(badge()?.props.status, "translated");
  assert.ok(nodes(header()).some(node => node.props?.className?.includes("bg-success")));
  await approve();
  button(t("review.previous")).props.onClick();
  assert.equal(badge()?.props.status, "approved");
});

async function startWith(entries: StringEntry[]) {
  h.values = []; h.effects = []; h.pending = [];
  h.rows = Object.fromEntries(entries.map(entry => [entry.id, entry]));
  render();
  await settle();
}
function contextRows() {
  const panel = nodes(render()).find(node => node.type === "aside");
  assert.ok(panel, "context panel is visible");
  return nodes(panel).filter(node => node.type === "li");
}

test("context shows the first and last file edges, including rows outside the review queue", async () => {
  const entries = Array.from({ length: 9 }, (_, i) => ({ ...row(`r${i}`), status: "pending" as StringEntry["status"] }));
  entries[0].status = "translated";
  entries[8].status = "translated";
  await startWith([...entries, { ...row("other"), file_path: "other.json" }]);
  assert.deepEqual(contextRows().map(node => node.props["data-entry-id"]), ["r0", "r1", "r2", "r3"]);
  assert.equal(contextRows()[0].props["aria-current"], true);
  button(t("review.skip")).props.onClick();
  await settle();
  assert.deepEqual(contextRows().map(node => node.props["data-entry-id"]), ["r5", "r6", "r7", "r8"]);
  assert.ok(contextRows()[3].props.className.includes("border-accent"));
  assert.ok(contextRows().every(node => !node.props.onClick));
  assert.ok(h.reads.some(filter => filter.file_path === "dialogue.json" && !filter.status));
});

test("context crosses a page boundary and keeps three rows on either side in locator order", async () => {
  const entries = Array.from({ length: 60 }, (_, i) => ({ ...row(`r${i}`), status: "pending" as StringEntry["status"] }));
  entries[49].status = "translated";
  await startWith(entries);
  assert.deepEqual(contextRows().map(node => node.props["data-entry-id"]), ["r46", "r47", "r48", "r49", "r50", "r51", "r52"]);
  assert.ok(h.reads.some(filter => filter.file_path === "dialogue.json" && filter.offset === 50));
});

test("known speaker membership is shown on the card and context; arbitrary context is not a speaker", async () => {
  h.speakers = [{ id: "alice", name: "Alice", count: 1, entry_ids: ["A"], note: "" }];
  await startWith([{ ...row("A"), context: "scene 4", tags: ["dialogue"] }, row("B")]);
  assert.ok(nodes(render()).some(node => node.props?.["data-speaker"] === "Alice" && text(node) === "Alice"));
  assert.ok(nodes(contextRows()[0]).some(node => node.type === "strong" && text(node) === "Alice"));
  assert.ok(!nodes(render()).some(node => node.props?.["data-speaker"] === "scene 4"));
  const path = nodes(render()).find(node => node.props?.title === "dialogue.json");
  assert.ok(path?.props.className.includes("truncate"));
});

for (const locale of ["en", "es"] as const) {
  test(`placeholder chips and live present/missing/extra checks use ${locale} labels and occurrence counts`, async () => {
    setLocale(locale);
    await startWith([{ ...row("A"), source: "Pay [gold] {b}", translation: "Pague [gold] {b}" }]);
    const chips = nodes(render()).filter(node => node.props?.["data-placeholder"]);
    assert.deepEqual(chips.map(text), ["[gold]", "{b}"]);
    assert.ok(chips.every(node => node.props.className.includes("font-mono")));
    let check = nodes(render()).find(node => node.props?.role === "status");
    assert.equal(text(check), locale === "en" ? "Placeholders: 2/2 present" : "Marcadores: 2/2 presentes");
    assert.ok(check.props.className.includes("text-success"));
    edit("Pague {b} {b}");
    check = nodes(render()).find(node => node.props?.role === "status");
    assert.equal(text(check), locale === "en" ? "Placeholders: missing [gold]; extra {b}" : "Marcadores: faltan [gold]; sobran {b}");
    assert.ok(check.props.className.includes("text-warning"));
    edit("");
    assert.ok(text(nodes(render()).find(node => node.props?.role === "status")).includes("[gold], {b}"));
    assert.ok(!textarea().props.className.includes("font-mono"));
    assert.ok(textarea().props.className.includes("leading-relaxed"));
    button(t("review.showDiff")).props.onClick();
    assert.ok(button(t("review.hideDiff")));
  });
}

test("repeated source placeholders require the same number of translated occurrences", async () => {
  await startWith([{ ...row("A"), source: "[gold] and [gold]", translation: "[gold]" }]);
  assert.equal(text(nodes(render()).find(node => node.props?.role === "status")), "Placeholders: missing [gold]");
});

test("review content stacks below 1100px and uses a centered two-to-one layout above it", () => {
  const layout = nodes(render()).find(node => node.props?.className?.includes("max-w-5xl"));
  assert.ok(layout?.props.className.includes("min-[1100px]:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]"));
  assert.ok(layout.props.className.includes("gap-6"));
  assert.ok(layout.props.className.includes("mx-auto"));
});

test("review shortcuts and key hints remain available and ignore textarea typing", async () => {
  const key = (value: string, target: any = null, ctrlKey = false) =>
    h.keydown?.({ key: value, ctrlKey, target, preventDefault() {} });
  for (const value of ["a", "s", "p", "Escape"]) key(value, { tagName: "TEXTAREA" });
  assert.equal(textarea().props.value, "Translation A");
  assert.equal(h.writes.length, 0);
  assert.deepEqual(h.navigations, []);
  key("e");
  assert.equal(h.focused, 1);
  key("s");
  assert.equal(textarea().props.value, "Translation B");
  key("p");
  assert.equal(textarea().props.value, "Translation A");
  key("Enter", null, true);
  await settle();
  assert.deepEqual(h.writes.map(write => write.id), ["A"]);
  assert.deepEqual(nodes(render()).filter(node => node.type === "kbd").map(text), ["P", "S", "A"]);
  key("a");
  await settle();
  assert.deepEqual(h.writes.map(write => write.id), ["A", "B"]);
  key("Escape");
  assert.deepEqual(h.navigations, ["/editor"]);
});

test("pending context shows loading and ignores a late response after moving to another file", async () => {
  let finish!: () => void;
  h.contextWaits["dialogue.json"] = new Promise<void>(resolve => { finish = resolve; });
  try {
    await startWith([row("A"), { ...row("B"), file_path: "other.json" }]);
    assert.ok(text(render()).includes("Loading surrounding entries…"));
    assert.deepEqual(contextRows().map(node => node.props["data-entry-id"]), ["A"]);
    button(t("review.skip")).props.onClick();
    await settle();
    assert.deepEqual(contextRows().map(node => node.props["data-entry-id"]), ["B"]);
  } finally {
    finish();
    await settle();
  }
  assert.deepEqual(contextRows().map(node => node.props["data-entry-id"]), ["B"]);
});

for (const locale of ["en", "es"] as const) {
  test(`context failure keeps the current row and shows a ${locale} message without blocking approval`, async () => {
    setLocale(locale);
    h.contextFailure = true;
    await startWith([row("A"), row("B")]);
    assert.ok(text(render()).includes(locale === "en" ? "Could not load surrounding entries." : "No se pudieron cargar las entradas cercanas."));
    assert.deepEqual(contextRows().map(node => node.props["data-entry-id"]), ["A"]);
    await approve();
    assert.equal(h.rows.A.status, "approved");
  });
}

test("context reuses fetched file pages and reflects saved and unsaved translations", async () => {
  await startWith([row("A"), row("B")]);
  const reads = h.reads.filter(filter => filter.file_path).length;
  edit("New A");
  assert.ok(text(contextRows()[0]).includes("New A"));
  await approve();
  await settle();
  assert.equal(h.reads.filter(filter => filter.file_path).length, reads);
  assert.ok(text(contextRows()[0]).includes("New A"));
});

test("live placeholder hints cover common engine controls and respect escaped delimiters", async () => {
  const tokens = ["{name}", "{/b}", "{color=#fff}", "<b>", "</b>", "%s", "%(name)s", "%1", "\\C[2]", "\\n"];
  await startWith([{ ...row("A"), source: `${tokens.join(" ")} {{literal} [[literal] {P}`, translation: tokens.join(" ") }]);
  assert.deepEqual(nodes(render()).filter(node => node.props?.["data-placeholder"]).map(text), tokens);
  assert.equal(text(nodes(render()).find(node => node.props?.role === "status")), "Placeholders: 10/10 present");
});

test("RPG Maker plain brackets are body text while engine controls are placeholder chips", async () => {
  h.format = "rpgmaker-mv";
  await startWith([{ ...row("A"), source: "[plain] \\V[1]", translation: "[texto] \\V[1]" }]);
  assert.deepEqual(nodes(render()).filter(node => node.props?.["data-placeholder"]).map(text), ["\\V[1]"]);
  assert.equal(text(nodes(render()).find(node => node.props?.role === "status")), "Placeholders: 1/1 present");
});

for (const locator of [
  (n: number) => `script.rpy#${n}`,
  (n: number) => `Map001.json#0#event_1#page_0#cmd_${n}#msg`,
  (n: number) => `Map001.json#0#event_${n}#page_0#cmd_0#msg`,
  (n: number) => `Map001.json#0#event_1#page_${n}#cmd_0#msg`,
]) {
  test(`context orders numeric locators across 9/10 and API pages: ${locator(8)}`, async () => {
    const entries = Array.from({ length: 120 }, (_, i) => ({
      ...row(locator(i + 1)), status: i === 7 ? "translated" : "pending",
    } as StringEntry)).sort((a, b) => a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
    await startWith(entries);
    assert.deepEqual(contextRows().map(node => node.props["data-entry-id"]),
      [5, 6, 7, 8, 9, 10, 11].map(locator));
    assert.ok(h.reads.some(filter => filter.file_path && filter.offset === 100));
  });
}

for (const [format, source, translated, expected] of [
  ["rpgmaker-mv", String.raw`Gold \g`, "Oro", String.raw`Placeholders: missing \g`],
  ["renpy", "Hello [party[0].name]", "Hola [party[1].name]", "Placeholders: missing [party[0].name]; extra [party[1].name]"],
]) {
  test(`live validation detects removed or changed controls: ${source}`, async () => {
    h.format = format;
    await startWith([{ ...row("A"), source, translation: source }]);
    edit(translated);
    const check = nodes(render()).find(node => node.props?.role === "status");
    assert.equal(text(check), expected);
    assert.ok(!check.props.className.includes("text-success"));
  });
}

for (const escape of [String.raw`\n`, String.raw`\nc`, String.raw`\nr`, String.raw`\N`, String.raw`\NC`, String.raw`\NR`]) {
  test(`name-box arguments are translatable while ${escape} remains protected`, async () => {
    h.format = "rpgmaker-mv";
    await startWith([{ ...row("A"), source: `${escape}<Guard> Hello`, translation: `${escape}<Guardia> Hola` }]);
    assert.deepEqual(nodes(render()).filter(node => node.props?.["data-placeholder"]).map(text), [escape]);
    assert.equal(text(nodes(render()).find(node => node.props?.role === "status")), "Placeholders: 1/1 present");
    edit("<Guardia> Hola");
    assert.equal(text(nodes(render()).find(node => node.props?.role === "status")), `Placeholders: missing ${escape}; extra <Guardia>`);
  });
}

for (const filePath of [String.raw`C:\Users\Mike\StarHarbor\game\options.rpy`, "/games/StarHarbor/game/options.rpy", "options.rpy"]) {
  test(`card header uses the Editor filename with full path tooltip: ${filePath}`, async () => {
    await startWith([{ ...row("A"), file_path: filePath }]);
    const label = nodes(render()).find(node => node.props?.title === filePath);
    assert.ok(label);
    assert.equal(text(label), "options.rpy");
    assert.ok(label.props.className.includes("truncate"));
  });
}

test("context never crosses archive member boundaries in the same RPA file", async () => {
  const entries = ["chapter1.rpyc", "chapter2.rpyc"].flatMap(member =>
    Array.from({ length: 5 }, (_, n) => ({
      ...row(`scripts.rpa#${member}#s${n}`), file_path: "scripts.rpa",
      status: member === "chapter2.rpyc" && n === 0 ? "translated" : "pending",
    } as StringEntry)));
  await startWith(entries);
  assert.deepEqual(contextRows().map(node => node.props["data-entry-id"]),
    [0, 1, 2, 3].map(n => `scripts.rpa#chapter2.rpyc#s${n}`));
  assert.equal(contextRows()[0].props["aria-current"], true);
});
