import { useState, useCallback, useEffect, useMemo, useRef } from "react";
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
	GitBranch,
} from "lucide-react";
import {
	getStrings,
	getStats,
	getString,
	validate,
	type ProjectStats,
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
import { draftEntryKey, draftProjectKey } from "../stores/draftStore";
import TranslationModal from "../components/TranslationModal";
import InjectModal from "../components/InjectModal";
import PatchModal from "../components/PatchModal";
import PatchStatusIndicator from "../components/PatchStatusIndicator";
import ExportModal from "../components/ExportModal";
import SearchReplaceModal from "../components/SearchReplaceModal";
import ValidationResultsModal from "../components/ValidationResultsModal";
import FontPatchDialog from "../components/FontPatchDialog";
import WorkflowGuideBanner from "../components/WorkflowGuideBanner";
import EmptyState from "../components/EmptyState";
import PivotModal from "../components/PivotModal";
import { pivotCarryOverCount } from "../lib/pivot";
import {
	editorStatsKey,
	formatUsdCost,
} from "../lib/translationCost";
import { buildSettingsPath } from "../lib/settingsNav";
import { useT } from "../lib/i18n";

const BTN_BASE =
	"inline-flex h-8 items-center gap-1.5 rounded-md px-3 text-body font-medium transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-1 focus-visible:ring-offset-surface disabled:cursor-not-allowed disabled:opacity-50";
const BTN_PRIMARY = `${BTN_BASE} bg-accent text-white shadow-sm hover:bg-accent-hover`;
const BTN_SECONDARY = `${BTN_BASE} border border-border bg-surface text-text hover:bg-surface-muted`;
const BTN_GHOST = `${BTN_BASE} px-2.5 text-text-muted hover:bg-surface-muted hover:text-text`;

/** Slim stacked progress bar: approved / reviewed / translated over total. */
function EditorProgress({ stats }: { stats: ProjectStats }) {
	const t = useT();
	const total = stats.total || 0;
	const done = stats.translated + stats.reviewed + stats.approved;
	const percent = total > 0 ? Math.round((done / total) * 100) : 0;
	const width = (n: number) => `${total > 0 ? (n / total) * 100 : 0}%`;
	const label = t("editor.progressAria", { done, total, percent });
	return (
		<div className="flex items-center gap-2" title={label}>
			<div
				role="progressbar"
				aria-label={label}
				aria-valuemin={0}
				aria-valuemax={total}
				aria-valuenow={done}
				className="flex h-2 w-36 overflow-hidden rounded-full bg-border"
			>
				<span className="h-full bg-success" style={{ width: width(stats.approved) }} />
				<span className="h-full bg-warning" style={{ width: width(stats.reviewed) }} />
				<span className="h-full bg-blue-500 dark:bg-blue-400" style={{ width: width(stats.translated) }} />
			</div>
			<span className="text-caption font-semibold tabular-nums text-text">
				{t("editor.progressPercent", { percent })}
			</span>
		</div>
	);
}

export default function Editor() {
	const t = useT();
	const { filter, selectedEntryId, setSelected, isTranslating } = useEditorStore();
	const validationWorklist = useEditorStore((s) => s.validationWorklist);
	const validationWorklistIndex = useEditorStore((s) => s.validationWorklistIndex);
	const startValidationWorklist = useEditorStore((s) => s.startValidationWorklist);
	const clearValidationWorklist = useEditorStore((s) => s.clearValidationWorklist);
	const stepValidationWorklist = useEditorStore((s) => s.stepValidationWorklist);
	const { project } = useProjectStore();
	const projectKey = project ? draftProjectKey(project) : "";
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
	const [showPivotModal, setShowPivotModal] = useState(false);
	const [showReplaceModal, setShowReplaceModal] = useState(false);
	const [showValidationModal, setShowValidationModal] = useState(false);
	const [showFontPatch, setShowFontPatch] = useState(false);
	const [fontPatchSelection, setFontPatchSelection] = useState<{ path: string; gamePath: string } | null>(null);
	const [patchBackupId, setPatchBackupId] = useState<string | undefined>();
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
		queryKey: ["strings", filter, projectKey],
		queryFn: () => getStrings(filter),
		staleTime: 30_000,
		enabled: !!project,
	});

	const sampleSources = useMemo(
		() => stringsData?.entries.map((e) => e.source),
		[stringsData],
	);

	const { data: statsData } = useQuery({
		queryKey: ["stats", projectKey],
		queryFn: getStats,
		staleTime: 10_000,
		enabled: !!project,
	});

	const { data: selectedEntry } = useQuery({
		queryKey: ["string", selectedEntryId, projectKey],
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
					t("activity.validation.clean", { count: v.total_checked }),
					undefined,
					"validate",
				);
			} else {
				addLog("warning", t("activity.validation.issues", {
					count: v.issues_found,
					entries: t("activity.count.entries", { count: v.entries_with_issues }),
					kinds: kinds ? ` (${kinds})` : "",
				}), undefined, "validate");
			}
		} catch (e) {
			const err = e instanceof Error ? e.message : String(e);
			addToast("error", t("editor.toast.validateFailed", { error: err }));
			addLog("error", t("activity.validation.failed"), err, "validate");
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
		showPivotModal ||
		showReplaceModal ||
		showValidationModal || showFontPatch;

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
			else if (showPivotModal) setShowPivotModal(false);
			else if (showExportModal) setShowExportModal(false);
			else if (showPatchModal) setShowPatchModal(false);
			else if (showInjectModal) setShowInjectModal(false);
			else if (showTranslateModal) setShowTranslateModal(false);
			else if (selectedEntryId) setSelected(null);
		},
		!showFontPatch && (editorModalOpen || !!selectedEntryId),
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
			{project?.persistence_warning && <div role="alert" className="border-b border-warning bg-warning-muted px-4 py-2 text-body text-warning">
				<p>{t("editor.recentSaveFailed")}</p>
				<details><summary>{t("recovery.details")}</summary><p className="break-words">{project.persistence_warning}</p></details>
			</div>}
			{!!project?.extraction_warnings?.length && (
				<details className="border-b border-warning bg-warning-muted px-4 py-2 text-body text-warning">
					<summary className="cursor-pointer font-medium">{t("editor.partialExtraction")}</summary>
					<p className="mt-1">{t("editor.partialExtractionDetail")}</p>
					<ul className="mt-1 max-h-32 overflow-y-auto list-disc pl-5 break-words">
						{project.extraction_warnings.map((warning, index) => <li key={index}>{warning}</li>)}
					</ul>
				</details>
			)}
			{/* Top bar: project identity + progress, then grouped actions */}
			<header className="flex flex-col gap-2 border-b border-border bg-surface px-4 pb-2 pt-2.5">
				<div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1.5">
					<div className="flex min-w-0 flex-wrap items-center gap-2">
						<h1 className="min-w-0 break-words text-section font-semibold text-text">
							{project?.name || t("editor.noProject")}
						</h1>
						{project && (
							<span
								className="rounded bg-surface-muted px-1.5 py-0.5 font-mono text-caption text-text-muted"
								title={t("editor.formatTitle")}
							>
								{project.format_id}
							</span>
						)}
						<PatchStatusIndicator
							gamePath={project?.path}
							onOpenPatch={() => setShowPatchModal(true)}
							refreshKey={patchStatusRefreshKey}
						/>
						{hasProject && guideDismissed && workflowStep && (
							<button
								type="button"
								onClick={handleGuidePrimaryAction}
								title={t("editor.nextStepTitle")}
								className="inline-flex items-center gap-0.5 rounded-full border border-accent-fg px-2 py-0.5 text-caption font-medium text-accent-fg transition-colors hover:bg-accent-muted focus:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg"
							>
								{t("editor.next", { step: t(`workflow.${workflowStep}`) })}
								<ChevronRight size={12} aria-hidden="true" />
							</button>
						)}
					</div>

					{statsData && (
						<div className="ml-auto flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
							<EditorProgress stats={statsData} />
							<span className="text-caption text-text-muted">
								{t(editorStatsKey(statsData.total_cost_usd), {
									pending: statsData.pending,
									translated: statsData.translated,
									approved: statsData.approved,
									cost: formatUsdCost(statsData.total_cost_usd),
								})}
							</span>
							{hasProject && (
								<button
									type="button"
									onClick={() => navigate(buildSettingsPath("history"))}
									title={t("editor.viewHistoryTitle")}
									className="rounded-sm text-caption font-medium text-accent-fg underline-offset-2 hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg"
								>
									{t("editor.viewHistory")}
								</button>
							)}
						</div>
					)}
				</div>

				<div
					role="toolbar"
					aria-label={t("editor.toolbarAria")}
					className="flex flex-wrap items-center gap-x-4 gap-y-2"
				>
					{/* Primary workflow actions */}
					<div role="group" aria-label={t("editor.workflowActions")} className="flex flex-wrap items-center gap-1.5">
						<button
							onClick={() => setShowTranslateModal(true)}
							disabled={!hasProject}
							className={BTN_PRIMARY}
							title={hasProject ? "Ctrl+T" : t("editor.hotkeyOrProject")}
						>
							<Languages size={15} aria-hidden="true" /> {t("editor.translate")}
						</button>

						<button
							onClick={() => {
								void handleValidate();
							}}
							disabled={validating || !hasProject}
							className={BTN_SECONDARY}
							title={hasProject ? "Ctrl+Shift+V" : t("editor.hotkeyOrProject")}
						>
							{validating ? (
								<Loader2 size={15} className="animate-spin" aria-hidden="true" />
							) : (
								<Shield size={15} aria-hidden="true" />
							)}
							{t("editor.validate")}
						</button>

						<button
							onClick={() => setShowInjectModal(true)}
							disabled={!hasProject}
							className={BTN_SECONDARY}
							title={hasProject ? "Ctrl+I" : t("editor.hotkeyOrProject")}
						>
							<FileCheck size={15} aria-hidden="true" /> {t("editor.inject")}
						</button>

						<button
							onClick={() => setShowPatchModal(true)}
							disabled={!hasProject}
							className={BTN_SECONDARY}
							title={hasProject ? "Ctrl+Shift+P" : t("editor.hotkeyOrProject")}
						>
							<Package size={15} aria-hidden="true" /> {t("editor.patch")}
						</button>
					</div>

					{/* Secondary project tools */}
					<div role="group" aria-label={t("editor.moreTools")} className="flex flex-wrap items-center gap-0.5 sm:ml-auto">
						<button
							onClick={() => setShowExportModal(true)}
							disabled={!hasProject}
							className={BTN_GHOST}
							title={hasProject ? "Ctrl+E" : t("editor.hotkeyOrProject")}
						>
							<Download size={15} aria-hidden="true" /> {t("editor.export")}
						</button>

						<button
							onClick={() => setShowReplaceModal(true)}
							disabled={!hasProject}
							className={BTN_GHOST}
							title={hasProject ? "Ctrl+Shift+F" : t("editor.hotkeyOrProject")}
						>
							<Replace size={15} aria-hidden="true" /> {t("editor.replace")}
						</button>

						<button
							type="button"
							onClick={() => setShowPivotModal(true)}
							disabled={!hasProject}
							className={BTN_GHOST}
							title={t("pivot.title")}
						>
							<GitBranch size={15} aria-hidden="true" /> {t("editor.pivot")}
						</button>
					</div>
				</div>
			</header>

			{validationWorklist && validationWorklist.length > 0 && (
				<div className="flex items-center gap-3 px-4 py-1.5 border-b border-warning bg-warning-muted text-warning">
					<span className="text-caption font-medium">
						{t("validate.worklist", {
							current: validationWorklistIndex + 1,
							total: validationWorklist.length,
						})}
					</span>
					<div className="flex items-center gap-1.5 ml-auto">
						<button
							type="button"
							onClick={() => stepValidationWorklist(-1)}
							disabled={validationWorklistIndex <= 0}
							className="px-2 py-0.5 text-caption font-medium rounded border border-warning disabled:opacity-40"
						>
							{t("validate.worklistPrev")}
						</button>
						<button
							type="button"
							onClick={() => stepValidationWorklist(1)}
							disabled={
								validationWorklistIndex >= validationWorklist.length - 1
							}
							className="px-2 py-0.5 text-caption font-medium rounded border border-warning disabled:opacity-40"
						>
							{t("validate.worklistNext")}
						</button>
						<button
							type="button"
							onClick={clearValidationWorklist}
							className="px-2 py-0.5 text-caption font-medium rounded border border-warning"
						>
							{t("validate.worklistDone")}
						</button>
					</div>
				</div>
			)}

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
				<FilterBar total={total} showing={entries.length} />
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
					<div className="flex flex-1 items-center justify-center text-text-muted">
						{t("editor.loadingStrings")}
					</div>
				) : stringsError ? (
					<div className="flex flex-1 flex-col items-center justify-center gap-3 px-6 text-center">
						<p className="font-medium text-danger">{t("editor.loadError")}</p>
						<p className="text-body text-text-muted">
							{stringsErrorDetail instanceof Error
								? stringsErrorDetail.message
								: t("common.tryAgain")}
						</p>
						<button
							onClick={() => {
								void refetch();
							}}
							className="rounded bg-accent px-4 py-2 text-body font-medium text-white hover:bg-accent-hover"
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
						key={draftEntryKey(projectKey, selectedEntry.id)}
						projectKey={projectKey}
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
				sampleSources={sampleSources}
			/>

			{/* Inject Modal */}
			<InjectModal
				open={showInjectModal}
				onClose={() => setShowInjectModal(false)}
				onOpenPack={(backupId) => {
					setPatchBackupId(backupId);
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
					setFontPatchSelection(null);
					setPatchBackupId(undefined);
				}}
				defaultGamePath={fontPatchSelection?.gamePath ?? project?.path}
				initialZipPath={fontPatchSelection?.path}
				initialBackupId={patchBackupId}
				initialTab={patchInitialTab}
				onPatchStateChanged={bumpPatchStatus}
			/>

			{/* PO / XLIFF export + import */}
			<ExportModal
				open={showExportModal}
				onClose={() => setShowExportModal(false)}
				onImported={handleRefetch}
			/>

			<PivotModal
				open={showPivotModal}
				onClose={() => setShowPivotModal(false)}
				carryOverCount={
					statsData ? pivotCarryOverCount(statsData) : null
				}
			/>

			<SearchReplaceModal
				open={showReplaceModal}
				onClose={() => setShowReplaceModal(false)}
				onDone={handleRefetch}
			/>

			<ValidationResultsModal
				open={showValidationModal}
				result={validationResult}
				onFontPatch={project && ["html-game", "rpgmaker-mv", "rpgmaker-mz", "renpy"].includes(project.format_id) ? () => { setShowValidationModal(false); setShowFontPatch(true); } : undefined}
				onClose={() => setShowValidationModal(false)}
				onSelectEntry={(entryId) => {
					setShowValidationModal(false);
					setSelected(entryId);
				}}
				onReviewInEditor={(entryIds) => {
					setShowValidationModal(false);
					startValidationWorklist(entryIds);
				}}
			/>
			<FontPatchDialog open={showFontPatch} defaultGamePath={project?.path ?? ""} onClose={() => setShowFontPatch(false)} onUsePatch={(path, gamePath) => { setShowFontPatch(false); setFontPatchSelection({ path, gamePath }); setPatchInitialTab("apply"); setShowPatchModal(true); }} />
		</div>
	);
}
