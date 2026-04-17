import { useState, useCallback } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Languages, Shield, Download, FileCheck } from "lucide-react";
import { getStrings, getStats, getString } from "../lib/api";
import { useEditorStore } from "../stores/editorStore";
import { useProjectStore } from "../stores/projectStore";
import { useHotkey } from "../lib/hotkeys";
import FilterBar from "../components/FilterBar";
import StringTable from "../components/StringTable";
import DetailPanel from "../components/DetailPanel";
import TranslationModal from "../components/TranslationModal";
import InjectModal from "../components/InjectModal";

export default function Editor() {
  const { filter, selectedEntryId, setSelected } = useEditorStore();
  const { project } = useProjectStore();
  const queryClient = useQueryClient();
  const [showTranslateModal, setShowTranslateModal] = useState(false);
  const [showInjectModal, setShowInjectModal] = useState(false);

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

  // Hotkeys
  useHotkey("translate", () => setShowTranslateModal(true));
  useHotkey("inject", () => setShowInjectModal(true));
  useHotkey("closePanel", () => {
    if (showInjectModal) setShowInjectModal(false);
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
          className="flex items-center gap-1.5 px-3 py-1.5 bg-emerald-600 hover:bg-emerald-700 text-white rounded text-sm font-medium transition-colors"
          title="Ctrl+T"
        >
          <Languages size={16} /> Translate
        </button>

        <button
          onClick={() => setShowInjectModal(true)}
          className="flex items-center gap-1.5 px-3 py-1.5 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded text-sm font-medium transition-colors"
          title="Ctrl+I"
        >
          <FileCheck size={16} /> Inject
        </button>

        <button
          className="flex items-center gap-1.5 px-3 py-1.5 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded text-sm font-medium transition-colors"
          title="Ctrl+Shift+V"
        >
          <Shield size={16} /> Validate
        </button>

        <button
          className="flex items-center gap-1.5 px-3 py-1.5 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded text-sm font-medium transition-colors"
          title="Ctrl+E"
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
      />
    </div>
  );
}
