import { useState, useEffect, useMemo, useRef } from "react";
import { useNavigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import {
	X,
	ChevronUp,
	ChevronDown,
	Trash2,
	Play,
	Square,
	CheckCircle,
	AlertCircle,
	Loader2,
	Clock,
	FolderOpen,
	File,
} from "lucide-react";
import clsx from "clsx";
import { useQueueStore, type QueueItem } from "../stores/queueStore";
import {
	getProviders,
	getConfig,
	type TranslationStartParams,
} from "../lib/api";
import {
	resolveTranslationDefaults,
	coerceProviderId,
	readLastUsedTranslationPrefs,
	saveLastUsedTranslationPrefs,
} from "../lib/translationDefaults";
import {
	useModalA11y,
	MODAL_BACKDROP_CLASS,
	modalPanelClass,
} from "../lib/modalA11y";
import {
	resolveProviderReadiness,
	formatProviderOptionLabel,
} from "../lib/providerReadiness";
import { buildSettingsPath } from "../lib/settingsNav";
import EmptyState from "./EmptyState";
import { useT } from "../lib/i18n";

const IS_TAURI = "__TAURI_INTERNALS__" in window;

const statusIcons: Record<string, typeof Clock> = {
	pending: Clock,
	extracting: Loader2,
	translating: Loader2,
	validating: Loader2,
	done: CheckCircle,
	error: AlertCircle,
	cancelled: Square,
};

const statusRowStyles: Record<string, string> = {
	pending: "border-l-transparent",
	extracting: "bg-blue-50/70 dark:bg-blue-950/30 border-l-blue-500",
	translating: "bg-emerald-50/70 dark:bg-emerald-950/30 border-l-emerald-500",
	validating: "bg-amber-50/70 dark:bg-amber-950/30 border-l-amber-500",
	done: "bg-gray-50 dark:bg-gray-800/50 border-l-emerald-500/60",
	error: "bg-red-50 dark:bg-red-950/30 border-l-red-500",
	cancelled: "opacity-60 border-l-gray-400",
};

const STATUS_LABEL_KEYS: Record<string, string> = {
	pending: "queue.status.pending",
	extracting: "queue.status.extracting",
	translating: "queue.status.translating",
	validating: "queue.status.validating",
	done: "queue.status.done",
	error: "queue.status.error",
	cancelled: "queue.status.cancelled",
};

const statusIconColors: Record<string, string> = {
	pending: "text-gray-400 dark:text-gray-500",
	extracting: "text-blue-500 dark:text-blue-400 animate-spin",
	translating: "text-emerald-500 dark:text-emerald-400 animate-spin",
	validating: "text-amber-500 dark:text-amber-400 animate-spin",
	done: "text-emerald-600 dark:text-emerald-400",
	error: "text-red-500 dark:text-red-400",
	cancelled: "text-gray-500 dark:text-gray-500",
};

const settingsInputClass =
	"mt-1 w-full p-1.5 border rounded text-sm dark:bg-gray-900 dark:border-gray-600 dark:text-gray-100";
const settingsLabelClass =
	"text-xs font-medium text-gray-500 dark:text-gray-400";

export default function QueuePanel() {
	const t = useT();
	const navigate = useNavigate();
	const {
		items,
		isRunning,
		isPanelOpen,
		translationParams,
		setPanelOpen,
		addItem,
		removeItem,
		moveItem,
		clearCompleted,
		setParams,
		startQueue,
		cancelQueue,
	} = useQueueStore();
	const { data: providers } = useQuery({
		queryKey: ["providers"],
		queryFn: getProviders,
		enabled: isPanelOpen,
	});
	const {
		data: config,
		isFetched: configFetched,
		isError: configError,
	} = useQuery({
		queryKey: ["config"],
		queryFn: getConfig,
		enabled: isPanelOpen,
	});

	// Same defaults chain as TranslationModal: last-used > Settings config > fallbacks.
	const initialDefaults = useMemo(
		() => resolveTranslationDefaults(undefined, readLastUsedTranslationPrefs()),
		[],
	);
	const [providerId, setProviderId] = useState(
		translationParams?.provider_id ?? initialDefaults.providerId,
	);
	const [sourceLang, setSourceLang] = useState(
		translationParams?.options.source_lang ?? initialDefaults.sourceLang,
	);
	const [targetLang, setTargetLang] = useState(
		translationParams?.options.target_lang ?? initialDefaults.targetLang,
	);
	const [batchSize, setBatchSize] = useState(
		translationParams?.options.batch_size ?? initialDefaults.batchSize,
	);
	const [gameContext, setGameContext] = useState(
		translationParams?.options.game_context ?? "",
	);
	const { dialogRef, dialogProps, titleProps } = useModalA11y({
		open: isPanelOpen,
		onClose: () => setPanelOpen(false),
		ownEscape: true,
	});

	// Once the config query settles, fill in config defaults — unless params from
	// an earlier run this session already seeded the form.
	const defaultsAppliedRef = useRef(false);
	useEffect(() => {
		if (
			!isPanelOpen ||
			defaultsAppliedRef.current ||
			!(configFetched || configError)
		)
			return;
		defaultsAppliedRef.current = true;
		if (translationParams) return;
		const d = resolveTranslationDefaults(
			config,
			readLastUsedTranslationPrefs(),
		);
		setProviderId(coerceProviderId(d.providerId, providers, config));
		setSourceLang(d.sourceLang);
		setTargetLang(d.targetLang);
		setBatchSize(d.batchSize);
	}, [
		isPanelOpen,
		config,
		configFetched,
		configError,
		providers,
		translationParams,
	]);

	// Keep the provider <select> on a listed, ready id when possible.
	useEffect(() => {
		if (!providers || providers.length === 0) return;
		setProviderId((prev) => coerceProviderId(prev, providers, config));
	}, [providers, config]);

	if (!isPanelOpen) return null;

	const handleAddFile = async () => {
		if (IS_TAURI) {
			const { open } = await import("@tauri-apps/plugin-dialog");
			const selected = await open({
				title: t("queue.dialog.addFiles"),
				multiple: true,
				filters: [
					{
						name: t("queue.dialog.gameFiles"),
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
					{ name: t("queue.dialog.allFiles"), extensions: ["*"] },
				],
			});
			if (Array.isArray(selected)) selected.forEach((p) => addItem(p));
			else if (typeof selected === "string") addItem(selected);
		} else {
			const path = prompt(t("queue.prompt.filePath"));
			if (path) addItem(path);
		}
	};

	const handleAddFolder = async () => {
		if (IS_TAURI) {
			const { open } = await import("@tauri-apps/plugin-dialog");
			const selected = await open({
				title: t("queue.dialog.addFolder"),
				directory: true,
			});
			if (typeof selected === "string") addItem(selected);
		} else {
			const path = prompt(t("queue.prompt.folderPath"));
			if (path) addItem(path);
		}
	};

	const buildParams = (): TranslationStartParams => ({
		provider_id: providerId,
		options: {
			source_lang: sourceLang,
			target_lang: targetLang,
			batch_size: batchSize,
			max_concurrent: 3,
			cost_limit_usd: null,
			game_context: gameContext || null,
			use_glossary: true,
			use_memory: true,
			skip_approved: true,
		},
	});

	const handleStart = () => {
		const params = buildParams();
		saveLastUsedTranslationPrefs({
			provider: providerId,
			source: sourceLang,
			target: targetLang,
			batchSize,
		});
		setParams(params);
		startQueue();
	};

	const pendingCount = items.filter((i) => i.status === "pending").length;
	const doneCount = items.filter((i) => i.status === "done").length;
	const activeCount = items.filter((i) =>
		["extracting", "translating", "validating"].includes(i.status),
	).length;
	const totalCount = items.length;
	const providerReadiness = resolveProviderReadiness(
		providerId,
		providers,
		config,
	);

	return (
		<div className={MODAL_BACKDROP_CLASS}>
			<div
				ref={dialogRef}
				{...dialogProps}
				className={modalPanelClass("max-w-2xl max-h-[80vh] flex flex-col")}
			>
				{/* Header */}
				<div className="flex items-center justify-between p-4 border-b border-gray-200 dark:border-gray-700">
					<h2 {...titleProps} className="font-bold text-lg">
						{t("queue.title")}
					</h2>
					<div className="flex items-center gap-2">
						{doneCount > 0 && (
							<button
								type="button"
								onClick={clearCompleted}
								className="text-xs text-gray-500 hover:text-gray-700 dark:text-gray-400 dark:hover:text-gray-200 focus:outline-none focus:ring-2 focus:ring-emerald-500 rounded px-1"
							>
								{t("queue.clearCompleted", { count: doneCount })}
							</button>
						)}
						<button
							type="button"
							onClick={() => setPanelOpen(false)}
							aria-label={t("queue.closeAria")}
							className="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 focus:outline-none focus:ring-2 focus:ring-emerald-500 rounded p-0.5"
						>
							<X size={20} />
						</button>
					</div>
				</div>

				<div className="flex-1 overflow-y-auto">
					{/* Queue list */}
					{items.length === 0 ? (
						<EmptyState
							title={t("queue.empty.title")}
							description={t("queue.empty.description")}
						/>
					) : (
						<div className="divide-y divide-gray-100 dark:divide-gray-800 border-b border-gray-100 dark:border-gray-800">
							{items.map((item, idx) => (
								<QueueItemRow
									key={item.id}
									item={item}
									index={idx}
									total={items.length}
									onRemove={() => removeItem(item.id)}
									onMoveUp={() => moveItem(item.id, "up")}
									onMoveDown={() => moveItem(item.id, "down")}
									disabled={isRunning}
								/>
							))}
						</div>
					)}

					{/* Add buttons */}
					{!isRunning && (
						<div className="flex gap-2 p-4">
							<button
								onClick={handleAddFile}
								className="flex items-center gap-2 px-3 py-2 text-sm border border-dashed border-gray-300 dark:border-gray-600 rounded-lg hover:bg-gray-50 dark:hover:bg-gray-800 transition-colors"
							>
								<File size={14} />
								{t("queue.addFile")}
							</button>
							<button
								onClick={handleAddFolder}
								className="flex items-center gap-2 px-3 py-2 text-sm border border-dashed border-gray-300 dark:border-gray-600 rounded-lg hover:bg-gray-50 dark:hover:bg-gray-800 transition-colors"
							>
								<FolderOpen size={14} />
								{t("queue.addFolder")}
							</button>
						</div>
					)}

					{/* Translation settings */}
					{!isRunning && items.length > 0 && (
						<div className="p-4 border-t border-gray-200 dark:border-gray-700 space-y-3 bg-gray-50/50 dark:bg-gray-800/30">
							<h3 className="text-xs font-semibold text-gray-500 dark:text-gray-400 uppercase">
								{t("queue.settings")}
							</h3>
							<div className="grid grid-cols-3 gap-3">
								<div>
									<label className={settingsLabelClass}>{t("queue.provider")}</label>
									<select
										value={providerId}
										onChange={(e) => setProviderId(e.target.value)}
										className={settingsInputClass}
									>
										{providers?.map((p) => (
											<option key={p.id} value={p.id}>
												{formatProviderOptionLabel(p)}
											</option>
										))}
									</select>
								</div>
								<div>
									<label className={settingsLabelClass}>{t("queue.source")}</label>
									<input
										value={sourceLang}
										onChange={(e) => setSourceLang(e.target.value)}
										className={settingsInputClass}
									/>
								</div>
								<div>
									<label className={settingsLabelClass}>{t("queue.target")}</label>
									<input
										value={targetLang}
										onChange={(e) => setTargetLang(e.target.value)}
										className={settingsInputClass}
									/>
								</div>
							</div>
							<div className="grid grid-cols-2 gap-3">
								<div>
									<label className={settingsLabelClass}>{t("queue.batchSize")}</label>
									<input
										type="number"
										value={batchSize}
										onChange={(e) => setBatchSize(+e.target.value)}
										min={1}
										max={100}
										className={settingsInputClass}
									/>
								</div>
								<div>
									<label className={settingsLabelClass}>{t("queue.gameContext")}</label>
									<input
										value={gameContext}
										onChange={(e) => setGameContext(e.target.value)}
										placeholder={t("common.optional")}
										className={settingsInputClass}
									/>
								</div>
							</div>
						</div>
					)}
				</div>

				{/* Footer */}
				<div className="p-4 border-t border-gray-200 dark:border-gray-700">
					{!providerReadiness.ready &&
						providerReadiness.reason === "missing_key" &&
						!isRunning &&
						pendingCount > 0 && (
							<div className="mb-3 p-2 rounded border border-amber-200 bg-amber-50 dark:border-amber-800 dark:bg-amber-900/20 text-sm text-amber-800 dark:text-amber-200">
								{t("queue.needsKey")}{" "}
								<button
									type="button"
									onClick={() => {
										setPanelOpen(false);
										navigate(buildSettingsPath("providers"));
									}}
									className="font-medium text-emerald-700 dark:text-emerald-400 hover:underline"
								>
									{t("queue.openSettings")}
								</button>
							</div>
						)}
					<div className="flex items-center justify-between gap-4">
						<div className="text-sm text-gray-600 dark:text-gray-300 tabular-nums">
							{isRunning ? (
								<span>
									{t(activeCount > 0 ? "queue.running" : "queue.starting", {
										done: doneCount,
										total: totalCount,
									})}
								</span>
							) : (
								<span>
									{t("queue.waiting", { pending: pendingCount, done: doneCount })}
								</span>
							)}
						</div>
						{isRunning ? (
							<button
								type="button"
								onClick={cancelQueue}
								className="flex items-center gap-2 px-4 py-2 bg-red-600 hover:bg-red-700 text-white rounded-lg text-sm font-medium transition-colors focus:outline-none focus:ring-2 focus:ring-red-400 focus:ring-offset-2 dark:focus:ring-offset-gray-900"
							>
								<Square size={14} aria-hidden="true" />
								{t("queue.cancel")}
							</button>
						) : (
							<button
								type="button"
								onClick={handleStart}
								disabled={pendingCount === 0}
								className="flex items-center gap-2 px-4 py-2 bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50 disabled:cursor-not-allowed text-white rounded-lg text-sm font-medium transition-colors focus:outline-none focus:ring-2 focus:ring-emerald-400 focus:ring-offset-2 dark:focus:ring-offset-gray-900"
							>
								<Play size={14} aria-hidden="true" />
								{t("queue.start", { count: pendingCount })}
							</button>
						)}
					</div>
				</div>
			</div>
		</div>
	);
}

function QueueItemRow({
	item,
	index,
	total,
	onRemove,
	onMoveUp,
	onMoveDown,
	disabled,
}: {
	item: QueueItem;
	index: number;
	total: number;
	onRemove: () => void;
	onMoveUp: () => void;
	onMoveDown: () => void;
	disabled: boolean;
}) {
	const t = useT();
	const Icon = statusIcons[item.status] ?? Clock;
	const percent =
		item.progress.total > 0
			? Math.round((item.progress.completed / item.progress.total) * 100)
			: 0;
	const isActive =
		item.status === "extracting" ||
		item.status === "translating" ||
		item.status === "validating";
	const showProgress = isActive || item.status === "done";
	const canDismiss = ["done", "error", "cancelled"].includes(item.status);

	return (
		<div
			className={clsx(
				"flex items-center gap-3 px-4 py-2.5 border-l-2",
				statusRowStyles[item.status] ?? statusRowStyles.pending,
			)}
		>
			<Icon
				size={16}
				className={statusIconColors[item.status]}
				aria-hidden="true"
			/>
			<div className="flex-1 min-w-0">
				<div className="flex items-center gap-2">
					<div className="text-sm font-medium truncate text-gray-900 dark:text-gray-100">
						{item.projectName}
					</div>
					<span className="shrink-0 text-[10px] font-semibold uppercase tracking-wide text-gray-500 dark:text-gray-400">
						{STATUS_LABEL_KEYS[item.status]
							? t(STATUS_LABEL_KEYS[item.status])
							: item.status}
					</span>
				</div>
				<div className="text-xs text-gray-500 dark:text-gray-400 truncate">
					{item.projectPath}
				</div>
				<div className="mt-1 h-4 flex items-center gap-2">
					{item.status === "validating" ? (
						<span className="text-[11px] text-amber-600 dark:text-amber-400">
							{t("queue.validatingTranslations")}
						</span>
					) : showProgress && item.progress.total > 0 ? (
						<>
							<div className="flex-1 h-1.5 bg-gray-200 dark:bg-gray-700 rounded-full overflow-hidden">
								<div
									className={clsx(
										"h-full rounded-full transition-all duration-300",
										item.status === "done"
											? "bg-emerald-500"
											: "bg-emerald-500",
									)}
									style={{
										width: `${item.status === "done" ? 100 : percent}%`,
									}}
								/>
							</div>
							<span className="text-[11px] text-gray-500 dark:text-gray-400 tabular-nums w-24 text-right shrink-0">
								{item.progress.completed}/{item.progress.total} ·{" "}
								{item.status === "done" ? 100 : percent}%
							</span>
						</>
					) : item.status === "extracting" ? (
						<span className="text-[11px] text-blue-600 dark:text-blue-400">
							{t("queue.extractingStrings")}
						</span>
					) : null}
				</div>
				{item.status === "done" && item.validationError && (
					<div
						className="text-xs text-amber-700 dark:text-amber-400 mt-0.5 truncate"
						title={item.validationError}
					>
						{t("queue.validation.failed")}
					</div>
				)}
				{item.status === "done" &&
					item.validationError == null &&
					item.validationIssues != null && (
						<div
							className={
								item.validationIssues > 0
									? "text-xs text-amber-700 dark:text-amber-400 mt-0.5"
									: "text-xs text-gray-500 dark:text-gray-400 mt-0.5"
							}
						>
							{item.validationIssues > 0
								? t("queue.validation.issues", { count: item.validationIssues })
								: t("queue.validation.clean")}
						</div>
					)}
				{item.error && (
					<div
						className="text-xs text-red-600 dark:text-red-400 mt-0.5 truncate"
						title={item.error}
					>
						{item.error}
					</div>
				)}
			</div>
			{!disabled && item.status === "pending" && (
				<div className="flex items-center gap-0.5 shrink-0">
					<button
						type="button"
						onClick={onMoveUp}
						disabled={index === 0}
						aria-label={t("queue.moveUp")}
						className="p-1 text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 disabled:opacity-30 focus:outline-none focus:ring-2 focus:ring-emerald-500 rounded"
					>
						<ChevronUp size={14} />
					</button>
					<button
						type="button"
						onClick={onMoveDown}
						disabled={index === total - 1}
						aria-label={t("queue.moveDown")}
						className="p-1 text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 disabled:opacity-30 focus:outline-none focus:ring-2 focus:ring-emerald-500 rounded"
					>
						<ChevronDown size={14} />
					</button>
					<button
						type="button"
						onClick={onRemove}
						aria-label={t("queue.remove")}
						className="p-1 text-gray-400 hover:text-red-500 dark:hover:text-red-400 focus:outline-none focus:ring-2 focus:ring-red-400 rounded"
					>
						<Trash2 size={14} />
					</button>
				</div>
			)}
			{canDismiss && !disabled && (
				<button
					type="button"
					onClick={onRemove}
					aria-label={t("queue.dismiss")}
					className="shrink-0 px-2 py-1 text-[11px] font-medium text-gray-500 hover:text-gray-700 dark:text-gray-400 dark:hover:text-gray-200 border border-gray-200 dark:border-gray-600 rounded focus:outline-none focus:ring-2 focus:ring-emerald-500"
				>
					{t("common.dismiss")}
				</button>
			)}
		</div>
	);
}
