import { useState, useCallback } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Languages, Shield, Download, FileCheck, Package, Loader2 } from "lucide-react";
import { getStrings, getStats, getString, validate, type ValidationResponse } from "../lib/api";
import { useEditorStore } from "../stores/editorStore";
import { useProjectStore } from "../stores/projectStore";
import { useHotkey } from "../lib/hotkeys";
import { addToast } from "../stores/toastStore";
import { addLog } from "../stores/logStore";
import FilterBar from "../components/FilterBar";
import StringTable from "../components/StringTable";
import DetailPanel from "../components/DetailPanel";
import TranslationModal from "../components/TranslationModal";
import InjectModal from "../components/InjectModal";
import PatchModal from "../components/PatchModal";
import ExportModal from "../components/ExportModal";
import SearchReplaceModal from "../components/SearchReplaceModal";
import ValidationResultsModal from "../components/ValidationResultsModal";

export default function Editor() {
  const { filter, selectedEntryId, setSelected } = useEditorStore();
  const { project } = useProjectStore();
  const queryClient = useQueryClient();
  const [showTranslateModal, setShowTranslateModal] = useState(false);
  const [showInjectModal, setShowInjectModal] = useState(false);
  const [showPatchModal, setShowPatchModal] = useState(false);
  const [patchInitialTab, setPatchInitialTab] = useState<"apply" | "pack">("apply");
  const [showExportModal, setShowExportModal] = useState(false);
  const [showReplaceModal, setShowReplaceModal] = useState(false);
  const [showValidationModal, setShowValidationModal] = useState(false);
  const [validationResult, setValidationResult] = useState<ValidationResponse | null>(null);
  const [validating, setValidating] = useState(false);

  const { data: stringsData, refetch } = useQuery({
    queryKey: ["strings", filter],
    queryFn: () => getStrings(filter),
    staleTime: 30_000,
  });

  const { data: statsData } = useQuery({
    queryKey: ["stats"],
    queryFn: getStats,
    staleTime: 10_000,
  });

  const { data: selectedEntry } = useQuery({
    queryKey: ["string", selectedEntryId],
    queryFn: () => getString(selectedEntryId!),
    enabled: !!selectedEntryId,
  });

  const handleRefetch = useCallback(() => {
    refetch();
    queryClient.invalidateQueries({ queryKey: ["stats"] });
    if (selectedEntryId) {
      queryClient.invalidateQueries({ queryKey: ["string", selectedEntryId] });
    }
  }, [refetch, queryClient, selectedEntryId]);

  const handleValidate = useCallback(async () => {
    if (validating) return;
    setValidating(true);
    try {
      const res = await validate();
      // Normalize older servers without issues[]
      if (!res.validation.issues) {
        res.validation.issues = [];
      }
      if (!res.fonts) {
        res.fonts = [];
      }
      setValidationResult(res);
      setShowValidationModal(true);

      const v = res.validation;
      const kinds = Object.entries(v.by_kind || {})
        .map(([k, n]) => `${k}: ${n}`)
        .join(", ");
      if (v.issues_found === 0) {
        addLog("info", `Validate: no issues in ${v.total_checked} strings`, undefined, "validate");
      } else {
        const msg = `${v.issues_found} issue(s) in ${v.entries_with_issues} entries${kinds ? ` (${kinds})` : ""}`;
        addLog("warning", `Validate: ${msg}`, undefined, "validate");
      }
    } catch (e) {
      const err = e instanceof Error ? e.message : String(e);
      addToast("error", `Validate failed: ${err}`);
      addLog("error", "Validate failed", err, "validate");
    } finally {
      setValidating(false);
    }
  }, [validating]);

  // Project-gated actions: buttons are disabled without a project, and the
  // matching hotkeys give toast feedback instead of a silent no-op.
  const hasProject = !!project;
  const requireProject = useCallback((fn: () => void) => {
    if (!useProjectStore.getState().project) {
      addToast("info", "Open a project first");
      return;
    }
    fn();
  }, []);

  // Hotkeys
  useHotkey("translate", () => requireProject(() => setShowTranslateModal(true)));
  useHotkey("inject", () => requireProject(() => setShowInjectModal(true)));
  useHotkey("applyPatch", () => requireProject(() => setShowPatchModal(true)));
  useHotkey("validate", () => requireProject(() => { void handleValidate(); }));
  useHotkey("exportFile", () => requireProject(() => setShowExportModal(true)));
  useHotkey("searchReplace", () => setShowReplaceModal(true));
  useHotkey("closePanel", () => {
    if (showValidationModal) setShowValidationModal(false);
    else if (showReplaceModal) setShowReplaceModal(false);
    else if (showExportModal) setShowExportModal(false);
    else if (showPatchModal) setShowPatchModal(false);
    else if (showInjectModal) setShowInjectModal(false);
    else if (showTranslateModal) setShowTranslateModal(false);
    else if (selectedEntryId) setSelected(null);
  });
  useHotkey("search", () => {
    document.querySelector<HTMLInputElement>('[data-search-input]')?.focus();
  });

  const entries = stringsData?.entries || [];
  const total = stringsData?.total || 0;

  return (
    <div className="flex flex-col h-full">
      {/* Top bar */}
      <div className="flex items-center gap-3 px-4 py-2 border-b border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900">
        <div className="flex-1">
          <span className="font-semibold">{project?.name || "No project"}</span>
          {project && (
            <span className="ml-2 px-2 py-0.5 bg-emerald-100 text-emerald-700 dark:bg-emerald-900 dark:text-emerald-300 rounded text-xs font-medium">
              {project.format_id}
            </span>
          )}
          {statsData && (
            <span className="ml-3 text-xs text-gray-500">
              {statsData.pending} pending · {statsData.translated} translated · {statsData.approved} approved
            </span>
          )}
        </div>

        <button
          onClick={() => setShowTranslateModal(true)}
          disabled={!hasProject}
          className="flex items-center gap-1.5 px-3 py-1.5 bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50 disabled:cursor-not-allowed text-white rounded text-sm font-medium transition-colors"
          title={hasProject ? "Ctrl+T" : "Open a project first"}
        >
          <Languages size={16} /> Translate
        </button>

        <button
          onClick={() => setShowInjectModal(true)}
          disabled={!hasProject}
          className="flex items-center gap-1.5 px-3 py-1.5 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 disabled:opacity-50 disabled:cursor-not-allowed rounded text-sm font-medium transition-colors"
          title={hasProject ? "Ctrl+I" : "Open a project first"}
        >
          <FileCheck size={16} /> Inject
        </button>

        <button
          onClick={() => setShowPatchModal(true)}
          disabled={!hasProject}
          className="flex items-center gap-1.5 px-3 py-1.5 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 disabled:opacity-50 disabled:cursor-not-allowed rounded text-sm font-medium transition-colors"
          title={hasProject ? "Ctrl+Shift+P" : "Open a project first"}
        >
          <Package size={16} /> Patch
        </button>

        <button
          onClick={() => { void handleValidate(); }}
          disabled={validating || !hasProject}
          className="flex items-center gap-1.5 px-3 py-1.5 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 disabled:opacity-50 disabled:cursor-not-allowed rounded text-sm font-medium transition-colors"
          title={hasProject ? "Ctrl+Shift+V" : "Open a project first"}
        >
          {validating ? <Loader2 size={16} className="animate-spin" /> : <Shield size={16} />}
          Validate
        </button>

        <button
          onClick={() => setShowExportModal(true)}
          disabled={!hasProject}
          className="flex items-center gap-1.5 px-3 py-1.5 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 disabled:opacity-50 disabled:cursor-not-allowed rounded text-sm font-medium transition-colors"
          title={hasProject ? "Ctrl+E" : "Open a project first"}
        >
          <Download size={16} /> Export
        </button>
      </div>

      {/* Filter + Table + Detail */}
      <FilterBar total={total} showing={entries.length} />

      <div className="flex flex-1 overflow-hidden">
        <StringTable data={entries} onRefetch={handleRefetch} />
        {selectedEntry && (
          <DetailPanel
            entry={selectedEntry}
            onRefetch={handleRefetch}
            onClose={() => setSelected(null)}
          />
        )}
      </div>

      {/* Translation Modal */}
      <TranslationModal
        open={showTranslateModal}
        onClose={() => setShowTranslateModal(false)}
        totalPending={statsData?.pending || 0}
        onComplete={handleRefetch}
      />

      {/* Inject Modal */}
      <InjectModal
        open={showInjectModal}
        onClose={() => setShowInjectModal(false)}
        onOpenPack={() => {
          setPatchInitialTab("pack");
          setShowPatchModal(true);
        }}
      />

      {/* Patch apply / rollback / pack */}
      <PatchModal
        open={showPatchModal}
        onClose={() => {
          setShowPatchModal(false);
          setPatchInitialTab("apply");
        }}
        defaultGamePath={project?.path}
        initialTab={patchInitialTab}
      />

      {/* PO / XLIFF export + import */}
      <ExportModal
        open={showExportModal}
        onClose={() => setShowExportModal(false)}
        onImported={handleRefetch}
      />

      <SearchReplaceModal
        open={showReplaceModal}
        onClose={() => setShowReplaceModal(false)}
        onDone={handleRefetch}
      />

      <ValidationResultsModal
        open={showValidationModal}
        result={validationResult}
        onClose={() => setShowValidationModal(false)}
        onSelectEntry={(entryId) => {
          setShowValidationModal(false);
          setSelected(entryId);
        }}
      />
    </div>
  );
}
