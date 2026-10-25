import { lazy, Suspense, useState, useEffect } from "react";
import { useNavigate, useLocation } from "react-router-dom";
import { skipToken, useQuery, useQueryClient } from "@tanstack/react-query";
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
import { getFormats, getConfig, getProviders, getInjectionStatus, type AppConfig, type ProjectStats } from "../lib/api";
import { draftProjectKey } from "../lib/projectScope";
import { statusStyles } from "../lib/statusStyles";
import { APP_VERSION } from "../lib/appVersion";
import { useProjectStore } from "../stores/projectStore";
import { useQueueStore } from "../stores/queueStore";
import { addLog } from "../stores/logStore";
import { addToast } from "../stores/toastStore";
import {
	completeOpenProject,
	completeOpenProjectDb,
	formatPickerPathFromState,
	isDetectionFailure,
	openDbCanConfirm,
	pickGameFolder,
	pickGameSource,
	pickGameFile,
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
import { useLocale, useT } from "../lib/i18n";
import { formatDescriptionKey } from "../lib/formatDescriptions";

const PatchModal = lazy(() => import("../components/PatchModal"));
const InjectionRecoveryModal = lazy(() => import("../components/InjectionRecoveryModal"));

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

const FORMAT_COLOR = "bg-surface-muted text-text-muted";

type RecentProject = AppConfig["recent_projects"][number];

function RecentProjectRow({ project, opening, onOpen, onQueue }: {
	project: RecentProject;
	opening: boolean;
	onOpen: () => void;
	onQueue: () => void;
}) {
	const t = useT();
	const { locale } = useLocale();
	// Passive observer: never open a project or fetch stats to populate Home.
	const { data: stats } = useQuery<ProjectStats>({
		queryKey: ["stats", draftProjectKey({ ...project, database_path: project.database_path ?? undefined })],
		queryFn: skipToken,
		enabled: false,
	});
	const percent = stats && stats.total > 0
		? Math.min(100, Math.max(0, Math.round((stats.translated + stats.reviewed + stats.approved) / stats.total * 100)))
		: undefined;
	const opened = Date.parse(project.last_opened);
	const seconds = (opened - Date.now()) / 1000;
	const unit = Math.abs(seconds) < 3600 ? "minute" : Math.abs(seconds) < 86400 ? "hour"
		: Math.abs(seconds) < 2592000 ? "day" : Math.abs(seconds) < 31536000 ? "month" : "year";
	const divisor = { minute: 60, hour: 3600, day: 86400, month: 2592000, year: 31536000 }[unit];
	return (
		<div className="flex items-center gap-2 border-t border-border first:border-t-0">
			<button type="button" data-recent-project={project.path} onClick={onOpen} disabled={opening}
				className="min-w-0 flex-1 rounded-md px-4 py-3 text-left hover:bg-surface-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-50 disabled:cursor-not-allowed [@media(max-height:760px)]:py-1.5">
				<span className="flex flex-wrap items-center gap-x-2 gap-y-1">
					<span className="min-w-0 truncate font-bold">{project.name}</span>
					<span className="rounded bg-surface-muted px-1.5 py-0.5 font-mono text-caption text-text-muted" title={t("editor.formatTitle")}>{project.format_id}</span>
					{shouldOpenProjectDb(project.database_path) && <span className="rounded bg-surface-muted px-1.5 py-0.5 text-caption text-text-muted" title={project.database_path!}>{t("welcome.recentDbBadge")}</span>}
					{Number.isFinite(opened) && <time dateTime={project.last_opened} title={new Date(opened).toLocaleString(locale)} className="ml-auto inline-flex items-center gap-1 text-caption text-text-muted">
						<Clock size={12} aria-hidden="true" />{new Intl.RelativeTimeFormat(locale, { numeric: "auto" }).format(Math.round(seconds / divisor), unit)}
					</time>}
				</span>
				<span className="mt-1 flex min-w-0 flex-col gap-1 sm:flex-row sm:items-center sm:gap-4">
					<span className="min-w-0 flex-1 truncate text-caption text-text-muted" title={project.path}>{project.path}</span>
				{percent !== undefined && <span className="flex shrink-0 items-center gap-2 sm:w-44">
					<span role="progressbar" aria-label={t("welcome.translatedPercent", { percent })} aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent} className={`h-1.5 flex-1 overflow-hidden rounded-full ${statusStyles.pending.bar}`}>
						<span className={`block h-full ${statusStyles.translated.bar}`} style={{ width: `${percent}%` }} />
					</span>
					<span className="text-caption text-text-muted">{t("welcome.translatedPercent", { percent })}</span>
				</span>}
				</span>
			</button>
			<button type="button" onClick={onQueue} title={t("welcome.addToQueue")} aria-label={t("welcome.addToQueue")} className="mr-3 rounded p-1 text-text-muted hover:bg-accent-muted hover:text-accent-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg">
				<Plus size={14} />
			</button>
		</div>
	);
}

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
		// Manual mode keeps file-source selections as files.
		await runOpenAction(async () => {
			const path = await pickGameSource(t, formats, formatId);
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
		const gamePath = await pickGameSource(t, formats);
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
		const path = await pickGameFile(t, formats);
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
	const continueProject = project ?? recentProjects[0];
	const availableFormats = formats?.filter((f) => f.stability !== "comingsoon") ?? [];
	const stableFormats = availableFormats.filter((f) => f.stability !== "experimental");
	const orderedFormats = [...stableFormats, ...availableFormats.filter((f) => f.stability === "experimental")];
	const engineChip = (f: typeof orderedFormats[number]) => (
		<span key={f.id} data-engine={f.id} title={`${formatDescriptionText(f.id, f.description) ?? ""} (${f.extensions.join(", ")})`}
			className={`inline-flex items-center gap-1.5 rounded-md border px-2 py-1 text-caption ${f.stability === "experimental" ? "border-border bg-surface-muted text-text-muted" : "border-accent bg-surface text-accent-fg"}`}>
			{f.name}
			{f.stability === "experimental" && <span className="text-text-muted">{t("welcome.format.experimental")}</span>}
		</span>
	);
	const showProviderHint =
		providers !== undefined &&
		!hasAnyReadyProvider(providers, config) &&
		!hintDismissed;

	const dismissProviderHint = () => {
		setHintDismissed(true);
		saveProviderSetupHintDismissed(true);
	};

	return (
		<div className="flex w-full flex-col min-h-full gap-8 px-6 pb-6 pt-16 max-w-4xl mx-auto text-text text-body [@media(max-height:760px)]:pt-4 [@media(max-height:760px)]:gap-4 [@media(max-height:760px)]:pb-4">
			{/* Hero */}
			<section className="flex flex-col gap-6 [@media(max-height:760px)]:gap-3">
			<div>
				<h1 className="text-page font-bold mb-1">{t("welcome.headline")}</h1>
				<p className="text-section text-text-muted">
					{t("welcome.tagline")}
				</p>
			</div>

			{/* Open buttons */}
			<div>
				<div className="flex gap-4 flex-wrap">
					{continueProject && (
						<button
							onClick={() => project ? navigate("/editor") : openRecent(recentProjects[0])}
							disabled={opening}
							className="flex items-center gap-2 px-6 py-3 [@media(max-height:760px)]:py-2 bg-accent hover:bg-accent-hover disabled:opacity-50 disabled:cursor-not-allowed text-white rounded-lg text-body font-medium transition-colors"
						>
							{opening && <Loader size={18} className="animate-spin" />}
							{t("welcome.continue", { name: continueProject.name })}
						</button>
					)}
					<button
						onClick={handleOpenFolder}
						disabled={opening}
						className={`flex items-center gap-2 px-6 py-3 [@media(max-height:760px)]:py-2 disabled:opacity-50 disabled:cursor-not-allowed rounded-lg text-body font-medium transition-colors ${continueProject ? "border border-border bg-surface hover:bg-surface-muted" : "bg-accent hover:bg-accent-hover text-white"}`}
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
						className="flex items-center gap-2 px-6 py-3 [@media(max-height:760px)]:py-2 border border-border bg-surface hover:bg-surface-muted disabled:opacity-50 disabled:cursor-not-allowed rounded-lg text-body font-medium transition-colors"
					>
						{opening ? (
							<Loader size={18} className="animate-spin" />
						) : (
							<File size={18} />
						)}
						{opening ? t("welcome.opening") : t("welcome.openFile")}
					</button>
				</div>
				<details className="mt-4 [@media(max-height:760px)]:mt-3">
					<summary className="cursor-pointer text-body text-text-muted">{t("welcome.otherWays")}</summary>
					<div className="flex gap-3 flex-wrap mt-3">
					<button
						type="button"
						onClick={handleOpenProjectDb}
						disabled={opening}
						className="flex items-center gap-2 px-6 py-3 [@media(max-height:760px)]:py-2 border border-border bg-surface hover:bg-surface-muted disabled:opacity-50 disabled:cursor-not-allowed rounded-lg text-body font-medium transition-colors"
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
						className="flex items-center gap-2 px-6 py-3 [@media(max-height:760px)]:py-2 border border-border bg-surface hover:bg-surface-muted disabled:opacity-50 disabled:cursor-not-allowed rounded-lg text-body font-medium transition-colors"
					>
						<Package size={18} />
						{t("welcome.applyPatch")}
					</button>
					<button type="button" onClick={() => setRecoveryPath(project?.path ?? "")} disabled={opening}
						className="flex items-center gap-2 px-6 py-3 [@media(max-height:760px)]:py-2 border border-border bg-surface hover:bg-surface-muted disabled:opacity-50 rounded-lg text-body font-medium">
						<RotateCcw size={18} />{t("recovery.action")}
					</button>
				</div>
				<div className="flex mt-2">
					<button
						onClick={handleChooseFormatManually}
						disabled={opening}
						className="flex items-center gap-1 text-caption text-text-muted hover:text-text disabled:opacity-50"
					>
						<Settings2 size={12} />
						{t("welcome.chooseFormat")}
					</button>
				</div>
				</details>
				{opening && (
					<p className="text-center text-caption text-text-muted mt-2">
						{t("welcome.extracting")}
					</p>
				)}
			</div>

			</section>

			{showProviderHint && (
				<div className="p-4 rounded-lg border border-border bg-warning-muted [@media(max-height:760px)]:p-3">
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
								aria-label={t("common.close")}
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
									const colorClass = FORMAT_COLOR;
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
														<span className="shrink-0 text-caption px-1.5 py-0.5 rounded bg-warning-muted text-warning">
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
										: t("welcome.format.chooseSource")}
						</button>
					</div>
				</div>
			)}

			{recentProjects.length > 0 && (
				<section aria-labelledby="recent-projects-title" className="rounded-lg border border-border bg-surface">
					<h2 id="recent-projects-title" className="px-4 py-3 text-body font-semibold [@media(max-height:760px)]:py-2">{t("welcome.recent")}</h2>
					<div>
						{recentProjects.slice(0, 6).map((p) => <RecentProjectRow key={`${p.database_path ?? ""}:${p.path}:${p.format_id}`} project={p} opening={opening} onOpen={() => openRecent(p)} onQueue={() => handleAddToQueue(p.path)} />)}
					</div>
				</section>
			)}

			{orderedFormats.length > 0 && (
				<section aria-labelledby="supported-engines-title">
					<h2 id="supported-engines-title" className="mb-3 text-body font-semibold">{t("welcome.supportedEngines")}</h2>
					<div className="flex flex-wrap gap-2">
						{orderedFormats.slice(0, 8).map(engineChip)}
						{orderedFormats.length > 8 && <details className="basis-full">
							<summary className="w-fit cursor-pointer rounded text-caption text-text-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg">{t("welcome.moreEngines", { count: orderedFormats.length - 8 })}</summary>
							<div className="mt-2 flex flex-wrap gap-2">{orderedFormats.slice(8).map(engineChip)}</div>
						</details>}
					</div>
				</section>
			)}

			<footer className="mt-auto flex justify-center gap-6 text-caption text-text-muted">
				<a href="https://github.com/Miike0303/Locust" target="_blank" rel="noopener noreferrer" className="hover:underline">GitHub</a>
				<span>v{APP_VERSION}</span>
			</footer>

			{showPatchModal && (
				<Suspense fallback={null}>
					<PatchModal
						open={showPatchModal}
						onClose={() => setShowPatchModal(false)}
						allowPack={false}
					/>
				</Suspense>
			)}
			{recoveryPath !== null && (
				<Suspense fallback={null}>
					<InjectionRecoveryModal defaultGamePath={recoveryPath} onClose={() => setRecoveryPath(null)} />
				</Suspense>
			)}
		</div>
	);
}
