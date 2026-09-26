import { create } from "zustand";
import type { ProjectInfo } from "../lib/api";
import { createDraftPersistence, DRAFT_PREFIX, type DraftStorageIssue, type DurableDraft } from "../lib/draftPersistence";

export function draftProjectKey(project: ProjectInfo): string {
  return JSON.stringify([project.database_path ?? null, project.path, project.format_id]);
}
export function draftEntryKey(projectKey: string, id: string): string {
  return JSON.stringify([projectKey, id]);
}
export interface EntryDraft {
  text: string;
  error: string | null;
  saving: boolean;
  revision?: string;
}
const persistence = createDraftPersistence();
const loaded = persistence.read();
function branches(records: DurableDraft[]): DurableDraft[] {
  const parents = new Set(records.map(r => JSON.stringify([r.entryKey, r.parent])));
  return records.filter(r => !parents.has(JSON.stringify([r.entryKey, r.revision]))).sort((a, b) => b.updatedAt - a.updatedAt || b.revision.localeCompare(a.revision));
}
const initialDrafts: Record<string, EntryDraft> = {};
for (const record of branches(loaded.records)) {
  initialDrafts[record.entryKey] ??= { text: record.text, error: record.error, saving: false, revision: record.revision };
}
export const useDraftStore = create<{
  drafts: Record<string, EntryDraft>;
  records: DurableDraft[];
  persistenceIssue: DraftStorageIssue | null;
  persistenceIssues: Record<string, DraftStorageIssue>;
}>(() => ({ drafts: initialDrafts, records: loaded.records, persistenceIssue: loaded.issue, persistenceIssues: {} }));
const pending = new Map<string, Promise<boolean>>();

function persistDraft(key: string): void {
  const draft = useDraftStore.getState().drafts[key];
  if (!draft) return;
  const result = persistence.write(key, draft.text, draft.error, draft.revision ?? null);
  const fresh = persistence.read();
  useDraftStore.setState(({ drafts, persistenceIssues }) => {
    const issues = { ...persistenceIssues };
    if (result.issue) issues[key] = result.issue;
    else delete issues[key];
    return {
      drafts: { ...drafts, [key]: { ...drafts[key], revision: result.record?.revision ?? draft.revision } },
      records: fresh.records, persistenceIssue: fresh.issue, persistenceIssues: issues,
    };
  });
}
export function refreshDurableDrafts(): void {
  const fresh = persistence.read();
  useDraftStore.setState(({ drafts }) => {
    const next = { ...drafts };
    for (const record of branches(fresh.records)) {
      // Never replace the active text or an in-flight write from another tab.
      next[record.entryKey] ??= { text: record.text, error: record.error, saving: false, revision: record.revision };
    }
    return { drafts: next, records: fresh.records, persistenceIssue: fresh.issue ?? useDraftStore.getState().persistenceIssue };
  });
}
if (typeof window !== "undefined") {
  window.addEventListener("storage", event => {
    if (event.key === null || event.key.startsWith(DRAFT_PREFIX)) refreshDurableDrafts();
  });
}
export function draftAlternatives(key: string, records: DurableDraft[], current?: EntryDraft): DurableDraft[] {
  return branches(records.filter(r => r.entryKey === key)).filter(r => r.revision !== current?.revision && r.text !== current?.text);
}
export function selectDraftAlternative(key: string, revision: string): void {
  if (pending.has(key)) return;
  const state = useDraftStore.getState();
  const current = state.drafts[key];
  if (current && !state.records.some(r => r.entryKey === key && r.text === current.text)) {
    persistDraft(key);
    if (!useDraftStore.getState().records.some(r => r.entryKey === key && r.text === current.text)) return;
  }
  const record = useDraftStore.getState().records.find(r => r.entryKey === key && r.revision === revision);
  if (!record) return;
  // Keep the previously selected branch available. Selecting is not deletion.
  useDraftStore.setState(({ drafts }) => ({ drafts: { ...drafts,
    [key]: { text: record.text, error: record.error, saving: false, revision: record.revision },
  } }));
}
export function editDraft(key: string, text: string): void {
  useDraftStore.setState(({ drafts }) => ({ drafts: {
    ...drafts, [key]: { ...drafts[key], text, error: null, saving: drafts[key]?.saving ?? false },
  } }));
  // Synchronous persistence covers abrupt reload/exit without an unload handler.
  persistDraft(key);
}
export function acknowledgeDraft(key: string, serverText: string): void {
  const draft = useDraftStore.getState().drafts[key];
  if (!draft || draft.saving || draft.text !== serverText) return;
  const fresh = persistence.read();
  let issue = fresh.issue;
  for (const record of fresh.records) {
    if (record.entryKey === key && (record.revision === draft.revision || record.text === serverText)) {
      issue = persistence.remove(record) ?? issue;
    }
  }
  const remaining = persistence.read();
  // Failed cleanup must not resurrect an old persisted draft without a warning.
  useDraftStore.setState(({ drafts }) => {
    const next = { ...drafts };
    delete next[key];
    const alternative = branches(remaining.records.filter(r => r.entryKey === key))
      .find(r => r.revision !== draft.revision && r.text !== serverText);
    if (alternative && alternative.text !== serverText) next[key] = { text: alternative.text, error: alternative.error, saving: false, revision: alternative.revision };
    const issues = { ...useDraftStore.getState().persistenceIssues };
    if (issue) issues[key] = issue;
    else delete issues[key];
    return { drafts: next, records: remaining.records, persistenceIssue: remaining.issue, persistenceIssues: issues };
  });
}
export function saveDraft(
  key: string,
  serverText: string,
  write: (text: string) => Promise<unknown>,
): Promise<boolean> {
  const active = pending.get(key);
  if (active) return active;
  const draft = useDraftStore.getState().drafts[key];
  if (!draft || draft.text === serverText) {
    acknowledgeDraft(key, serverText);
    return Promise.resolve(true);
  }
  const submitted = draft.text;
  useDraftStore.setState(({ drafts }) => ({ drafts: {
    ...drafts, [key]: { ...draft, error: null, saving: true },
  } }));
  const operation = Promise.resolve().then(() => write(submitted)).then(() => true, (error: unknown) => {
    useDraftStore.setState(({ drafts }) => ({ drafts: {
      ...drafts, [key]: { ...drafts[key], error: error instanceof Error ? error.message : String(error) },
    } }));
    return false;
  }).finally(() => {
    pending.delete(key);
    useDraftStore.setState(({ drafts }) => ({ drafts: {
      ...drafts, [key]: { ...drafts[key], saving: false },
    } }));
    persistDraft(key);
  });
  pending.set(key, operation);
  return operation;
}
