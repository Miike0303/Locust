import { useEffect, useRef, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { getSpeakers, saveSpeakerNote, type ProjectInfo } from "../lib/api";
import { speakerOwner, type SpeakerGroup, type SpeakerGroups } from "../lib/speakers";
import { useT } from "../lib/i18n";
import { draftProjectKey } from "../stores/draftStore";
import { useProjectStore } from "../stores/projectStore";
import { useEditorStore } from "../stores/editorStore";
import EmptyState from "../components/EmptyState";
import ConfirmDialog from "../components/ConfirmDialog";

const ACTION_SHAPE = "rounded-md px-3 py-2 text-body font-medium disabled:opacity-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg";
const ACTION = `${ACTION_SHAPE} bg-accent-muted text-accent-fg hover:bg-accent-muted/80`;
const SELECTED_ACTION = `${ACTION_SHAPE} bg-accent text-white hover:bg-accent-hover focus-visible:ring-offset-2 focus-visible:ring-offset-surface`;
function isCurrent(projectKey: string) {
  const current = useProjectStore.getState().project;
  return !!current && draftProjectKey(current) === projectKey;
}

interface NoteState {
  text: string;
  baseline: string;
  revision: number;
  pending: number | null;
  error: string;
  saved: boolean;
}

/** Clean notes follow query refreshes; dirty or in-flight drafts retain ownership. */
function reconcileNote(note: string, state?: NoteState): NoteState {
  if (state && (state.pending !== null || state.text !== state.baseline)) return state;
  return { text: note, baseline: note, revision: state?.revision ?? 0, pending: null,
    error: state?.error ?? "", saved: !!state?.saved && state.baseline === note };
}

/**
 * Unsaved note drafts per project, kept for the session when the page unmounts
 * (route change, project reopen) so leaving never discards typed text.
 */
const keptNoteDrafts = new Map<string, Record<string, NoteState>>();

/** Drops every kept draft. Tests use it to isolate cases. */
export function forgetSpeakerNoteDrafts() {
  keptNoteDrafts.clear();
}

function keepDirtyNotes(projectKey: string, states: Record<string, NoteState>) {
  // An in-flight save cannot report back to an unmounted view; the draft text stays dirty.
  const dirty = Object.entries(states)
    .filter(([, state]) => state.text !== state.baseline)
    .map(([id, state]) => [id, { ...state, pending: null, error: "", saved: false }] as const);
  if (dirty.length) keptNoteDrafts.set(projectKey, Object.fromEntries(dirty));
  else keptNoteDrafts.delete(projectKey);
}

export default function Speakers() {
  const project = useProjectStore(s => s.project);
  const projectKey = project ? draftProjectKey(project) : "";
  return <ProjectSpeakers key={projectKey} project={project} projectKey={projectKey} />;
}

function ProjectSpeakers({ project, projectKey }: { project: ProjectInfo | null; projectKey: string }) {
  const t = useT();
  const navigate = useNavigate();
  const startValidationWorklist = useEditorStore(s => s.startValidationWorklist);
  const [selected, setSelected] = useState<string | null>(null);
  const [notes, setNotes] = useState<Record<string, NoteState>>(() => keptNoteDrafts.get(projectKey) ?? {});
  // The project owns drafts and requests, not the selected editor's mount.
  // The ref also rejects duplicate/stale events before React commits a render.
  const noteStates = useRef(notes);
  const generation = useRef(0);
  const active = useRef(true);
  const qc = useQueryClient();
  useEffect(() => {
    active.current = true;
    return () => {
      active.current = false;
      keepDirtyNotes(projectKey, noteStates.current);
    };
  }, [projectKey]);
  const updateNote = (id: string, state: NoteState) => {
    noteStates.current = { ...noteStates.current, [id]: state };
    setNotes(noteStates.current);
  };
  const owner = speakerOwner(project);
  const query = useQuery({
    queryKey: ["speakers", projectKey],
    queryFn: ({ signal }) => getSpeakers(owner!, signal),
    enabled: !!owner,
    refetchOnMount: "always",
  });
  const openRows = (ids: string[]) => {
    if (!isCurrent(projectKey) || !ids.length) return;
    startValidationWorklist(ids);
    navigate("/editor");
  };
  const currentNote = (group: SpeakerGroup) => {
    const cached = qc.getQueryData<SpeakerGroups>(["speakers", projectKey]);
    const serverNote = cached?.groups.find(g => g.id === group.id)?.note ?? group.note;
    return reconcileNote(serverNote, noteStates.current[group.id]);
  };
  const editNote = (group: SpeakerGroup, revision: number, text: string) => {
    const state = currentNote(group);
    if (!active.current || !isCurrent(projectKey) || state.revision !== revision) return;
    updateNote(group.id, { ...state, text, revision: revision + 1, error: "", saved: false });
  };
  const saveNote = async (group: SpeakerGroup, revision: number, clear = false) => {
    const state = currentNote(group);
    if (!owner || !active.current || !isCurrent(projectKey) || state.pending !== null || state.revision !== revision) return;
    const token = ++generation.current;
    const requestRevision = revision + 1;
    updateNote(group.id, { ...state, revision: requestRevision, pending: token, error: "", saved: false });
    const currentRequest = () => active.current && isCurrent(projectKey) && noteStates.current[group.id]?.pending === token;
    try {
      const result = await saveSpeakerNote({ ...owner, speaker_id: group.id, note: clear ? "" : state.text });
      if (!currentRequest()) return;
      const latest = noteStates.current[group.id];
      const unchanged = latest.revision === requestRevision;
      qc.setQueryData<SpeakerGroups>(["speakers", projectKey], previous => previous && ({ ...previous,
        groups: previous.groups.map(g => g.id === group.id ? { ...g, note: result.note } : g),
      }));
      updateNote(group.id, { ...latest, text: unchanged ? result.note : latest.text, baseline: result.note,
        revision: latest.revision + 1, pending: null, error: "", saved: unchanged });
    } catch (e) {
      if (!currentRequest()) return;
      const latest = noteStates.current[group.id];
      updateNote(group.id, { ...latest, revision: latest.revision + 1, pending: null,
        error: e instanceof Error ? e.message : String(e), saved: false });
    }
  };
  const group = query.data?.groups.find(g => g.id === selected);
  const note = group ? reconcileNote(group.note, notes[group.id]) : null;
  const unassigned = query.data?.unassigned_entry_ids ?? [];
  if (!project) return <div className="flex h-full flex-col bg-surface text-text">
    <header className="border-b border-border px-6 py-4">
      <h1 className="text-page font-bold">{t("speakers.title")}</h1>
    </header>
    <EmptyState title={t("editor.empty.title")} description={t("speakers.openProject")} actionLabel={t("common.goHome")} actionTo="/" />
  </div>;
  return <div className="flex h-full flex-col bg-surface text-text">
    <header className="space-y-2 border-b border-border px-6 py-4">
      <h1 className="text-page font-bold">{t("speakers.title")}</h1>
      <p className="text-body text-text-muted">{t("speakers.contextHint")}</p>
      <p className="text-body text-text-muted">{t("speakers.evidenceHint")}</p>
    </header>
    <div className="flex-1 overflow-auto p-6">
      {!owner ? <><EmptyState title={t("speakers.noProject")} description={t(project ? "speakers.reopen" : "speakers.openProject")} /><Link to="/" className="text-accent-fg underline">{t("qa.home")}</Link></>
        : query.isError ? <div role="alert" className="space-y-3 text-danger"><p>{t("speakers.loadError")} {query.error.message}</p><button type="button" className={ACTION} onClick={() => { void query.refetch(); }}>{t("common.retry")}</button></div>
        : query.isPending ? <p role="status">{t("common.loading")}</p>
        : !query.data?.groups.length && !unassigned.length ? <EmptyState title={t("speakers.empty")} description={t("speakers.evidenceHint")} />
        : <div className="grid gap-6 md:grid-cols-2">
          <div className="space-y-3">
            <ul className="space-y-2" aria-label={t("speakers.title")}>
              {query.data?.groups.map(g => <li key={g.id}><button key={g.id} type="button" className={selected === g.id ? SELECTED_ACTION : ACTION} aria-pressed={selected === g.id} onClick={() => setSelected(g.id)}>{g.name} ({g.count})</button></li>)}
            </ul>
            <div className="space-y-2 rounded-lg border border-border p-4">
              <h2 className="font-semibold">{t("speakers.unassigned")} ({unassigned.length})</h2>
              <button type="button" className={ACTION} disabled={!unassigned.length} onClick={() => openRows(unassigned)}>{t("speakers.openUnassigned")}</button>
            </div>
          </div>
          {group ? <section className="space-y-3 rounded-lg border border-border p-4">
            <h2 className="font-semibold">{group.name} ({group.count})</h2>
            <button type="button" className={ACTION} onClick={() => openRows(group.entry_ids)}>{t("speakers.openRows")}</button>
            <SpeakerNoteEditor key={JSON.stringify([projectKey, group.id])} name={group.name} state={note!}
              onEdit={text => editNote(group, note!.revision, text)}
              onSave={() => saveNote(group, note!.revision)} onClear={() => saveNote(group, note!.revision, true)} />
          </section> : <p className="text-body text-text-muted">{t("speakers.select")}</p>}
        </div>}
    </div>
  </div>;
}

function SpeakerNoteEditor({ name, state, onEdit, onSave, onClear }: {
  name: string; state: NoteState; onEdit: (text: string) => void; onSave: () => Promise<void>; onClear: () => Promise<void>;
}) {
  const t = useT();
  const [confirmClear, setConfirmClear] = useState(false);
  const { text, pending, saved, error } = state;
  return <div className="space-y-3">
    <label className="block space-y-2 text-body"><span>{t("speakers.note")}</span>
      <textarea className="min-h-40 w-full rounded-md border border-border bg-surface p-3 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg" value={text} onChange={e => onEdit(e.target.value)} />
    </label>
    <div className="flex gap-2">
      <button type="button" className={ACTION} disabled={pending !== null} onClick={onSave}>{t("speakers.save")}</button>
      <button type="button" className={ACTION} disabled={pending !== null} onClick={() => setConfirmClear(true)}>{t("speakers.clear")}</button>
    </div>
    <ConfirmDialog open={confirmClear} title={t("speakers.clearConfirm.title")} message={t("speakers.clearConfirm.message", { name })}
      confirmLabel={t("speakers.clear")} destructive onCancel={() => setConfirmClear(false)}
      onConfirm={() => { setConfirmClear(false); return onClear(); }} />
    {pending !== null && <p role="status" className="text-body text-text-muted">{t("speakers.saving")}</p>}
    {saved && <p role="status" className="text-body text-text-muted">{t("speakers.saved")}</p>}
    {error && <p role="alert" className="text-body text-danger">{t("speakers.saveError")} {error}</p>}
  </div>;
}
