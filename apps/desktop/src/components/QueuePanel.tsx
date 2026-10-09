import { IS_TAURI } from "../lib/runtime";
import { addToast } from "../stores/toastStore";
import { localizeApiError } from "../lib/apiError";
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
import type { MessageKey } from "../lib/i18n";
import { useQueueStore, type QueueItem, type QueueItemStatus } from "../stores/queueStore";
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
	readTranslationFallbacks,
	saveTranslationFallbacks,
	buildTranslationStartParams,
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
	translating: "bg-accent-muted border-l-accent-fg",
	validating: "bg-warning-muted border-l-warning",
	done: "bg-surface-muted border-l-success/60",
	error: "bg-danger-muted border-l-danger",
	cancelled: "opacity-60 border-l-text-muted",
};

const STATUS_LABEL_KEYS: Record<QueueItemStatus, MessageKey> = {
	pending: "queue.status.pending",
	extracting: "queue.status.extracting",
	translating: "queue.status.translating",
	validating: "queue.status.validating",
	done: "queue.status.done",
	error: "queue.status.error",
	cancelled: "queue.status.cancelled",
};

const statusIconColors: Record<string, string> = {
	pending: "text-text-muted",
	extracting: "text-blue-500 dark:text-blue-400 animate-spin",
	translating: "text-accent-fg animate-spin",
	validating: "text-warning animate-spin",
	done: "text-success",
	error: "text-danger",
	cancelled: "text-text-muted",
};

const settingsInputClass =
	"mt-1 w-full p-1.5 border border-border rounded text-body bg-surface text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg";
const settingsLabelClass =
	"text-caption font-medium text-text-muted";

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
	const [maxConcurrent, setMaxConcurrent] = useState(
		translationParams?.options.max_concurrent ?? initialDefaults.maxConcurrent,
	);
	const [costLimit, setCostLimit] = useState(() => {
		const fromParams = translationParams?.options.cost_limit_usd;
		if (typeof fromParams === "number" && Number.isFinite(fromParams)) {
			return String(fromParams);
		}
		return initialDefaults.costLimit;
	});
	const [fallbackIds, setFallbackIds] = useState<string[]>(() =>
		translationParams?.fallback_provider_ids?.length
			? [...translationParams.fallback_provider_ids]
			: readTranslationFallbacks(),
	);
	const [fallbackPick, setFallbackPick] = useState("");
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
		setMaxConcurrent(d.maxConcurrent);
		setCostLimit(d.costLimit);
		setFallbackIds(readTranslationFallbacks());
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

	const buildParams = (): TranslationStartParams =>
		buildTranslationStartParams({
			providerId,
			fallbackIds,
			sourceLang,
			targetLang,
			batchSize,
			maxConcurrent,
			costLimit,
			gameContext,
		});

	const handleStart = () => {
        let params: TranslationStartParams;
        try { params = buildParams(); }
        catch (error) {
            addToast("error", localizeApiError(error instanceof Error ? error.message : String(error)));
            return;
        }
		saveLastUsedTranslationPrefs({
			provider: providerId,
			source: sourceLang,
			target: targetLang,
			batchSize,
			maxConcurrent,
			costLimit,
		});
		saveTranslationFallbacks(fallbackIds);
		setParams(params);
		startQueue();
	};

	const addFallback = () => {
		if (
			!fallbackPick ||
			fallbackPick === providerId ||
			fallbackIds.includes(fallbackPick)
		)
			return;
		setFallbackIds((prev) => [...prev, fallbackPick]);
		setFallbackPick("");
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
				<div className="flex items-center justify-between p-4 border-b border-border">
					<h2 {...titleProps} className="font-bold text-section">
						{t("queue.title")}
					</h2>
					<div className="flex items-center gap-2">
						{doneCount > 0 && (
							<button
								type="button"
								onClick={clearCompleted}
								className="text-caption text-text-muted hover:text-text focus:outline-none focus:ring-2 focus:ring-accent-fg rounded px-1"
							>
								{t("queue.clearCompleted", { count: doneCount })}
							</button>
						)}
						<button
							type="button"
							onClick={() => setPanelOpen(false)}
							aria-label={t("queue.closeAria")}
							className="text-text-muted hover:text-text focus:outline-none focus:ring-2 focus:ring-accent-fg rounded p-0.5"
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
						<div className="divide-y divide-border border-b border-border">
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
								className="flex items-center gap-2 px-3 py-2 text-body border border-dashed border-border rounded-lg hover:bg-surface-muted transition-colors"
							>
								<File size={14} />
								{t("queue.addFile")}
							</button>
							<button
								onClick={handleAddFolder}
								className="flex items-center gap-2 px-3 py-2 text-body border border-dashed border-border rounded-lg hover:bg-surface-muted transition-colors"
							>
								<FolderOpen size={14} />
								{t("queue.addFolder")}
							</button>
						</div>
					)}

					{/* Translation settings */}
					{!isRunning && items.length > 0 && (
						<div className="p-4 border-t border-border space-y-3 bg-surface-muted">
							<h3 className="text-caption font-semibold text-text-muted uppercase">
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
							<div className="grid grid-cols-3 gap-3">
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
									<label className={settingsLabelClass}>{t("queue.maxConcurrent")}</label>
									<input
										type="number"
										value={maxConcurrent}
										onChange={(e) => setMaxConcurrent(+e.target.value)}
										min={1}
										max={16}
										className={settingsInputClass}
									/>
								</div>
								<div>
									<label className={settingsLabelClass}>{t("queue.costLimit")}</label>
									<input
										type="number"
										step="0.01"
										value={costLimit}
										onChange={(e) => setCostLimit(e.target.value)}
										placeholder={t("common.optional")}
										className={settingsInputClass}
									/>
								</div>
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
							<div>
								<label className={settingsLabelClass}>{t("queue.fallbacks")}</label>
								<p className="text-caption text-text-muted mt-0.5">
									{t("queue.fallbacksHint")}
								</p>
								<div className="mt-1 flex gap-2">
									<select
										value={fallbackPick}
										onChange={(e) => setFallbackPick(e.target.value)}
										className={settingsInputClass + " flex-1"}
									>
										<option value="">{t("common.optional")}</option>
										{providers
											?.filter(
												(p) =>
													p.id !== providerId && !fallbackIds.includes(p.id),
											)
											.map((p) => (
												<option key={p.id} value={p.id}>
													{formatProviderOptionLabel(p)}
												</option>
											))}
									</select>
									<button
										type="button"
										onClick={addFallback}
										disabled={!fallbackPick}
										className="px-2 py-1.5 text-caption border border-border bg-surface text-text hover:bg-surface-muted rounded disabled:opacity-50"
									>
										{t("queue.addFallback")}
									</button>
								</div>
								{fallbackIds.length > 0 && (
									<ul className="mt-1 flex flex-wrap gap-1">
										{fallbackIds.map((id) => (
											<li
												key={id}
												className="text-caption px-1.5 py-0.5 rounded bg-surface text-text flex items-center gap-1"
											>
												{id}
												<button
													type="button"
													onClick={() =>
														setFallbackIds((prev) => prev.filter((x) => x !== id))
													}
													className="opacity-70 hover:opacity-100"
													aria-label={t("queue.removeFallback", { id })}
												>
													×
												</button>
											</li>
										))}
									</ul>
								)}
							</div>
						</div>
					)}
				</div>

				{/* Footer */}
				<div className="p-4 border-t border-border">
					{!providerReadiness.ready &&
						providerReadiness.reason === "missing_key" &&
						!isRunning &&
						pendingCount > 0 && (
							<div className="mb-3 p-2 rounded border border-warning bg-warning-muted text-body text-warning">
								{t("queue.needsKey")}{" "}
								<button
									type="button"
									onClick={() => {
										setPanelOpen(false);
										navigate(buildSettingsPath("providers"));
									}}
									className="font-medium text-accent-fg hover:underline"
								>
									{t("queue.openSettings")}
								</button>
							</div>
						)}
					<div className="flex items-center justify-between gap-4">
						<div className="text-body text-text-muted tabular-nums">
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
								className="flex items-center gap-2 px-4 py-2 bg-red-600 hover:bg-red-700 text-white rounded-lg text-body font-medium transition-colors focus:outline-none focus:ring-2 focus:ring-danger focus:ring-offset-2 focus:ring-offset-surface"
							>
								<Square size={14} aria-hidden="true" />
								{t("queue.cancel")}
							</button>
						) : (
							<button
								type="button"
								onClick={handleStart}
								disabled={pendingCount === 0}
								className="flex items-center gap-2 px-4 py-2 bg-accent hover:bg-accent-hover disabled:opacity-50 disabled:cursor-not-allowed text-white rounded-lg text-body font-medium transition-colors focus:outline-none focus:ring-2 focus:ring-accent-fg focus:ring-offset-2 focus:ring-offset-surface"
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
					<div className="text-body font-medium truncate text-text">
						{item.projectName}
					</div>
					<span className="shrink-0 text-caption font-semibold uppercase tracking-wide text-text-muted">
						{t(STATUS_LABEL_KEYS[item.status])}
					</span>
				</div>
				<div className="text-caption text-text-muted truncate">
					{item.projectPath}
				</div>
				<div className="mt-1 h-4 flex items-center gap-2">
					{item.status === "validating" ? (
						<span className="text-caption text-warning">
							{t("queue.validatingTranslations")}
						</span>
					) : showProgress && item.progress.total > 0 ? (
						<>
							<div className="flex-1 h-1.5 bg-border rounded-full overflow-hidden">
								<div
									className={clsx(
										"h-full rounded-full transition-all duration-300",
										item.status === "done"
											? "bg-accent"
											: "bg-accent",
									)}
									style={{
										width: `${item.status === "done" ? 100 : percent}%`,
									}}
								/>
							</div>
							<span className="text-caption text-text-muted tabular-nums w-24 text-right shrink-0">
								{item.progress.completed}/{item.progress.total} ·{" "}
								{item.status === "done" ? 100 : percent}%
							</span>
						</>
					) : item.status === "extracting" ? (
						<span className="text-caption text-blue-600 dark:text-blue-400">
							{t("queue.extractingStrings")}
						</span>
					) : null}
				</div>
				{item.status === "done" && item.validationError && (
					<div
						className="text-caption text-warning mt-0.5 truncate"
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
									? "text-caption text-warning mt-0.5"
									: "text-caption text-text-muted mt-0.5"
							}
						>
							{item.validationIssues > 0
								? t("queue.validation.issues", { count: item.validationIssues })
								: t("queue.validation.clean")}
						</div>
					)}
				{item.error && (
					<div
						className="text-caption text-danger mt-0.5 truncate"
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
						className="p-1 text-text-muted hover:text-text disabled:opacity-30 focus:outline-none focus:ring-2 focus:ring-accent-fg rounded"
					>
						<ChevronUp size={14} />
					</button>
					<button
						type="button"
						onClick={onMoveDown}
						disabled={index === total - 1}
						aria-label={t("queue.moveDown")}
						className="p-1 text-text-muted hover:text-text disabled:opacity-30 focus:outline-none focus:ring-2 focus:ring-accent-fg rounded"
					>
						<ChevronDown size={14} />
					</button>
					<button
						type="button"
						onClick={onRemove}
						aria-label={t("queue.remove")}
						className="p-1 text-text-muted hover:text-danger focus:outline-none focus:ring-2 focus:ring-danger rounded"
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
					className="shrink-0 px-2 py-1 text-caption font-medium text-text-muted hover:text-text border border-border rounded focus:outline-none focus:ring-2 focus:ring-accent-fg"
				>
					{t("common.dismiss")}
				</button>
			)}
		</div>
	);
}
