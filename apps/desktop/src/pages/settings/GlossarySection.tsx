import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader, Plus, Search, Trash2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { SETTINGS_INPUT_CLASS } from "../../components/BufferedSetting";
import ConfirmDialog from "../../components/ConfirmDialog";
import type { GlossaryEntry } from "../../lib/api";
import { addGlossaryEntry, deleteGlossaryEntry, getConfig, getGlossary } from "../../lib/api";
import { useT } from "../../lib/i18n";
import { settingsQueryState } from "../../lib/settingsQueryState";
import { useProjectStore } from "../../stores/projectStore";
import { addToast } from "../../stores/toastStore";

export default function GlossarySection() {
  const t = useT();
  const project = useProjectStore((s) => s.project);
  const projectScope = project?.database_path ?? project?.path ?? null;
  const { data: config } = useQuery({ queryKey: ["config"], queryFn: getConfig });
  const qc = useQueryClient();
  const configPair = config
    ? `${config.default_source_lang}-${config.default_target_lang}`
    : "ja-en";
  const [langPairOverride, setLangPairOverride] = useState<string | null>(null);
  const activePair = (langPairOverride ?? configPair).trim() || "ja-en";

  const { data: entries, isLoading, isError, error } = useQuery({
    queryKey: ["glossary", projectScope, activePair],
    queryFn: () => getGlossary(activePair),
    enabled: projectScope !== null && !!activePair,
  });
  const queryState = projectScope === null ? "no_project" : isError ? settingsQueryState(error) : null;

  const [filter, setFilter] = useState("");
  const [term, setTerm] = useState("");
  const [translation, setTranslation] = useState("");
  const [saving, setSaving] = useState(false);
  const [entryToDelete, setEntryToDelete] = useState<{
    entry: GlossaryEntry;
    projectScope: string | null;
  } | null>(null);

  useEffect(() => {
    setEntryToDelete(null);
    setTerm("");
    setTranslation("");
    setSaving(false);
  }, [projectScope]);

  const isCurrentProject = (scope: string | null) => {
    const current = useProjectStore.getState().project;
    return scope !== null && scope === (current?.database_path ?? current?.path ?? null);
  };

  const canSubmit = (scope: string | null) => {
    if (isCurrentProject(scope)) return true;
    addToast("error", t("settings.glossary.toast.projectChanged"));
    return false;
  };

  const invalidateGlossary = (scope: string | null) => {
    // A completed mutation must never refetch another project's backend into this cache.
    void qc.invalidateQueries({
      queryKey: ["glossary", scope],
      refetchType: isCurrentProject(scope) ? "active" : "none",
    });
  };

  const filtered = useMemo(() => {
    const list = entries ?? [];
    const q = filter.trim().toLowerCase();
    if (!q) return list;
    return list.filter(
      (e) =>
        e.term.toLowerCase().includes(q) ||
        e.translation.toLowerCase().includes(q) ||
        (e.context?.toLowerCase().includes(q) ?? false)
    );
  }, [entries, filter]);

  const handleAdd = async () => {
    if (!canSubmit(projectScope)) return;
    const termText = term.trim();
    const tr = translation.trim();
    if (!termText || !tr) {
      addToast("error", t("settings.glossary.toast.termRequired"));
      return;
    }
    if (!activePair.trim()) {
      addToast("error", t("settings.glossary.toast.pairRequired"));
      return;
    }
    setSaving(true);
    try {
      const entry: GlossaryEntry = {
        term: termText,
        translation: tr,
        lang_pair: activePair.trim(),
        context: null,
        case_sensitive: false,
      };
      await addGlossaryEntry(entry);
      if (isCurrentProject(projectScope)) {
        setTerm("");
        setTranslation("");
      }
      invalidateGlossary(projectScope);
    } catch (e: any) {
      addToast("error", t("settings.glossary.toast.addFailed", { error: e.message }));
    } finally {
      if (isCurrentProject(projectScope)) setSaving(false);
    }
  };

  const handleDeleteConfirmed = async (pending: NonNullable<typeof entryToDelete>) => {
    setEntryToDelete(null);
    if (!canSubmit(pending.projectScope)) return;
    try {
      const { entry } = pending;
      await deleteGlossaryEntry(entry.term, entry.lang_pair);
      invalidateGlossary(pending.projectScope);
    } catch (e: any) {
      addToast("error", t("settings.glossary.toast.deleteFailed", { error: e.message }));
    }
  };

  return (
    <div className="space-y-6">
      <h2 className="text-page font-bold">{t("settings.glossary.title")}</h2>
      <p className="text-body text-text-muted -mt-4">
        {t("settings.glossary.description")}
      </p>

      <div className="grid grid-cols-1 sm:grid-cols-3 gap-4 items-end border border-border bg-surface rounded-xl p-5">
        <div>
          <label className="text-body font-medium">{t("settings.glossary.langPair")}</label>
          <input
            value={langPairOverride ?? configPair}
            onChange={(e) => setLangPairOverride(e.target.value)}
            placeholder="ja-en"
            className={`mt-1 ${SETTINGS_INPUT_CLASS}`}
          />
        </div>
        <div className="sm:col-span-2 relative">
          <label className="text-body font-medium">{t("settings.glossary.filter")}</label>
          <div className="relative mt-1">
            <Search size={14} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-text-muted" />
            <input
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder={t("settings.glossary.filterPlaceholder")}
              className={`${SETTINGS_INPUT_CLASS} pl-8 pr-3`}
            />
          </div>
        </div>
      </div>

      <div className="border border-border bg-surface rounded-xl p-5 space-y-4">
        <h3 className="text-section font-semibold">{t("settings.glossary.addEntry")}</h3>
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div>
            <label className="text-body text-text-muted">{t("settings.glossary.term")}</label>
            <input
              value={term}
              onChange={(e) => setTerm(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleAdd()}
              placeholder={t("settings.glossary.sourceTerm")}
              className={`mt-1 ${SETTINGS_INPUT_CLASS}`}
            />
          </div>
          <div>
            <label className="text-body text-text-muted">{t("settings.glossary.translation")}</label>
            <input
              value={translation}
              onChange={(e) => setTranslation(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleAdd()}
              placeholder={t("settings.glossary.preferredTranslation")}
              className={`mt-1 ${SETTINGS_INPUT_CLASS}`}
            />
          </div>
        </div>
        <button
          onClick={handleAdd}
          disabled={saving || projectScope === null}
          className="flex items-center gap-1.5 px-3 py-2 bg-accent hover:bg-accent-hover disabled:opacity-50 text-white rounded-md text-body font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-2 focus-visible:ring-offset-surface"
        >
          <Plus size={14} /> {saving ? t("settings.glossary.adding") : t("settings.glossary.addEntry")}
        </button>
      </div>

      <div>
        <h3 className="text-section font-semibold mb-3">
          {entries ? t("settings.glossary.entries", { count: filtered.length }) : t("settings.glossary.entriesBare")}
        </h3>
        {isLoading ? (
          <p className="text-body text-text-muted flex items-center gap-2">
            <Loader size={14} className="animate-spin" /> {t("common.loading")}
          </p>
        ) : queryState === "no_project" ? (
          <p role="status" className="text-body text-text-muted">
            {t("settings.glossary.noProject")}
          </p>
        ) : queryState === "failed" ? (
          <p role="alert" className="text-body text-danger">
            {t("settings.glossary.loadFailed", { error: error?.message ?? "" })}
          </p>
        ) : filtered.length === 0 ? (
          <p className="text-body text-text-muted">
            {filter
              ? t("settings.glossary.emptyFilter")
              : t("settings.glossary.empty")}
          </p>
        ) : (
          <div className="overflow-x-auto border border-border bg-surface rounded-xl">
          <table className="w-full min-w-[36rem] text-body">
            <thead>
              <tr className="bg-surface-muted text-left text-text-muted text-caption uppercase tracking-wide">
                <th className="px-3 py-2">{t("settings.glossary.col.term")}</th>
                <th className="px-3 py-2">{t("settings.glossary.col.translation")}</th>
                <th className="px-3 py-2 w-24">{t("settings.glossary.col.pair")}</th>
                <th className="px-3 py-2 w-12"></th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((e) => (
                <tr
                  key={`${e.lang_pair}:${e.term}`}
                  className="border-t border-border hover:bg-surface-muted"
                >
                  <td className="px-3 py-2 font-medium break-words">{e.term}</td>
                  <td className="px-3 py-2 break-words">{e.translation}</td>
                  <td className="px-3 py-2">
                    <span className="px-2 py-0.5 bg-accent-muted text-accent-fg rounded text-caption whitespace-nowrap">
                      {e.lang_pair}
                    </span>
                  </td>
                  <td className="px-3 py-2">
                    <button
                      onClick={() => setEntryToDelete({ entry: e, projectScope })}
                      className="inline-flex items-center justify-center rounded-md p-2 text-danger hover:bg-danger-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-danger"
                      title={t("settings.glossary.deleteTitle")}
                    >
                      <Trash2 size={14} />
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          </div>
        )}
      </div>

      <ConfirmDialog
        open={entryToDelete !== null && entryToDelete.projectScope === projectScope}
        title={t("settings.glossary.confirm.title")}
        message={entryToDelete ? t("settings.glossary.confirm.message", { term: entryToDelete.entry.term }) : ""}
        confirmLabel={t("common.delete")}
        destructive
        onConfirm={() => entryToDelete && handleDeleteConfirmed(entryToDelete)}
        onCancel={() => setEntryToDelete(null)}
      />
    </div>
  );
}
