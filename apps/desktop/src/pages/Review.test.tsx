import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { ApiError } from "../lib/apiError.ts";
import type { FileStats, StringEntry } from "../lib/api.ts";
import * as contextLibrary from "../lib/reviewContext.ts";
import { draftProjectKey, projectScopedQueryFn } from "../lib/projectScope.ts";
import { useProjectStore } from "../stores/projectStore.ts";
import { draftEntryKey, editDraft, saveDraft, useDraftStore } from "../stores/draftStore.ts";
import { setLocale, t } from "../lib/i18n/index.ts";

const project = { path: "/game", database_path: "/game.db", format_id: "renpy", name: "Game" };
const projectKey = draftProjectKey(project);
const client = new QueryClient({ defaultOptions: { queries: {
  retry: false, staleTime: 30_000, gcTime: Infinity,
} } });

type Patch = Partial<Pick<StringEntry, "translation" | "status">> & { expected_translation?: string | null };
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
  saved: [] as StringEntry[],
  failure: null as unknown, waitForSave: null as Promise<void> | null, invalidations: 0,
  keydown: null as ((event: any) => void) | null, navigations: [] as string[],
  t, client, projectStore: useProjectStore,
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
  get format() { return useProjectStore.getState().project?.format_id ?? ""; },
  set format(format_id: string) { useProjectStore.getState().setProject({ ...project, format_id }); },
  contextFailure: false, contextWaits: {} as Record<string, Promise<void> | undefined>,
  contextBuilds: [] as string[],
  reviewContext: contextLibrary.reviewContext,
  buildContextIndex(rows: StringEntry[]) {
    this.contextBuilds.push(rows[0]?.file_path ?? "");
    return contextLibrary.buildReviewContextIndex(rows);
  },
  focused: 0,
  storage: new Map<string, string>(), storageFailure: false,
  fileStatsFailure: false,
  fetchWaits: {} as Record<string, Promise<void> | undefined>,
  async getFileStats(): Promise<FileStats[]> {
    if (this.fileStatsFailure) throw new Error("File stats unavailable");
    const files = new Map<string, FileStats>();
    for (const entry of Object.values(this.rows)) {
      const stats = files.get(entry.file_path) ?? { file_path: entry.file_path, total: 0,
        pending: 0, translated: 0, reviewed: 0, approved: 0, error: 0 };
      stats.total++;
      stats[entry.status]++;
      files.set(entry.file_path, stats);
    }
    return [...files.values()];
  },
  reads: [] as { status?: string; file_path?: string; limit?: number; offset?: number }[],
  async getStrings(filter: { status?: string; file_path?: string; limit?: number; offset?: number }) {
    this.reads.push(filter);
    if (filter.file_path && !filter.status) {
      if (this.contextWaits[filter.file_path]) await this.contextWaits[filter.file_path];
      if (this.contextFailure) throw new Error("Context unavailable");
    }
    const entries = Object.values(this.rows).filter(entry =>
      (!filter.status || entry.status === filter.status) && (!filter.file_path || entry.file_path === filter.file_path));
    const offset = filter.offset ?? 0;
    const result = { entries: entries.slice(offset, offset + (filter.limit ?? entries.length)), total: entries.length,
      offset, limit: filter.limit ?? entries.length };
    const wait = this.fetchWaits[JSON.stringify([filter.file_path ?? "", filter.status, offset])];
    if (wait) await wait;
    return result;
  },
  async patchString(id: string, patch: Patch) {
    this.writes.push({ id, patch });
    const rows = this.rows;
    if (this.waitForSave) await this.waitForSave;
    if (this.failure !== null) throw this.failure;
    if ("expected_translation" in patch && patch.expected_translation !== rows[id].translation) {
      throw new ApiError("409: translation changed since it was loaded");
    }
    rows[id] = { ...rows[id], ...patch };
    this.saved.push(rows[id]);
    return rows[id];
  },
};
(globalThis as any).__reviewTest = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/pages/Review.tsx")) {
      const stubs: Record<string, string> = {
        react: `const h=globalThis.__reviewTest;export const useState=x=>h.state(x);export const useRef=x=>h.ref(x);export const useEffect=(fn,deps)=>h.effect(fn,deps);export const useCallback=(fn,deps)=>h.callback(fn,deps);`,
        "react-router-dom": `export const useNavigate=()=>path=>globalThis.__reviewTest.navigations.push(path);`,
        "@tanstack/react-query": `export const useQueryClient=()=>globalThis.__reviewTest.client;`,
        "../stores/projectStore": `const store=globalThis.__reviewTest.projectStore;export const useProjectStore=Object.assign(select=>select(store.getState()),{getState:store.getState});`,
        "../lib/reviewContext": `const h=globalThis.__reviewTest;export const buildReviewContextIndex=rows=>h.buildContextIndex(rows);export const reviewContext=(...args)=>h.reviewContext(...args);`,
        "../lib/i18n": `export const useT=()=>globalThis.__reviewTest.t;`,
        "../lib/api": `const h=globalThis.__reviewTest;export const getFileStats=()=>h.getFileStats();export const getStrings=filter=>h.getStrings(filter);export const patchString=(id,patch)=>h.patchString(id,patch);export const getSpeakers=async()=>({groups:h.speakers,unassigned_entry_ids:[]});`,
      };
      if (stubs[specifier]) return { url: `data:text/javascript,${encodeURIComponent(stubs[specifier])}`, shortCircuit: true };
    }
    return nextResolve(specifier, context);
  },
});
const previousStorage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem(key: string) { if (h.storageFailure) throw new Error("Storage denied"); return h.storage.get(key) ?? null; },
  setItem(key: string, value: string) { if (h.storageFailure) throw new Error("Storage denied"); h.storage.set(key, value); },
  removeItem(key: string) { if (h.storageFailure) throw new Error("Storage denied"); h.storage.delete(key); },
} });
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
// Count calls without replacing QueryClient's real invalidation/cache behavior.
const invalidateQueries = client.invalidateQueries.bind(client);
client.invalidateQueries = (...args: Parameters<typeof client.invalidateQueries>) => {
  h.invalidations++;
  return invalidateQueries(...args);
};
after(() => {
  client.clear();
  useProjectStore.getState().clearProject();
  hooks.deregister();
  if (previousStorage) Object.defineProperty(globalThis, "localStorage", previousStorage);
  else Reflect.deleteProperty(globalThis, "localStorage");
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
  client.clear();
  useDraftStore.setState({ drafts: {}, records: [], persistenceIssue: null, persistenceIssues: {} });
  useProjectStore.getState().setProject(project);
  setLocale("en");
  h.values = []; h.effects = []; h.pending = []; h.errors = [];
  h.rows = { A: row("A"), B: row("B") }; h.writes = []; h.saved = [];
  h.failure = null; h.waitForSave = null; h.invalidations = 0;
  h.keydown = null; h.navigations = []; h.speakers = []; h.reads = [];
  h.format = "renpy"; h.contextFailure = false; h.contextWaits = {}; h.focused = 0;
  h.storage.clear(); h.storageFailure = false; h.fetchWaits = {};
  h.fileStatsFailure = false; h.contextBuilds = [];
  render();
  await settle();
  assert.equal(textarea().props.value, "Translation A");
});

// Match Editor's public query keys, fetch gate and 30-second freshness window.
function editorQueries(status?: StringEntry["status"]) {
  const filter = { ...(status ? { status } : {}), limit: 50, offset: 0 };
  return {
    list: { queryKey: ["strings", filter, projectKey],
      queryFn: projectScopedQueryFn(projectKey, () => h.getStrings(filter)) },
    detail: { queryKey: ["string", "A", projectKey],
      queryFn: projectScopedQueryFn(projectKey, async () => h.rows.A) },
  };
}

for (const status of [undefined, "translated", "approved", "reviewed"] as const) {
  test(`approval then Editor navigation refreshes the ${status ?? "all"} table and selected detail`, async () => {
    if (status === "reviewed") {
      await startWith([{ ...row("A"), status: "reviewed" }, { ...row("B"), status: "pending" }]);
    }
    const queries = editorQueries(status);
    await client.fetchQuery(queries.list);
    await client.fetchQuery(queries.detail);
    const statsKey = ["stats", projectKey];
    client.setQueryData(statsKey, { approved: 0 });
    if (!status) edit("Approved revision");
    await approve();
    assert.equal(h.saved[h.saved.length - 1]?.status, "approved", "the public save API returned approved");
    button(t(status === "reviewed" ? "review.returnToEditor" : "common.exit")).props.onClick();
    assert.deepEqual(h.navigations, ["/editor"]);
    const table = new QueryObserver(client, queries.list);
    const detail = new QueryObserver(client, queries.detail);
    const unsubscribe = [table.subscribe(() => {}), detail.subscribe(() => {})];
    try {
      await settle();
      const rows = table.getCurrentResult().data!.entries;
      assert.deepEqual(rows.map(entry => [entry.id, entry.status]),
        status === "translated" ? [["B", "translated"]] :
        status === "reviewed" ? [] :
        status === "approved" ? [["A", "approved"]] : [["A", "approved"], ["B", "translated"]]);
      assert.equal(detail.getCurrentResult().data!.status, "approved");
      assert.equal(detail.getCurrentResult().data!.translation, status ? "Translation A" : "Approved revision");
      if (!status) assert.equal(rows[0].translation, "Approved revision");
      assert.equal(client.getQueryState(statsKey)?.isInvalidated, true);
    } finally { unsubscribe.forEach(stop => stop()); }
  });
}

test("approval invalidates only this database's caches and leaves newer Editor drafts intact", async () => {
  const own = [["strings", {}, projectKey], ["string", "A", projectKey], ["stats", projectKey]];
  const others = [
    { ...project, database_path: "/pivot.db" }, { ...project, path: "/other" },
    { ...project, format_id: "rpgmaker-mv" },
  ].flatMap(other => {
    const key = draftProjectKey(other);
    return [["strings", {}, key], ["string", "A", key], ["stats", key]];
  });
  const unrelated = [["config"], ["memory-suggestions", projectKey, "A"]];
  for (const key of [...own, ...others, ...unrelated]) client.setQueryData(key, "cached");
  const draftKey = draftEntryKey(projectKey, "A");
  editDraft(draftKey, "Submitted Editor draft", "Translation A");
  let finish!: () => void;
  const saving = saveDraft(draftKey, "Translation A", () => new Promise<void>(resolve => { finish = resolve; }));
  await Promise.resolve();
  editDraft(draftKey, "Newer Editor draft");
  const drafts = useDraftStore.getState().drafts;
  assert.equal(drafts[draftKey].saving, true);
  const reads = h.reads.length;
  try {
    await approve();
    for (const key of own) assert.equal(client.getQueryState(key)?.isInvalidated, true, JSON.stringify(key));
    for (const key of [...others, ...unrelated]) {
      assert.equal(client.getQueryState(key)?.isInvalidated, false, JSON.stringify(key));
      assert.equal(client.getQueryData(key), "cached");
    }
    assert.equal(h.reads.length, reads, "invalidation defers API reads until Editor navigation");
    assert.equal(useDraftStore.getState().drafts, drafts, "approval cannot acknowledge a newer pending Editor draft");
  } finally { finish(); await saving; }
  assert.equal(useDraftStore.getState().drafts[draftKey].text, "Newer Editor draft");
});

for (const action of ["skip", "refused", "pending"] as const) {
  test(`${action} approval does not invalidate or replace Editor cache data`, async () => {
    const queries = editorQueries("translated");
    const cached = await client.fetchQuery(queries.list);
    client.setQueryData(["stats", projectKey], { approved: 0 });
    let finish = () => {};
    if (action === "skip") button(t("review.skip")).props.onClick();
    else {
      if (action === "refused") h.failure = new Error("Approval refused");
      else h.waitForSave = new Promise<void>(resolve => { finish = resolve; });
      button(t("review.approve")).props.onClick();
      if (action === "refused") await settle();
    }
    try {
      assert.equal(client.getQueryData(queries.list.queryKey), cached);
      assert.equal(client.getQueryState(queries.list.queryKey)?.isInvalidated, false);
      assert.equal(client.getQueryState(["stats", projectKey])?.isInvalidated, false);
      assert.equal(h.invalidations, 0);
      assert.deepEqual(h.saved, []);
    } finally { finish(); await settle(); }
  });
}

test("no project shows the empty state and refuses a previously captured approval handler", async () => {
  const click = button(t("review.approve")).props.onClick;
  useProjectStore.getState().clearProject();
  click(); // The store can change before React has rendered the new project.
  await settle();
  assert.equal(textarea(), undefined);
  assert.deepEqual(h.writes, []);
  assert.equal(h.invalidations, 0);
});

for (const replacement of [
  { ...project, database_path: "/pivot.db" }, { ...project, path: "/other" },
  { ...project, format_id: "rpgmaker-mv" },
]) {
  test(`a pending approval cannot advance or invalidate the replacement project: ${draftProjectKey(replacement)}`, async () => {
    const oldRows = h.rows;
    const nextKey = draftProjectKey(replacement);
    const oldCache = ["strings", {}, projectKey];
    const nextCache = ["strings", {}, nextKey];
    client.setQueryData(oldCache, "old cached");
    client.setQueryData(nextCache, "next cached");
    let finish!: () => void;
    h.waitForSave = new Promise<void>(resolve => { finish = resolve; });
    button(t("review.approve")).props.onClick();
    useProjectStore.getState().setProject(replacement);
    h.rows = { A: { ...row("A"), translation: "Replacement A" }, B: row("B") };
    render();
    finish();
    await settle();
    assert.equal(oldRows.A.status, "approved");
    assert.equal(h.saved[h.saved.length - 1]?.status, "approved");
    assert.equal(textarea().props.value, "Replacement A");
    progress(1, 0);
    assert.equal(client.getQueryState(oldCache)?.isInvalidated, true);
    assert.equal(client.getQueryState(nextCache)?.isInvalidated, false);
    assert.equal(client.getQueryData(nextCache), "next cached");
  });
}

test("the approval decision preserves approved status and distinguishes cleared text from no change", () => {
  assert.deepEqual(reviewApprovalPatch(row("A"), "Translation A"), { status: "approved", expected_translation: "Translation A" });
  assert.deepEqual(reviewApprovalPatch({ ...row("A"), status: "reviewed" }, "Translation A"), { status: "approved", expected_translation: "Translation A" });
  const approved = { ...row("A"), status: "approved" as const };
  assert.equal(reviewApprovalPatch(approved, "Translation A"), null);
  assert.deepEqual(reviewApprovalPatch(approved, "Revised A"), { translation: "Revised A", status: "approved", expected_translation: "Translation A" });
  assert.deepEqual(reviewApprovalPatch(approved, ""), { translation: "", status: "approved", expected_translation: "Translation A" });
  assert.equal(reviewApprovalPatch({ ...approved, translation: null }, ""), null);
});

for (const locale of ["en", "es"] as const) {
  for (const edited of [false, true]) {
    test(`${locale}: ${edited ? "edited" : "unchanged"} approval guards the shown baseline and retains the draft on conflict`, async () => {
      setLocale(locale);
      const draft = edited ? "V-review" : "Translation A";
      if (edited) edit(draft);
      h.rows.A = { ...h.rows.A, translation: "V2", status: "reviewed" };
      const before = { ...h.rows.A };
      const invalidations = h.invalidations;
      await approve();
      assert.deepEqual(h.writes, [{ id: "A", patch: {
        ...(edited ? { translation: draft } : {}), status: "approved", expected_translation: "Translation A",
      } }]);
      assert.deepEqual(h.rows.A, before);
      assert.equal(textarea().props.value, draft);
      progress(1, 0);
      assert.ok(nodes(render()).some(node => node.props.role === "alert" && text(node) === t("api.error.translationConflict")));
      assert.equal(h.invalidations, invalidations, "conflicts must not invalidate Editor data");
    });
  }
}

test("unchanged approval sends only status and baseline and preserves provider attribution", async () => {
  await startWith([{ ...row("A"), provider_used: "original-provider" }, row("B")]);
  await approve();
  assert.deepEqual(h.writes[0].patch, { status: "approved", expected_translation: "Translation A" });
  assert.equal(h.rows.A.provider_used, "original-provider");
});

for (const baseline of [null, ""] as const) {
  test(`approval preserves the nullable loaded baseline: ${JSON.stringify(baseline)}`, async () => {
    await startWith([{ ...row("A"), translation: baseline }, row("B")]);
    await approve();
    assert.deepEqual(h.writes[0].patch, { status: "approved", expected_translation: baseline });
  });
}

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
  h.contextBuilds = [];
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


function scopeSelector() {
  const select = nodes(render()).find(node => node.type === "select");
  assert.ok(select, "file scope selector is available");
  assert.ok(select.props["aria-label"] || nodes(render()).some(node =>
    node.type === "label" && node.props.htmlFor === select.props.id), "scope has an accessible label");
  return select;
}
function selectScope(filePath: string) { scopeSelector().props.onChange({ target: { value: filePath } }); render(); }
function fileRows(translated = 55) {
  return ["A", "B"].flatMap(file => Array.from({ length: 60 }, (_, i) => ({
    ...row(`${file}${i}`), file_path: `${file}.json`,
    status: i < translated ? "translated" : "reviewed",
  } as StringEntry)));
}
async function flushScope() {
  // Keep rendering the new scope while an old scope's request is unresolved.
  for (let i = 0; i < 24; i++) { await Promise.resolve(); render(); }
}

for (const translated of [55, 5]) {
  test(`file A visits exactly its 60 rows across approval-driven pagination (${translated} translated)`, async () => {
    await startWith([...fileRows(translated), { ...row("pending"), file_path: "pending.json", status: "pending" },
      { ...row("approved"), file_path: "approved.json", status: "approved" },
      { ...row("A-pending"), file_path: "A.json", status: "pending" },
      { ...row("A-approved"), file_path: "A.json", status: "approved" }]);
    const options = nodes(scopeSelector()).filter(node => node.type === "option");
    assert.deepEqual(options.map(node => node.props.value), ["", "A.json", "B.json"]);
    assert.deepEqual(options.slice(1).map(node => node.props.children.join("")), ["A.json (60)", "B.json (60)"],
      "counts include only translated + reviewed");
    h.reads = [];
    selectScope("A.json");
    await settle();
    progress(1, 0, 60);
    for (let i = 0; i < 60; i++) {
      assert.equal(textarea().props.value, `Translation A${i}`);
      await approve();
    }
    assert.deepEqual(h.writes.map(write => write.id), Array.from({ length: 60 }, (_, i) => `A${i}`));
    assert.ok(h.writes.every(write => write.patch.expected_translation === `Translation ${write.id}`));
    assert.ok(Object.values(h.rows).filter(entry => entry.file_path === "B.json").every(entry => entry.status !== "approved"));
    assert.ok(text(render()).includes(t("review.complete")));
    const queueReads = h.reads.filter(filter => filter.status);
    assert.ok(queueReads.every(filter => filter.file_path === "A.json"), "bootstrap and subsequent pages stay scoped");
    assert.ok(queueReads.some(filter => filter.offset! > 0), "approvals drive offset-adjusted pagination");
    assert.ok(queueReads.some(filter => filter.status === "translated"));
    assert.ok(queueReads.some(filter => filter.status === "reviewed"));
    selectScope("B.json");
    await settle();
    progress(1, 0, 60);
    assert.equal(textarea().props.value, "Translation B0");
  });
}

test("All files keeps the original 120-row walk and unscoped queue requests", async () => {
  await startWith(fileRows());
  assert.equal(scopeSelector().props.value, "");
  progress(1, 0, 120);
  for (let i = 0; i < 120; i++) await approve();
  assert.equal(new Set(h.writes.map(write => write.id)).size, 120);
  assert.ok(h.reads.filter(filter => filter.status).every(filter => !filter.file_path));
  assert.ok(text(render()).includes(t("review.complete")));
});

for (const rejected of [false, true]) {
  test(`scope switch ignores a delayed bootstrap ${rejected ? "failure" : "response"}`, async () => {
    await startWith(fileRows());
    let finish!: () => void;
    let fail!: (error: Error) => void;
    h.fetchWaits[JSON.stringify(["A.json", "translated", 0])] = new Promise<void>((resolve, reject) => { finish = resolve; fail = reject; });
    selectScope("A.json");
    assert.ok(text(render()).includes(t("review.loading")));
    selectScope("B.json");
    await flushScope();
    assert.equal(textarea().props.value, "Translation B0");
    progress(1, 0, 60);
    if (rejected) fail(new Error("Old scope failed")); else finish();
    await settle();
    assert.equal(textarea().props.value, "Translation B0");
    assert.ok(!text(render()).includes("Old scope failed"));
    await approve();
    assert.deepEqual(h.writes.map(write => write.id), ["B0"]);
    progress(2, 1, 60);
  });
}

test("switching during a delayed page cannot merge the previous file or retain its cursor and draft", async () => {
  await startWith(fileRows());
  selectScope("A.json");
  await settle();
  let finish!: () => void;
  h.fetchWaits[JSON.stringify(["A.json", "translated", 9])] = new Promise<void>(resolve => { finish = resolve; });
  for (let i = 0; i < 40; i++) await approve();
  button(t("review.approve")).props.onClick();
  await flushScope();
  button(t("review.skip")).props.onClick();
  render();
  assert.ok(h.reads.some(filter => filter.status === "translated" && filter.file_path === "A.json" && filter.offset === 9));
  edit("Discard this scope's draft");
  button(t("review.showDiff")).props.onClick();
  selectScope("B.json");
  await flushScope();
  assert.equal(textarea().props.value, "Translation B0");
  progress(1, 0, 60);
  assert.ok(button(t("review.showDiff")));
  finish();
  await settle();
  assert.equal(textarea().props.value, "Translation B0");
  for (let i = 0; i < 60; i++) await approve();
  assert.deepEqual(h.writes.slice(41).map(write => write.id), Array.from({ length: 60 }, (_, i) => `B${i}`));
});

test("scope changes synchronously refuse a captured approval handler and pending saves lock the selector", async () => {
  await startWith(fileRows());
  const staleApprove = button(t("review.approve")).props.onClick;
  scopeSelector().props.onChange({ target: { value: "B.json" } });
  staleApprove();
  await settle();
  assert.deepEqual(h.writes, []);
  let finish!: () => void;
  h.waitForSave = new Promise<void>(resolve => { finish = resolve; });
  button(t("review.approve")).props.onClick();
  assert.equal(scopeSelector().props.disabled, true);
  scopeSelector().props.onChange({ target: { value: "A.json" } });
  assert.equal(scopeSelector().props.value, "B.json");
  finish();
  await settle();
  progress(2, 1, 60);
});

test("last scope is remembered per project using the existing project preference pattern", async () => {
  const entries = fileRows();
  await startWith(entries);
  selectScope("A.json");
  await settle();
  await startWith(entries);
  assert.equal(scopeSelector().props.value, "A.json");
  progress(1, 0, 60);
  useProjectStore.getState().setProject({ ...project, database_path: "/other.db" });
  render();
  await settle();
  assert.equal(scopeSelector().props.value, "");
  progress(1, 0, 120);
  selectScope("B.json");
  await settle();
  useProjectStore.getState().setProject(project);
  render();
  await settle();
  assert.equal(scopeSelector().props.value, "A.json");
  progress(1, 0, 60);
  selectScope("");
  await settle();
  await startWith(entries);
  assert.equal(scopeSelector().props.value, "");
});

test("a remembered file with no reviewable rows falls back to All files", async () => {
  await startWith(fileRows());
  selectScope("A.json");
  await settle();
  await startWith([{ ...row("done"), file_path: "A.json", status: "approved" }, { ...row("next"), file_path: "B.json" }]);
  assert.equal(scopeSelector().props.value, "");
  assert.equal(textarea().props.value, "Translation next");
  progress(1, 0, 1);
});

test("file scope works when browser storage is unavailable", async () => {
  h.storageFailure = true;
  await startWith(fileRows());
  selectScope("B.json");
  await settle();
  assert.equal(textarea().props.value, "Translation B0");
  progress(1, 0, 60);
});

test("file stats failure keeps All files review usable and exposes a retry", async () => {
  h.fileStatsFailure = true;
  await startWith(fileRows());
  progress(1, 0, 120);
  assert.equal(scopeSelector().props.value, "");
  assert.ok(nodes(render()).some(node => node.props?.role === "alert"));
  h.fileStatsFailure = false;
  button(t("common.retry")).props.onClick();
  await settle();
  assert.deepEqual(nodes(scopeSelector()).filter(node => node.type === "option").map(node => node.props.value), ["", "A.json", "B.json"]);
});

test("many selections and approvals in one file build its complete context index once", async () => {
  h.reads = [];
  await startWith(Array.from({ length: 80 }, (_, n) => row(`r${n}`)));
  assert.deepEqual(h.contextBuilds, ["dialogue.json"]);
  for (let n = 0; n < 60; n++) {
    if (n % 3 === 0) await approve();
    else { button(t("review.skip")).props.onClick(); await settle(); }
  }
  for (let n = 0; n < 10; n++) {
    button(t("review.previous")).props.onClick();
    await settle();
  }
  assert.deepEqual(h.contextBuilds, ["dialogue.json"]);
  assert.equal(h.reads.filter(filter => filter.file_path === "dialogue.json" && !filter.status).length, 2,
    "the index includes both context pages");
  assert.deepEqual(contextRows().map(node => node.props["data-entry-id"]),
    [47, 48, 49, 50, 51, 52, 53].map(n => `r${n}`));
});

test("switching file scope clears completed context indexes before returning to the same file", async () => {
  await startWith(fileRows());
  assert.deepEqual(h.contextBuilds, ["A.json"]);
  selectScope("B.json");
  await settle();
  selectScope("A.json");
  await settle();
  selectScope("");
  await settle();
  assert.deepEqual(h.contextBuilds, ["A.json", "B.json", "A.json", "A.json"]);
});

for (const replacement of [
  { ...project, database_path: "/context.db" }, { ...project, path: "/context-game" },
  { ...project, format_id: "rpgmaker-mv" },
]) {
  test(`project change clears the context index: ${draftProjectKey(replacement)}`, async () => {
    assert.deepEqual(h.contextBuilds, ["dialogue.json"]);
    h.rows = { A: row("A"), B: { ...row("B"), translation: "Replacement neighbor", status: "pending" } };
    useProjectStore.getState().setProject(replacement);
    render();
    await settle();
    assert.deepEqual(h.contextBuilds, ["dialogue.json", "dialogue.json"]);
    assert.ok(text(contextRows()[1]).includes("Replacement neighbor"));
  });
}

test("context index LRU keeps four files and refreshes a file when revisited", async () => {
  const files = ["A", "B", "C", "D", "A", "E", "B"];
  await startWith(files.map((file, n) => ({ ...row(`${file}.rpy#s${n}`), file_path: `${file}.rpy` })));
  for (let n = 1; n < files.length; n++) {
    button(t("review.skip")).props.onClick();
    await settle();
  }
  assert.deepEqual(h.contextBuilds, ["A.rpy", "B.rpy", "C.rpy", "D.rpy", "E.rpy", "B.rpy"]);
});

test("cached context keeps saved neighbor text and the selected live draft", async () => {
  edit("Saved neighbor");
  await approve();
  assert.ok(text(contextRows()[0]).includes("Saved neighbor"));
  edit("Current draft");
  assert.ok(text(contextRows()[1]).includes("Current draft"));
  assert.deepEqual(h.contextBuilds, ["dialogue.json"]);
});

test("navigation while context pages are pending builds only the completed file index", async () => {
  let finish!: () => void;
  h.contextWaits["delayed.rpy"] = new Promise<void>(resolve => { finish = resolve; });
  await startWith(Array.from({ length: 60 }, (_, n) => ({
    ...row(`delayed.rpy#s${n}`), file_path: "delayed.rpy",
  })));
  for (let n = 0; n < 5; n++) {
    button(t("review.skip")).props.onClick();
    render();
  }
  assert.deepEqual(h.contextBuilds, []);
  finish();
  await flushScope();
  assert.deepEqual(h.contextBuilds, ["delayed.rpy"]);
  assert.deepEqual(contextRows().map(node => node.props["data-entry-id"]),
    [2, 3, 4, 5, 6, 7, 8].map(n => `delayed.rpy#s${n}`));
});
