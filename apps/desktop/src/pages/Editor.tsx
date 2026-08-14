import { useState, useCallback, useEffect, useRef } from "react";
import { useNavigate } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
	Languages,
	Shield,
	Download,
	FileCheck,
	Package,
	Loader2,
	Replace,
	ChevronRight,
} from "lucide-react";
import {
	getStrings,
	getStats,
	getString,
	validate,
	type ValidationResponse,
} from "../lib/api";
import { useEditorStore } from "../stores/editorStore";
import { useProjectStore } from "../stores/projectStore";
import { useHotkey } from "../lib/hotkeys";
import {
	readSkipReviewPreference,
	readWorkflowGuideDismissed,
	resolveWorkflowGuideStep,
	saveSkipReviewPreference,
	saveWorkflowGuideDismissed,
} from "../lib/workflowGuide";
import { addToast } from "../stores/toastStore";
import { addLog } from "../stores/logStore";
import FilterBar from "../components/FilterBar";
import StringTable from "../components/StringTable";
import DetailPanel from "../components/DetailPanel";
import TranslationModal from "../components/TranslationModal";
import InjectModal from "../components/InjectModal";
import PatchModal from "../components/PatchModal";
import PatchStatusIndicator from "../components/PatchStatusIndicator";
import ExportModal from "../components/ExportModal";
import SearchReplaceModal from "../components/SearchReplaceModal";
import ValidationResultsModal from "../components/ValidationResultsModal";
import WorkflowGuideBanner from "../components/WorkflowGuideBanner";
import EmptyState from "../components/EmptyState";
import { useT } from "../lib/i18n";

export default function Editor() {
	const t = useT();
	const { filter, selectedEntryId, setSelected, isTranslating } = useEditorStore();
	const { project } = useProjectStore();
	const navigate = useNavigate();
	const queryClient = useQueryClient();
	const [showTranslateModal, setShowTranslateModal] = useState(false);
	const [showInjectModal, setShowInjectModal] = useState(false);
	const [showPatchModal, setShowPatchModal] = useState(false);
	const [patchInitialTab, setPatchInitialTab] = useState<"apply" | "pack">(
		"apply",
	);
	const [patchStatusRefreshKey, setPatchStatusRefreshKey] = useState(0);
	const [showExportModal, setShowExportModal] = useState(false);
	const [showReplaceModal, setShowReplaceModal] = useState(false);
	const [showValidationModal, setShowValidationModal] = useState(false);
	const [validationResult, setValidationResult] =
		useState<ValidationResponse | null>(null);
	const [validating, setValidating] = useState(false);
	const [guideDismissed, setGuideDismissed] = useState<boolean>(() =>
		readWorkflowGuideDismissed(),
	);
	const [skipReview, setSkipReview] = useState<boolean>(() =>
		readSkipReviewPreference(),
	);

	const {
		data: stringsData,
		isLoading: stringsLoading,
		isError: stringsError,
		error: stringsErrorDetail,
		refetch,
	} = useQuery({
		queryKey: ["strings", filter],
		queryFn: () => getStrings(filter),
		staleTime: 30_000,
		enabled: !!project,
	});

	const { data: statsData } = useQuery({
		queryKey: ["stats"],
		queryFn: getStats,
		staleTime: 10_000,
		enabled: !!project,
	});

	const { data: selectedEntry } = useQuery({
		queryKey: ["string", selectedEntryId],
		queryFn: () => getString(selectedEntryId!),
		enabled: !!project && !!selectedEntryId,
	});

	const bumpPatchStatus = useCallback(() => {
		setPatchStatusRefreshKey((key) => key + 1);
	}, []);

	const handleRefetch = useCallback(() => {
		refetch();
		queryClient.invalidateQueries({ queryKey: ["stats"] });
		if (selectedEntryId) {
			queryClient.invalidateQueries({ queryKey: ["string", selectedEntryId] });
		}
	}, [refetch, queryClient, selectedEntryId]);

	const wasTranslating = useRef(false);
	useEffect(() => {
		if (wasTranslating.current && !isTranslating) {
			handleRefetch();
		}
		wasTranslating.current = isTranslating;
	}, [isTranslating, handleRefetch]);

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
				addLog(
					"info",
					`Validate: no issues in ${v.total_checked} strings`,
					undefined,
					"validate",
				);
			} else {
				const msg = `${v.issues_found} issue(s) in ${v.entries_with_issues} entries${kinds ? ` (${kinds})` : ""}`;
				addLog("warning", `Validate: ${msg}`, undefined, "validate");
			}
		} catch (e) {
			const err = e instanceof Error ? e.message : String(e);
			addToast("error", t("editor.toast.validateFailed", { error: err }));
			addLog("error", "Validate failed", err, "validate");
		} finally {
			setValidating(false);
		}
	}, [validating, t]);

	// Project-gated actions: buttons are disabled without a project, and the
	// matching hotkeys give toast feedback instead of a silent no-op.
	const hasProject = !!project;
	const requireProject = useCallback((fn: () => void) => {
		if (!useProjectStore.getState().project) {
			addToast("info", t("editor.toast.openProjectFirst"));
			return;
		}
		fn();
	}, [t]);

	const editorModalOpen =
		showTranslateModal ||
		showInjectModal ||
		showPatchModal ||
		showExportModal ||
		showReplaceModal ||
		showValidationModal;

	// Action hotkeys pause behind work modals; Escape remains available in their inputs.
	useHotkey(
		"translate",
		() => requireProject(() => setShowTranslateModal(true)),
		!editorModalOpen,
	);
	useHotkey(
		"inject",
		() => requireProject(() => setShowInjectModal(true)),
		!editorModalOpen,
	);
	useHotkey(
		"applyPatch",
		() => requireProject(() => setShowPatchModal(true)),
		!editorModalOpen,
	);
	useHotkey(
		"validate",
		() =>
			requireProject(() => {
				void handleValidate();
			}),
		!editorModalOpen,
	);
	useHotkey(
		"exportFile",
		() => requireProject(() => setShowExportModal(true)),
		!editorModalOpen,
	);
	useHotkey(
		"searchReplace",
		() => requireProject(() => setShowReplaceModal(true)),
		!editorModalOpen,
	);
	useHotkey(
		"search",
		() => {
			document.querySelector<HTMLInputElement>("[data-search-input]")?.focus();
		},
		!editorModalOpen,
	);
	useHotkey(
		"closePanel",
		() => {
			if (showValidationModal) setShowValidationModal(false);
			else if (showReplaceModal) setShowReplaceModal(false);
			else if (showExportModal) setShowExportModal(false);
			else if (showPatchModal) setShowPatchModal(false);
			else if (showInjectModal) setShowInjectModal(false);
			else if (showTranslateModal) setShowTranslateModal(false);
			else if (selectedEntryId) setSelected(null);
		},
		editorModalOpen || !!selectedEntryId,
		true,
	);

	const entries = stringsData?.entries || [];
	const total = stringsData?.total || 0;
	const hasActiveFilters = Boolean(
		filter.status || filter.search || filter.file_path || filter.tag,
	);
	const workflowStep = resolveWorkflowGuideStep({
		hasProject,
		stats: statsData,
		skipReview,
	});

	const handleGuidePrimaryAction = () => {
		if (workflowStep === "translate") setShowTranslateModal(true);
		else if (workflowStep === "review") navigate("/review");
		else if (workflowStep === "inject") setShowInjectModal(true);
	};

	const handleSkipReview = () => {
		saveSkipReviewPreference(true);
		setSkipReview(true);
	};

	const handleDismissGuide = () => {
		saveWorkflowGuideDismissed(true);
		setGuideDismissed(true);
	};

	return (
		<div className="flex flex-col h-full">
			{/* Top bar */}
			<div className="flex items-center gap-3 px-4 py-2 border-b border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900">
				<div className="flex-1">
					<span className="font-semibold">{project?.name || t("editor.noProject")}</span>
					{project && (
						<span className="ml-2 px-2 py-0.5 bg-emerald-100 text-emerald-700 dark:bg-emerald-900 dark:text-emerald-300 rounded text-xs font-medium">
							{project.format_id}
						</span>
					)}
					{statsData && (
						<span className="ml-3 text-xs text-gray-500">
							{t("editor.stats", {
								pending: statsData.pending,
								translated: statsData.translated,
								approved: statsData.approved,
							})}
						</span>
					)}
					{hasProject && guideDismissed && workflowStep && (
						<button
							type="button"
							onClick={handleGuidePrimaryAction}
							title={t("editor.nextStepTitle")}
							className="ml-3 inline-flex items-center gap-0.5 rounded-full border border-emerald-300 px-2 py-0.5 text-xs font-medium text-emerald-700 transition-colors hover:bg-emerald-50 dark:border-emerald-800 dark:text-emerald-300 dark:hover:bg-emerald-950/40"
						>
							{t("editor.next", { step: t(`workflow.${workflowStep}`) })}
							<ChevronRight size={12} aria-hidden="true" />
						</button>
					)}
				</div>

				{/* Primary workflow actions */}
				<div className="flex items-center gap-2">
					<button
						onClick={() => setShowTranslateModal(true)}
						disabled={!hasProject}
						className="flex items-center gap-1.5 px-3 py-1.5 bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50 disabled:cursor-not-allowed text-white rounded text-sm font-medium transition-colors"
						title={hasProject ? "Ctrl+T" : t("editor.hotkeyOrProject")}
					>
						<Languages size={16} /> {t("editor.translate")}
					</button>

					<button
						onClick={() => setShowInjectModal(true)}
						disabled={!hasProject}
						className="flex items-center gap-1.5 px-3 py-1.5 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 disabled:opacity-50 disabled:cursor-not-allowed rounded text-sm font-medium transition-colors"
						title={hasProject ? "Ctrl+I" : t("editor.hotkeyOrProject")}
					>
						<FileCheck size={16} /> {t("editor.inject")}
					</button>

					<button
						onClick={() => setShowPatchModal(true)}
						disabled={!hasProject}
						className="flex items-center gap-1.5 px-3 py-1.5 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 disabled:opacity-50 disabled:cursor-not-allowed rounded text-sm font-medium transition-colors"
						title={hasProject ? "Ctrl+Shift+P" : t("editor.hotkeyOrProject")}
					>
						<Package size={16} /> {t("editor.patch")}
					</button>

					<PatchStatusIndicator
						gamePath={project?.path}
						onOpenPatch={() => setShowPatchModal(true)}
						refreshKey={patchStatusRefreshKey}
					/>
				</div>

				<div
					aria-hidden="true"
					className="h-6 w-px bg-gray-200 dark:bg-gray-700"
				/>

				{/* Secondary project tools */}
				<div className="flex items-center gap-2">
					<button
						onClick={() => {
							void handleValidate();
						}}
						disabled={validating || !hasProject}
						className="flex items-center gap-1.5 px-3 py-1.5 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 disabled:opacity-50 disabled:cursor-not-allowed rounded text-sm font-medium transition-colors"
						title={hasProject ? "Ctrl+Shift+V" : t("editor.hotkeyOrProject")}
					>
						{validating ? (
							<Loader2 size={16} className="animate-spin" />
						) : (
							<Shield size={16} />
						)}
						{t("editor.validate")}
					</button>

					<button
						onClick={() => setShowExportModal(true)}
						disabled={!hasProject}
						className="flex items-center gap-1.5 px-3 py-1.5 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 disabled:opacity-50 disabled:cursor-not-allowed rounded text-sm font-medium transition-colors"
						title={hasProject ? "Ctrl+E" : t("editor.hotkeyOrProject")}
					>
						<Download size={16} /> {t("editor.export")}
					</button>

					<button
						onClick={() => setShowReplaceModal(true)}
						disabled={!hasProject}
						className="flex items-center gap-1.5 px-3 py-1.5 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 disabled:opacity-50 disabled:cursor-not-allowed rounded text-sm font-medium transition-colors"
						title={hasProject ? "Ctrl+Shift+F" : t("editor.hotkeyOrProject")}
					>
						<Replace size={16} /> {t("editor.replace")}
					</button>
				</div>
			</div>

			{!guideDismissed && workflowStep && (
				<WorkflowGuideBanner
					step={workflowStep}
					onPrimaryAction={handleGuidePrimaryAction}
					onSkipReview={
						workflowStep === "review" && !skipReview
							? handleSkipReview
							: undefined
					}
					onDismiss={handleDismissGuide}
				/>
			)}

			{/* Filter + Table + Detail */}
			{hasProject ? (
				<FilterBar total={total} showing={entries.length} entries={entries} />
			) : null}

			<div className="flex flex-1 overflow-hidden">
				{!hasProject ? (
					<EmptyState
						title={t("editor.empty.title")}
						description={t("editor.empty.description")}
						actionLabel={t("editor.empty.action")}
						onAction={() => navigate("/")}
					/>
				) : stringsLoading ? (
					<div className="flex flex-1 items-center justify-center text-gray-500">
						{t("editor.loadingStrings")}
					</div>
				) : stringsError ? (
					<div className="flex flex-1 flex-col items-center justify-center gap-3 px-6 text-center">
						<p className="font-medium text-red-600">{t("editor.loadError")}</p>
						<p className="text-sm text-gray-500">
							{stringsErrorDetail instanceof Error
								? stringsErrorDetail.message
								: t("common.tryAgain")}
						</p>
						<button
							onClick={() => {
								void refetch();
							}}
							className="rounded bg-emerald-600 px-4 py-2 text-sm font-medium text-white hover:bg-emerald-700"
						>
							{t("common.retry")}
						</button>
					</div>
				) : (
					<StringTable
						data={entries}
						onRefetch={handleRefetch}
						hasActiveFilters={hasActiveFilters}
					/>
				)}
				{hasProject && !stringsLoading && !stringsError && selectedEntry && (
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
				onReview={() => navigate("/review")}
			/>

			{/* Inject Modal */}
			<InjectModal
				open={showInjectModal}
				onClose={() => setShowInjectModal(false)}
				onOpenPack={() => {
					setShowInjectModal(false);
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
				onPatchStateChanged={bumpPatchStatus}
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
