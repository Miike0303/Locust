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
} from "lucide-react";
import { getFormats, getConfig, getProviders } from "../lib/api";
import { useProjectStore } from "../stores/projectStore";
import { useQueueStore } from "../stores/queueStore";
import { addLog } from "../stores/logStore";
import { addToast } from "../stores/toastStore";
import PatchModal from "../components/PatchModal";
import {
	completeOpenProject,
	formatPickerPathFromState,
	isDetectionFailure,
	pickGameFolder,
} from "../lib/openProjectFlow";
import {
	useModalA11y,
	MODAL_BACKDROP_CLASS,
	modalPanelClass,
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

const IS_TAURI = "__TAURI_INTERNALS__" in window;

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
	sugarcube:
		"bg-emerald-100 text-emerald-700 dark:bg-emerald-900/40 dark:text-emerald-300",
	"html-game":
		"bg-cyan-100 text-cyan-700 dark:bg-cyan-900/40 dark:text-cyan-300",
	unreal: "bg-red-100 text-red-700 dark:bg-red-900/40 dark:text-red-300",
	kirikiri:
		"bg-amber-100 text-amber-800 dark:bg-amber-900/40 dark:text-amber-300",
	yuris:
		"bg-violet-100 text-violet-700 dark:bg-violet-900/40 dark:text-violet-300",
	nscripter:
		"bg-slate-100 text-slate-700 dark:bg-slate-800 dark:text-slate-300",
	tyrano: "bg-rose-100 text-rose-700 dark:bg-rose-900/40 dark:text-rose-300",
	qsp: "bg-teal-100 text-teal-700 dark:bg-teal-900/40 dark:text-teal-300",
	vntextpatch:
		"bg-lime-100 text-lime-800 dark:bg-lime-900/40 dark:text-lime-300",
};

export default function Welcome() {
	const t = useT();
	const navigate = useNavigate();
	const location = useLocation();
	const queryClient = useQueryClient();
	const setProject = useProjectStore((s) => s.setProject);
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

	// Format picker state — shown only when auto-detect fails, or on explicit request.
	const [picker, setPicker] = useState<{
		path: string | null;
		reason: "manual" | "detect-failed";
	} | null>(null);
	const [selectedFormat, setSelectedFormat] = useState("auto");
	const [opening, setOpening] = useState(false);
	const [showPatchModal, setShowPatchModal] = useState(false);
	const { dialogRef, dialogProps, titleProps } = useModalA11y({
		open: !!picker,
		onClose: () => setPicker(null),
		ownEscape: true,
	});

	useEffect(() => {
		const pendingPath = formatPickerPathFromState(location.state);
		if (!pendingPath) return;
		setSelectedFormat("");
		setPicker({ path: pendingPath, reason: "detect-failed" });
		navigate(".", { replace: true, state: {} });
	}, [location.state, navigate]);

	const openWithPath = async (path: string, formatId?: string) => {
		setOpening(true);
		try {
			const result = await completeOpenProject(path, formatId, {
				setProject,
				queryClient,
			});
			addLog(
				"info",
				`Opened: ${result.project_name} (${result.format_name}, ${result.total_strings} strings)`,
				undefined,
				"project",
			);
			setPicker(null);
			navigate("/editor");
		} catch (err: any) {
			const msg = err?.message ?? String(err);
			if (!formatId && isDetectionFailure(msg)) {
				// Auto-detect failed — let the user pick the engine instead of toasting.
				addLog("warning", "Could not auto-detect game engine", path, "project");
				setSelectedFormat("");
				setPicker({ path, reason: "detect-failed" });
			} else {
				addLog("error", `Failed to open project`, msg, "project");
				addToast("error", t("welcome.toast.failedOpen", { error: msg }));
			}
		} finally {
			setOpening(false);
		}
	};

	const pickFolderPath = () => pickGameFolder(t);

	const handleConfirmFormat = async () => {
		if (!picker) return;
		const formatId = selectedFormat === "auto" ? undefined : selectedFormat;
		if (picker.path) {
			await openWithPath(picker.path, formatId);
			return;
		}
		// Manual mode: format chosen first, now pick the game folder.
		const path = await pickFolderPath();
		if (path) await openWithPath(path, formatId);
	};

	const handleAddToQueue = (path: string) => {
		addToQueue(path);
		setQueueOpen(true);
		addToast("info", t("welcome.toast.addedToQueue"));
	};

	const handleOpenFile = async () => {
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
	};

	const handleOpenFolder = async () => {
		const path = await pickFolderPath();
		if (path) await openWithPath(path);
	};

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
		<div className="flex flex-col min-h-full p-8 max-w-4xl mx-auto">
			{/* Hero */}
			<div className="text-center mb-8">
				<Globe size={48} className="mx-auto mb-3 text-emerald-500" />
				<h1 className="text-3xl font-bold mb-1">{t("nav.appName")}</h1>
				<p className="text-gray-500 dark:text-gray-400">
					{t("welcome.tagline")}
				</p>
			</div>

			{showWelcomeGuide && (
				<section
					aria-label={t("welcome.guide.aria")}
					className="mb-8 rounded-lg border border-emerald-200 bg-emerald-50 dark:border-emerald-800 dark:bg-emerald-950/30 p-4"
				>
					<div className="flex items-start justify-between gap-3 mb-3">
						<div>
							<h2 className="text-sm font-semibold text-emerald-800 dark:text-emerald-200">
								{t("welcome.guide.title")}
							</h2>
							<p className="text-xs text-gray-600 dark:text-gray-400 mt-0.5">
								{t("welcome.guide.subtitle")}
							</p>
						</div>
						<button
							type="button"
							onClick={dismissWelcomeGuide}
							aria-label={t("welcome.guide.dismiss")}
							className="shrink-0 text-emerald-700 hover:text-emerald-900 dark:text-emerald-300 dark:hover:text-emerald-100 focus:outline-none focus:ring-2 focus:ring-emerald-500 rounded p-0.5"
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
									className="flex items-start gap-2 rounded-md border border-emerald-100 bg-white/70 p-3 dark:border-emerald-900/50 dark:bg-gray-900/40"
								>
									<span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-emerald-600 text-xs font-bold text-white">
										{index + 1}
									</span>
									<div className="min-w-0">
										<div className="flex items-center gap-1.5 text-sm font-medium text-gray-800 dark:text-gray-200">
											<StepIcon
												size={14}
												className="text-emerald-600 dark:text-emerald-400"
												aria-hidden="true"
											/>
											{t(`welcome.guide.${step.id}.label`)}
										</div>
										<p className="text-xs text-gray-500 dark:text-gray-400 mt-0.5">
											{t(`welcome.guide.${step.id}.description`)}
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
						className="flex items-center gap-2 px-6 py-3 bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50 disabled:cursor-not-allowed text-white rounded-lg text-sm font-medium transition-colors"
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
						className="flex items-center gap-2 px-6 py-3 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 disabled:opacity-50 disabled:cursor-not-allowed rounded-lg text-sm font-medium transition-colors"
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
						onClick={() => setShowPatchModal(true)}
						disabled={opening}
						className="flex items-center gap-2 px-6 py-3 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 disabled:opacity-50 disabled:cursor-not-allowed rounded-lg text-sm font-medium transition-colors"
					>
						<Package size={18} />
						{t("welcome.applyPatch")}
					</button>
				</div>
				<div className="flex justify-center mt-2">
					<button
						onClick={handleChooseFormatManually}
						disabled={opening}
						className="flex items-center gap-1 text-xs text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 disabled:opacity-50"
					>
						<Settings2 size={12} />
						{t("welcome.chooseFormat")}
					</button>
				</div>
				{opening && (
					<p className="text-center text-xs text-gray-400 mt-2">
						{t("welcome.extracting")}
					</p>
				)}
			</div>

			{showProviderHint && (
				<div className="mb-8 p-4 rounded-lg border border-amber-200 bg-amber-50 dark:border-amber-800 dark:bg-amber-900/20">
					<div className="flex items-start justify-between gap-3">
						<p className="text-sm text-amber-900 dark:text-amber-100">
							{t("welcome.providerHint")}{" "}
							<button
								type="button"
								onClick={() => navigate(buildSettingsPath("providers"))}
								className="font-medium text-emerald-700 dark:text-emerald-400 hover:underline"
							>
								{t("welcome.providerHintLink")}
							</button>
							.
						</p>
						<button
							type="button"
							onClick={dismissProviderHint}
							aria-label={t("welcome.providerHintDismiss")}
							className="shrink-0 text-amber-700 dark:text-amber-300 hover:text-amber-900 dark:hover:text-amber-100"
						>
							<X size={16} />
						</button>
					</div>
				</div>
			)}

			{/* Format Picker Modal — shown when auto-detect fails or on "Choose format manually" */}
			{picker && (
				<div className={MODAL_BACKDROP_CLASS}>
					<div
						ref={dialogRef}
						{...dialogProps}
						className={modalPanelClass("max-w-md p-6")}
					>
						<div className="flex justify-between items-center mb-4">
							<h2 {...titleProps} className="text-lg font-bold">
								{t("welcome.format.title")}
							</h2>
							<button
								onClick={() => setPicker(null)}
								className="text-gray-400 hover:text-gray-600"
							>
								<X size={20} />
							</button>
						</div>

						{picker.path && (
							<p className="text-sm text-gray-500 mb-1 truncate">
								{picker.path}
							</p>
						)}
						{picker.reason === "detect-failed" ? (
							<p className="text-xs text-amber-600 dark:text-amber-400 mb-4">
								{t("welcome.format.detectFailed")}
							</p>
						) : (
							<p className="text-xs text-gray-400 mb-4">
								{t("welcome.format.manualHint")}
							</p>
						)}

						<div className="space-y-1.5 max-h-64 overflow-y-auto mb-4">
							{picker.reason !== "detect-failed" && (
								<button
									onClick={() => setSelectedFormat("auto")}
									className={`w-full text-left p-3 rounded-lg border transition-colors flex items-center gap-3 ${
										selectedFormat === "auto"
											? "border-emerald-500 bg-emerald-50 dark:bg-emerald-900/20"
											: "border-gray-200 dark:border-gray-700 hover:bg-gray-50 dark:hover:bg-gray-800"
									}`}
								>
									<div className="p-1.5 rounded bg-emerald-100 text-emerald-700 dark:bg-emerald-900/40 dark:text-emerald-300">
										<Wand2 size={16} />
									</div>
									<div>
										<div className="text-sm font-medium">{t("welcome.format.auto")}</div>
										<div className="text-xs text-gray-500">
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
										"bg-gray-100 text-gray-700 dark:bg-gray-800 dark:text-gray-300";
									const experimental = f.stability === "experimental";
									return (
										<button
											key={f.id}
											onClick={() => setSelectedFormat(f.id)}
											className={`w-full text-left p-3 rounded-lg border transition-colors flex items-center gap-3 ${
												selectedFormat === f.id
													? "border-emerald-500 bg-emerald-50 dark:bg-emerald-900/20"
													: "border-gray-200 dark:border-gray-700 hover:bg-gray-50 dark:hover:bg-gray-800"
											}`}
										>
											<div className={`p-1.5 rounded ${colorClass}`}>
												<Icon size={16} />
											</div>
											<div className="min-w-0 flex-1">
												<div className="text-sm font-medium flex items-center gap-2">
													<span className="truncate">{f.name}</span>
													{experimental && (
														<span className="shrink-0 text-[10px] uppercase tracking-wide px-1.5 py-0.5 rounded bg-sky-100 text-sky-700 dark:bg-sky-900/40 dark:text-sky-300">
															{t("welcome.format.experimental")}
														</span>
													)}
												</div>
												<div className="text-xs text-gray-500">
													{f.extensions.join(", ")}
												</div>
											</div>
										</button>
									);
								})}
						</div>

						<button
							onClick={handleConfirmFormat}
							disabled={opening || !selectedFormat}
							className="w-full py-2.5 bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50 text-white rounded-lg font-medium transition-colors"
						>
							{opening
								? t("welcome.openingDots")
								: picker.path
									? t("welcome.format.openProject")
									: t("welcome.format.chooseFolder")}
						</button>
					</div>
				</div>
			)}

			{/* Recent Projects */}
			{recentProjects.length > 0 && (
				<div className="mb-10">
					<h2 className="text-sm font-semibold text-gray-500 uppercase mb-3">
						{t("welcome.recent")}
					</h2>
					<div className="space-y-2">
						{recentProjects.map((p, i) => {
							const Icon = FORMAT_ICONS[p.format_id] ?? Globe;
							const colorClass =
								FORMAT_COLORS[p.format_id] ??
								"bg-gray-100 text-gray-700 dark:bg-gray-800 dark:text-gray-300";
							return (
								<div
									key={i}
									className="w-full p-3 rounded-lg border border-gray-200 dark:border-gray-700 hover:border-emerald-300 hover:bg-gray-50 dark:hover:border-emerald-700 dark:hover:bg-gray-800 transition-colors flex items-center gap-3"
								>
									<button
										onClick={() => openWithPath(p.path, p.format_id)}
										disabled={opening}
										className="flex items-center gap-3 flex-1 min-w-0 text-left disabled:opacity-50 disabled:cursor-not-allowed"
									>
										<div className={`p-2 rounded-lg ${colorClass}`}>
											<Icon size={18} />
										</div>
										<div className="flex-1 min-w-0">
											<div className="font-medium truncate">{p.name}</div>
											<div className="text-xs text-gray-500 truncate">
												{p.path}
											</div>
										</div>
									</button>
									<div className="flex items-center gap-2 text-xs text-gray-400 shrink-0">
										<span
											className={`px-2 py-0.5 rounded-full text-xs ${colorClass}`}
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
											className="p-1 rounded hover:bg-gray-200 dark:hover:bg-gray-600 text-gray-400 hover:text-emerald-500"
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
					<h2 className="text-sm font-semibold text-gray-500 uppercase mb-3">
						{t("welcome.availableFormats")}
					</h2>
					<div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 gap-3">
						{formats
							.filter((f) => f.stability !== "comingsoon")
							.map((f) => {
								const Icon = FORMAT_ICONS[f.id] ?? Globe;
								const colorClass =
									FORMAT_COLORS[f.id] ??
									"bg-gray-100 text-gray-700 dark:bg-gray-800 dark:text-gray-300";
								const experimental = f.stability === "experimental";
								return (
									<div
										key={f.id}
										className="p-3 rounded-lg border border-gray-200 dark:border-gray-700"
									>
										<div className="flex items-center gap-2 mb-1">
											<div className={`p-1.5 rounded ${colorClass}`}>
												<Icon size={14} />
											</div>
											<span className="text-sm font-medium truncate">
												{f.name}
											</span>
											{experimental && (
												<span className="ml-auto shrink-0 text-[10px] uppercase tracking-wide px-1.5 py-0.5 rounded bg-sky-100 text-sky-700 dark:bg-sky-900/40 dark:text-sky-300">
													{t("welcome.format.experimental")}
												</span>
											)}
										</div>
										{f.description && (
											<p className="text-xs text-gray-500 line-clamp-2">
												{f.description}
											</p>
										)}
										<div className="mt-1.5 flex flex-wrap gap-1">
											{f.extensions.slice(0, 3).map((ext) => (
												<span
													key={ext}
													className="px-1.5 py-0.5 bg-gray-100 dark:bg-gray-800 rounded text-xs text-gray-500"
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
			<div className="mt-auto pt-8 flex justify-center gap-6 text-xs text-gray-400">
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
		</div>
	);
}
