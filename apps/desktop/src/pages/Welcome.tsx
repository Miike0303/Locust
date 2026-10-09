import { IS_TAURI } from "../lib/runtime";
import { useState, useEffect } from "react";
import { useNavigate, useLocation } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
	FolderOpen,
	File,
	Globe,
	Swords,
	Heart,
	Box,
	Shield,
	Code,
	Clock,
	X,
	Plus,
	Wand2,
	Loader,
	Settings2,
	Languages,
	FileCheck,
	Package,
	BookOpen,
	Sparkles,
	Terminal,
	Clapperboard,
	Puzzle,
	Braces,
	Database,
	RotateCcw,
} from "lucide-react";
import { getFormats, getConfig, getProviders, getInjectionStatus } from "../lib/api";
import { useProjectStore } from "../stores/projectStore";
import { useQueueStore } from "../stores/queueStore";
import { addLog } from "../stores/logStore";
import { addToast } from "../stores/toastStore";
import PatchModal from "../components/PatchModal";
import InjectionRecoveryModal from "../components/InjectionRecoveryModal";
import {
	completeOpenProject,
	completeOpenProjectDb,
	formatPickerPathFromState,
	isDetectionFailure,
	openDbCanConfirm,
	pickGameFolder,
	pickLocustDbFile,
	runOpenAction,
	shouldOpenProjectDb,
} from "../lib/openProjectFlow";
import {
	PENDING_AFTER_STALE_FILTER,
	projectOpenMergeNotice,
	shouldFocusPendingAfterOpen,
} from "../lib/projectOpenMerge";
import { useEditorStore } from "../stores/editorStore";
import {
	useModalA11y,
	MODAL_BACKDROP_CLASS,
} from "../lib/modalA11y";
import {
	hasAnyReadyProvider,
	readProviderSetupHintDismissed,
	saveProviderSetupHintDismissed,
} from "../lib/providerReadiness";
import { buildSettingsPath } from "../lib/settingsNav";
import {
	readWelcomeGuideDismissed,
	saveWelcomeGuideDismissed,
	WELCOME_WORKFLOW_STEPS,
} from "../lib/workflowGuide";
import { useT } from "../lib/i18n";
import { formatDescriptionKey } from "../lib/formatDescriptions";

const FORMAT_ICONS: Record<string, typeof Globe> = {
	"rpgmaker-mv": Swords,
	"rpgmaker-vxa": Swords,
	renpy: Heart,
	unity: Box,
	"wolf-rpg": Shield,
	sugarcube: Globe,
	"html-game": Code,
	unreal: Box,
	kirikiri: BookOpen,
	yuris: Sparkles,
	nscripter: Terminal,
	tyrano: Clapperboard,
	qsp: Puzzle,
	vntextpatch: Braces,
};

const FORMAT_COLORS: Record<string, string> = {
	"rpgmaker-mv":
		"bg-blue-100 text-blue-700 dark:bg-blue-900/40 dark:text-blue-300",
	"rpgmaker-vxa":
		"bg-indigo-100 text-indigo-700 dark:bg-indigo-900/40 dark:text-indigo-300",
	renpy: "bg-pink-100 text-pink-700 dark:bg-pink-900/40 dark:text-pink-300",
	unity:
		"bg-purple-100 text-purple-700 dark:bg-purple-900/40 dark:text-purple-300",
	"wolf-rpg":
		"bg-orange-100 text-orange-700 dark:bg-orange-900/40 dark:text-orange-300",
	sugarcube: "bg-accent-muted text-accent-fg",
	"html-game":
		"bg-cyan-100 text-cyan-700 dark:bg-cyan-900/40 dark:text-cyan-300",
	unreal: "bg-danger-muted text-danger",
	kirikiri: "bg-warning-muted text-warning",
	yuris:
		"bg-violet-100 text-violet-700 dark:bg-violet-900/40 dark:text-violet-300",
	nscripter: "bg-surface-muted text-text-muted",
	tyrano: "bg-rose-100 text-rose-700 dark:bg-rose-900/40 dark:text-rose-300",
	qsp: "bg-teal-100 text-teal-700 dark:bg-teal-900/40 dark:text-teal-300",
	vntextpatch:
		"bg-lime-100 text-lime-800 dark:bg-lime-900/40 dark:text-lime-300",
};

export default function Welcome() {
	const t = useT();
	const formatDescriptionText = (id: string, fallback?: string) => {
		const key = formatDescriptionKey(id);
		return key ? t(key) : fallback;
	};
	const navigate = useNavigate();
	const location = useLocation();
	const queryClient = useQueryClient();
	const setProject = useProjectStore((s) => s.setProject);
	const setFilter = useEditorStore((s) => s.setFilter);
	const { data: formats } = useQuery({
		queryKey: ["formats"],
		queryFn: getFormats,
	});
	const { data: config } = useQuery({
		queryKey: ["config"],
		queryFn: getConfig,
	});
	const { data: providers } = useQuery({
		queryKey: ["providers"],
		queryFn: getProviders,
	});

	const project = useProjectStore((s) => s.project);

	const [hintDismissed, setHintDismissed] = useState(() =>
		readProviderSetupHintDismissed(),
	);
	const [welcomeGuideDismissed, setWelcomeGuideDismissed] = useState(() =>
		readWelcomeGuideDismissed(),
	);

	const addToQueue = useQueueStore((s) => s.addItem);
	const setQueueOpen = useQueueStore((s) => s.setPanelOpen);

	// Format picker — extract path, or open-db draft (CLI / external .locust.db).
	const [picker, setPicker] = useState<{
		path: string | null;
		reason: "manual" | "detect-failed";
	} | null>(null);
	const [openDbDraft, setOpenDbDraft] = useState<{
		databasePath: string;
		gamePath: string;
	} | null>(null);
	const [selectedFormat, setSelectedFormat] = useState("auto");
	const [opening, setOpening] = useState(false);
	const [showPatchModal, setShowPatchModal] = useState(false);
	const [recoveryPath, setRecoveryPath] = useState<string | null>(null);
	const formatOverlayOpen = !!picker || !!openDbDraft;
	const { dialogRef, dialogProps, titleProps } = useModalA11y({
		open: formatOverlayOpen,
		onClose: () => {
			setPicker(null);
			setOpenDbDraft(null);
		},
		ownEscape: true,
	});

	useEffect(() => {
		const pendingPath = formatPickerPathFromState(location.state);
		if (!pendingPath) return;
		setSelectedFormat("");
		setPicker({ path: pendingPath, reason: "detect-failed" });
		navigate(".", { replace: true, state: {} });
	}, [location.state, navigate]);

	const offerRecovery = async (path: string) => {
		try {
			const status = await getInjectionStatus(path);
			if (status.pending) {
				setPicker(null);
				setOpenDbDraft(null);
				setRecoveryPath(path);
				addToast("warning", t("recovery.pending"));
				return true;
			}
		} catch { /* Keep the original open error; manual recovery remains available. */ }
		return false;
	};

	const openWithPath = async (path: string, formatId?: string, preferSaved = false) => {
		setOpening(true);
		try {
			const result = await completeOpenProject(path, formatId, {
				setProject,
				queryClient,
			}, preferSaved);
			if (!result) return;
			const notice = projectOpenMergeNotice(result, t);
			addLog(
				notice.toast ? "warning" : "info",
				notice.logMessage,
				undefined,
				"project",
			);
			if (notice.toastMessage) {
				addToast("warning", notice.toastMessage);
			}
			if (shouldFocusPendingAfterOpen(result)) {
				setFilter(PENDING_AFTER_STALE_FILTER);
			}
			setPicker(null);
			navigate("/editor");
		} catch (err: any) {
			const msg = err?.message ?? String(err);
			if (!formatId && isDetectionFailure(msg)) {
				// Auto-detect failed — let the user pick the engine instead of toasting.
				addLog("warning", t("activity.project.detectFailed"), path, "project");
				setSelectedFormat("");
				setPicker({ path, reason: "detect-failed" });
			} else {
				addLog("error", t("activity.project.openFailed"), msg, "project");
				if (!await offerRecovery(path)) addToast("error", t("welcome.toast.failedOpen", { error: msg }));
			}
		} finally {
			setOpening(false);
		}
	};

	/** Reopen a saved .locust.db (pivot / open-db recent) without extracting the game. */
	const openWithDb = async (
		databasePath: string,
		gamePath: string,
		formatId: string,
	) => {
		setOpening(true);
		try {
			const result = await completeOpenProjectDb(databasePath, gamePath, formatId, {
				setProject,
				queryClient,
			});
			addLog(
				"info",
				t("activity.project.openedDb", { name: result.project_name, db: databasePath, count: result.total_strings }),
				undefined,
				"project",
			);
			addToast("success", t("welcome.toast.openedDb", { name: result.project_name }));
			setPicker(null);
			setOpenDbDraft(null);
			navigate("/editor");
		} catch (err: unknown) {
			const msg = err instanceof Error ? err.message : String(err);
			addLog("error", t("activity.project.openDbFailed"), msg, "project");
			if (!await offerRecovery(gamePath)) addToast("error", t("welcome.toast.failedOpen", { error: msg }));
		} finally {
			setOpening(false);
		}
	};

	const openRecent = async (p: {
		path: string;
		format_id: string;
		database_path?: string | null;
	}) => {
		if (shouldOpenProjectDb(p.database_path)) {
			await openWithDb(p.database_path!.trim(), p.path, p.format_id);
			return;
		}
		await openWithPath(p.path, p.format_id, true);
	};

	const pickFolderPath = () => pickGameFolder(t);

	/** A rejected native dialog must toast, not vanish as an unhandled rejection. */
	const reportDialogFailure = (message: string) => {
		addLog("error", t("activity.project.openFailed"), message, "project");
		addToast("error", t("welcome.toast.failedOpen", { error: message }));
	};

	const handleConfirmFormat = async () => {
		if (openDbDraft) {
			if (!openDbCanConfirm(openDbDraft.databasePath, openDbDraft.gamePath, selectedFormat)) {
				return;
			}
			await openWithDb(
				openDbDraft.databasePath,
				openDbDraft.gamePath,
				selectedFormat,
			);
			return;
		}
		if (!picker) return;
		const formatId = selectedFormat === "auto" ? undefined : selectedFormat;
		if (picker.path) {
			await openWithPath(picker.path, formatId);
			return;
		}
		// Manual mode: format chosen first, now pick the game folder.
		await runOpenAction(async () => {
			const path = await pickFolderPath();
			if (path) await openWithPath(path, formatId);
		}, reportDialogFailure);
	};

	const handleOpenProjectDb = () => runOpenAction(async () => {
		const picked = await pickLocustDbFile(t);
		if (picked.status === "cancelled") return;
		if (picked.status === "invalid") {
			addToast("info", t("welcome.toast.needLocustDb"));
			return;
		}
		const gamePath = await pickFolderPath();
		if (!gamePath) {
			addToast("info", t("welcome.toast.cancelledOpenDb"));
			return;
		}
		setPicker(null);
		setSelectedFormat("");
		setOpenDbDraft({ databasePath: picked.path, gamePath });
	}, reportDialogFailure);

	const handleAddToQueue = (path: string) => {
		addToQueue(path);
		setQueueOpen(true);
		addToast("info", t("welcome.toast.addedToQueue"));
	};

	const handleOpenFile = () => runOpenAction(async () => {
		let path: string | null = null;
		if (IS_TAURI) {
			const { open } = await import("@tauri-apps/plugin-dialog");
			const selected = await open({
				title: t("welcome.dialog.selectFile"),
				filters: [
					{
						name: t("welcome.dialog.gameFiles"),
						extensions: [
							"exe",
							"html",
							"htm",
							"rpy",
							"rpa",
							"rpgproject",
							"rvproj2",
						],
					},
					{ name: t("welcome.dialog.allFiles"), extensions: ["*"] },
				],
			});
			if (typeof selected === "string") path = selected;
		} else {
			path = prompt(t("welcome.prompt.filePath"));
		}
		if (path) await openWithPath(path);
	}, reportDialogFailure);

	const handleOpenFolder = () => runOpenAction(async () => {
		const path = await pickFolderPath();
		if (path) await openWithPath(path);
	}, reportDialogFailure);

	const handleChooseFormatManually = () => {
		setSelectedFormat("auto");
		setPicker({ path: null, reason: "manual" });
	};

	const recentProjects = config?.recent_projects ?? [];
	const showWelcomeGuide =
		!welcomeGuideDismissed && recentProjects.length === 0 && !project;
	const showProviderHint =
		providers !== undefined &&
		!hasAnyReadyProvider(providers, config) &&
		!hintDismissed;

	const dismissWelcomeGuide = () => {
		setWelcomeGuideDismissed(true);
		saveWelcomeGuideDismissed(true);
	};

	const dismissProviderHint = () => {
		setHintDismissed(true);
		saveProviderSetupHintDismissed(true);
	};

	return (
		<div className="flex flex-col min-h-full p-8 max-w-4xl mx-auto text-text text-body">
			{/* Hero */}
			<div className="text-center mb-8">
				<Globe size={48} className="mx-auto mb-3 text-accent-fg" />
				<h1 className="text-page font-bold mb-1">{t("nav.appName")}</h1>
				<p className="text-section text-text-muted">
					{t("welcome.tagline")}
				</p>
			</div>

			{showWelcomeGuide && (
				<section
					aria-label={t("welcome.guide.aria")}
					className="mb-8 rounded-lg border border-border bg-surface p-4"
				>
					<div className="flex items-start justify-between gap-3 mb-3">
						<div>
							<h2 className="text-body font-semibold text-accent-fg">
								{t("welcome.guide.title")}
							</h2>
							<p className="text-caption text-text-muted mt-0.5">
								{t("welcome.guide.subtitle")}
							</p>
						</div>
						<button
							type="button"
							onClick={dismissWelcomeGuide}
							aria-label={t("welcome.guide.dismiss")}
							className="shrink-0 text-accent-fg hover:text-text focus:outline-none focus:ring-2 focus:ring-accent-fg rounded p-0.5"
						>
							<X size={16} />
						</button>
					</div>
					<ol className="grid gap-2 sm:grid-cols-3">
						{WELCOME_WORKFLOW_STEPS.map((step, index) => {
							const StepIcon =
								step.id === "open"
									? FolderOpen
									: step.id === "translate"
										? Languages
										: FileCheck;
							return (
								<li
									key={step.id}
									className="flex items-start gap-2 rounded-md border border-border bg-surface p-3"
								>
									<span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-accent text-caption font-bold text-white">
										{index + 1}
									</span>
									<div className="min-w-0">
										<div className="flex items-center gap-1.5 text-body font-medium text-text">
											<StepIcon
												size={14}
												className="text-accent-fg"
												aria-hidden="true"
											/>
											{t(step.labelKey)}
										</div>
										<p className="text-caption text-text-muted mt-0.5">
											{t(step.descriptionKey)}
										</p>
									</div>
								</li>
							);
						})}
					</ol>
				</section>
			)}

			{/* Open buttons */}
			<div className="mb-10">
				<div className="flex justify-center gap-4 flex-wrap">
					<button
						onClick={handleOpenFolder}
						disabled={opening}
						className="flex items-center gap-2 px-6 py-3 bg-accent hover:bg-accent-hover disabled:opacity-50 disabled:cursor-not-allowed text-white rounded-lg text-body font-medium transition-colors"
					>
						{opening ? (
							<Loader size={18} className="animate-spin" />
						) : (
							<FolderOpen size={18} />
						)}
						{opening ? t("welcome.opening") : t("welcome.openFolder")}
					</button>
					<button
						onClick={handleOpenFile}
						disabled={opening}
						className="flex items-center gap-2 px-6 py-3 border border-border bg-surface hover:bg-surface-muted disabled:opacity-50 disabled:cursor-not-allowed rounded-lg text-body font-medium transition-colors"
					>
						{opening ? (
							<Loader size={18} className="animate-spin" />
						) : (
							<File size={18} />
						)}
						{opening ? t("welcome.opening") : t("welcome.openFile")}
					</button>
					<button
						type="button"
						onClick={handleOpenProjectDb}
						disabled={opening}
						className="flex items-center gap-2 px-6 py-3 border border-border bg-surface hover:bg-surface-muted disabled:opacity-50 disabled:cursor-not-allowed rounded-lg text-body font-medium transition-colors"
					>
						{opening ? (
							<Loader size={18} className="animate-spin" />
						) : (
							<Database size={18} />
						)}
						{opening ? t("welcome.opening") : t("welcome.openDb")}
					</button>
					<button
						type="button"
						onClick={() => setShowPatchModal(true)}
						disabled={opening}
						className="flex items-center gap-2 px-6 py-3 border border-border bg-surface hover:bg-surface-muted disabled:opacity-50 disabled:cursor-not-allowed rounded-lg text-body font-medium transition-colors"
					>
						<Package size={18} />
						{t("welcome.applyPatch")}
					</button>
					<button type="button" onClick={() => setRecoveryPath(project?.path ?? "")} disabled={opening}
						className="flex items-center gap-2 px-6 py-3 border border-border bg-surface hover:bg-surface-muted disabled:opacity-50 rounded-lg text-body font-medium">
						<RotateCcw size={18} />{t("recovery.action")}
					</button>
				</div>
				<div className="flex justify-center mt-2">
					<button
						onClick={handleChooseFormatManually}
						disabled={opening}
						className="flex items-center gap-1 text-caption text-text-muted hover:text-text disabled:opacity-50"
					>
						<Settings2 size={12} />
						{t("welcome.chooseFormat")}
					</button>
				</div>
				{opening && (
					<p className="text-center text-caption text-text-muted mt-2">
						{t("welcome.extracting")}
					</p>
				)}
			</div>

			{showProviderHint && (
				<div className="mb-8 p-4 rounded-lg border border-border bg-warning-muted">
					<div className="flex items-start justify-between gap-3">
						<p className="text-body text-warning">
							{t("welcome.providerHint")}{" "}
							<button
								type="button"
								onClick={() => navigate(buildSettingsPath("providers"))}
								className="font-medium text-accent-fg hover:underline"
							>
								{t("welcome.providerHintLink")}
							</button>
							.
						</p>
						<button
							type="button"
							onClick={dismissProviderHint}
							aria-label={t("welcome.providerHintDismiss")}
							className="shrink-0 text-warning hover:text-text"
						>
							<X size={16} />
						</button>
					</div>
				</div>
			)}

			{/* Format Picker Modal — extract detect/manual, or open-db after picking .locust.db + game */}
			{(picker || openDbDraft) && (
				<div className={MODAL_BACKDROP_CLASS}>
					<div
						ref={dialogRef}
						{...dialogProps}
						className="bg-surface text-text border border-border rounded-lg shadow-xl w-full max-w-md p-6"
					>
						<div className="flex justify-between items-center mb-4">
							<h2 {...titleProps} className="text-section font-bold">
								{t("welcome.format.title")}
							</h2>
							<button
								onClick={() => {
									setPicker(null);
									setOpenDbDraft(null);
								}}
								className="text-text-muted hover:text-text"
							>
								<X size={20} />
							</button>
						</div>

						{openDbDraft ? (
							<>
								<p className="text-body text-text-muted mb-1 truncate" title={openDbDraft.databasePath}>
									{openDbDraft.databasePath}
								</p>
								<p className="text-caption text-text-muted mb-1 truncate" title={openDbDraft.gamePath}>
									{t("welcome.recentGame", { path: openDbDraft.gamePath })}
								</p>
								<p className="text-caption text-text-muted mb-4">
									{t("welcome.format.openDbHint")}
								</p>
							</>
						) : (
							<>
								{picker?.path && (
									<p className="text-body text-text-muted mb-1 truncate">
										{picker.path}
									</p>
								)}
								{picker?.reason === "detect-failed" ? (
									<p className="text-caption text-warning mb-4">
										{t("welcome.format.detectFailed")}
									</p>
								) : (
									<p className="text-caption text-text-muted mb-4">
										{t("welcome.format.manualHint")}
									</p>
								)}
							</>
						)}

						<div className="space-y-1.5 max-h-64 overflow-y-auto mb-4">
							{!openDbDraft && picker?.reason !== "detect-failed" && (
								<button
									onClick={() => setSelectedFormat("auto")}
									className={`w-full text-left p-3 rounded-lg border transition-colors flex items-center gap-3 ${
										selectedFormat === "auto"
											? "border-accent-fg bg-accent-muted"
											: "border-border bg-surface hover:bg-surface-muted"
									}`}
								>
									<div className="p-1.5 rounded bg-accent-muted text-accent-fg">
										<Wand2 size={16} />
									</div>
									<div>
										<div className="text-body font-medium">{t("welcome.format.auto")}</div>
										<div className="text-caption text-text-muted">
											{t("welcome.format.autoHint")}
										</div>
									</div>
								</button>
							)}

							{formats
								?.filter((f) => f.stability !== "comingsoon")
								.map((f) => {
									const Icon = FORMAT_ICONS[f.id] ?? Globe;
									const colorClass =
										FORMAT_COLORS[f.id] ??
										"bg-surface-muted text-text-muted";
									const experimental = f.stability === "experimental";
									return (
										<button
											key={f.id}
											onClick={() => setSelectedFormat(f.id)}
											className={`w-full text-left p-3 rounded-lg border transition-colors flex items-center gap-3 ${
												selectedFormat === f.id
													? "border-accent-fg bg-accent-muted"
													: "border-border bg-surface hover:bg-surface-muted"
											}`}
										>
											<div className={`p-1.5 rounded ${colorClass}`}>
												<Icon size={16} />
											</div>
											<div className="min-w-0 flex-1">
												<div className="text-body font-medium flex items-center gap-2">
													<span className="truncate">{f.name}</span>
													{experimental && (
														<span className="shrink-0 text-caption uppercase tracking-wide px-1.5 py-0.5 rounded bg-warning-muted text-warning">
															{t("welcome.format.experimental")}
														</span>
													)}
												</div>
												<div className="text-caption text-text-muted">
													{f.extensions.join(", ")}
												</div>
											</div>
										</button>
									);
								})}
						</div>

						<button
							onClick={handleConfirmFormat}
							disabled={
								opening ||
								!selectedFormat ||
								(!!openDbDraft &&
									!openDbCanConfirm(
										openDbDraft.databasePath,
										openDbDraft.gamePath,
										selectedFormat,
									))
							}
							className="w-full py-2.5 bg-accent hover:bg-accent-hover disabled:opacity-50 text-white rounded-lg text-section font-medium transition-colors"
						>
							{opening
								? t("welcome.openingDots")
								: openDbDraft
									? t("welcome.format.openDbConfirm")
									: picker?.path
										? t("welcome.format.openProject")
										: t("welcome.format.chooseFolder")}
						</button>
					</div>
				</div>
			)}

			{/* Recent Projects */}
			{recentProjects.length > 0 && (
				<div className="mb-10">
					<h2 className="text-body font-semibold text-text-muted uppercase mb-3">
						{t("welcome.recent")}
					</h2>
					<div className="space-y-2">
						{recentProjects.map((p, i) => {
							const Icon = FORMAT_ICONS[p.format_id] ?? Globe;
							const colorClass =
								FORMAT_COLORS[p.format_id] ??
								"bg-surface-muted text-text-muted";
							const isDbRecent = shouldOpenProjectDb(p.database_path);
							return (
								<div
									key={i}
									className="w-full p-3 rounded-lg border border-border bg-surface hover:border-accent-fg hover:bg-surface-muted transition-colors flex items-center gap-3"
								>
									<button
										onClick={() => openRecent(p)}
										disabled={opening}
										className="flex items-center gap-3 flex-1 min-w-0 text-left disabled:opacity-50 disabled:cursor-not-allowed"
									>
										<div className={`p-2 rounded-lg ${colorClass}`}>
											<Icon size={18} />
										</div>
										<div className="flex-1 min-w-0">
											<div className="text-section font-medium truncate flex items-center gap-2">
												<span className="truncate">{p.name}</span>
												{isDbRecent && (
													<span className="shrink-0 text-caption uppercase tracking-wide px-1.5 py-0.5 rounded bg-violet-100 text-violet-800 dark:bg-violet-950 dark:text-violet-200">
														{t("welcome.recentDbBadge")}
													</span>
												)}
											</div>
											<div className="text-caption text-text-muted truncate">
												{isDbRecent ? p.database_path : p.path}
											</div>
											{isDbRecent && (
												<div className="text-caption text-text-muted truncate">
													{t("welcome.recentGame", { path: p.path })}
												</div>
											)}
										</div>
									</button>
									<div className="flex items-center gap-2 text-caption text-text-muted shrink-0">
										<span
											className={`px-2 py-0.5 rounded-full text-caption ${colorClass}`}
										>
											{p.format_id}
										</span>
										{p.last_opened && (
											<span className="flex items-center gap-1">
												<Clock size={12} />
												{new Date(p.last_opened).toLocaleDateString()}
											</span>
										)}
										<button
											onClick={(e) => {
												e.stopPropagation();
												handleAddToQueue(p.path);
											}}
											className="p-1 rounded hover:bg-accent-muted text-text-muted hover:text-accent-fg"
											title={t("welcome.addToQueue")}
										>
											<Plus size={14} />
										</button>
									</div>
								</div>
							);
						})}
					</div>
				</div>
			)}

			{formats && formats.length > 0 && (
				<div>
					<h2 className="text-body font-semibold text-text-muted uppercase mb-3">
						{t("welcome.availableFormats")}
					</h2>
					<div className="grid grid-cols-[repeat(auto-fit,minmax(min(100%,14rem),1fr))] gap-3">
						{formats
							.filter((f) => f.stability !== "comingsoon")
							.map((f) => {
								const Icon = FORMAT_ICONS[f.id] ?? Globe;
								const colorClass =
									FORMAT_COLORS[f.id] ??
									"bg-surface-muted text-text-muted";
								const experimental = f.stability === "experimental";
								return (
									<div
										key={f.id}
										className="p-3 rounded-lg border border-border bg-surface"
									>
										<div className="flex items-start gap-2 mb-2">
											<div className={`p-1.5 rounded shrink-0 ${colorClass}`}>
												<Icon size={14} />
											</div>
											<div className="min-w-0 flex-1">
											<span className="block text-body font-medium break-words">
												{f.name}
											</span>
											{experimental && (
												<span className="inline-block mt-1 text-caption uppercase tracking-wide px-1.5 py-0.5 rounded bg-warning-muted text-warning">
													{t("welcome.format.experimental")}
												</span>
											)}
											</div>
										</div>
										{(formatDescriptionKey(f.id) || f.description) && (
											<p className="text-caption text-text-muted line-clamp-2">
												{formatDescriptionText(f.id, f.description)}
											</p>
										)}
										<div className="mt-1.5 flex flex-wrap gap-1">
											{f.extensions.slice(0, 3).map((ext) => (
												<span
													key={ext}
													className="px-1.5 py-0.5 bg-surface-muted rounded text-caption text-text-muted"
												>
													{ext}
												</span>
											))}
										</div>
									</div>
								);
							})}
					</div>
				</div>
			)}

			{/* Footer stats */}
			<div className="mt-auto pt-8 flex justify-center gap-6 text-caption text-text-muted">
				<span>
					{t("welcome.formatsAvailable", {
						count: formats?.filter((f) => f.stability !== "comingsoon").length ?? 0,
					})}
				</span>
				<span>{t("welcome.recentCount", { count: recentProjects.length })}</span>
				<span>
					<a
						href="https://github.com/Miike0303/Locust"
						className="hover:underline"
					>
						GitHub
					</a>
				</span>
			</div>

			<PatchModal
				open={showPatchModal}
				onClose={() => setShowPatchModal(false)}
				allowPack={false}
			/>
			{recoveryPath !== null && <InjectionRecoveryModal defaultGamePath={recoveryPath} onClose={() => setRecoveryPath(null)} />}
		</div>
	);
}
