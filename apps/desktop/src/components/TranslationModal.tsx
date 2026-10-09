import { localizeApiError } from "../lib/apiError";
import { formatObservedCost } from "../lib/translationCost";
import { useState, useEffect, useRef } from "react";
import { X } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import {
	getProviders,
	getConfig,
	checkProviderHealth,
} from "../lib/api";
import { translationModalStep } from "../lib/translationJob";
import { startSingleTranslation } from "../lib/translationStart";
import {
	clearTranslationSnapshotIfIdle,
	requestTranslationCancel,
	setTranslationModalOpen,
} from "../lib/translationJobSession";
import {
	detectSourceLanguage,
	resolveTranslationDefaults,
	coerceProviderId,
	readLastUsedTranslationPrefs,
	saveLastUsedTranslationPrefs,
	readTranslationFallbacks,
	saveTranslationFallbacks,
	buildTranslationStartParams,
} from "../lib/translationDefaults";
import { LANGUAGES } from "../lib/languages";
import { useEditorStore } from "../stores/editorStore";
import { useProjectStore } from "../stores/projectStore";
import { addLog } from "../stores/logStore";
import { addToast } from "../stores/toastStore";
import {
	useModalA11y,
	MODAL_BACKDROP_CLASS,
	modalPanelClass,
} from "../lib/modalA11y";
import {
	operationalShortcutTarget,
	type OperationalShortcut,
} from "../lib/settingsNav";
import { resolveProviderReadiness, formatProviderOptionLabel } from "../lib/providerReadiness";
import { useLocale, useT } from "../lib/i18n";

interface TranslationModalProps {
	open: boolean;
	onClose: () => void;
	totalPending: number;
	onComplete: () => void;
	onReview?: () => void;
	/** Source strings of the open project, used to guess its language. */
	sampleSources?: readonly string[];
}

export default function TranslationModal({
	open,
	onClose,
	totalPending,
	onComplete,
	onReview,
	sampleSources,
}: TranslationModalProps) {
	const t = useT();
	const { locale } = useLocale();
	const navigate = useNavigate();
	const { data: providers } = useQuery({
		queryKey: ["providers"],
		queryFn: getProviders,
		enabled: open,
	});
	const {
		data: config,
		isFetched: configFetched,
		isError: configError,
	} = useQuery({
		queryKey: ["config"],
		queryFn: getConfig,
		enabled: open,
	});
	const { isTranslating, jobSnapshot } = useEditorStore();
	const { dialogRef, dialogProps, titleProps } = useModalA11y({
		open,
		ownEscape: false,
	});

	const [providerId, setProviderId] = useState("");
	const [sourceLang, setSourceLang] = useState("auto");
	const [targetLang, setTargetLang] = useState("es");
	const [fallbackIds, setFallbackIds] = useState<string[]>(() =>
		readTranslationFallbacks(),
	);
	const [fallbackPick, setFallbackPick] = useState("");
	const [batchSize, setBatchSize] = useState(40);
	const [maxConcurrent, setMaxConcurrent] = useState(1);
	const [costLimit, setCostLimit] = useState("");
	const [gameContext, setGameContext] = useState("");
	const [useGlossary, setUseGlossary] = useState(true);
	const [useMemory, setUseMemory] = useState(true);

	// Progress state
	const [testingHealth, setTestingHealth] = useState(false);
	const [healthResult, setHealthResult] = useState<{
		ok: boolean;
		message: string;
	} | null>(null);

	const [startError, setStartError] = useState<string | null>(null);

	// Apply resolved defaults (last-used > Settings config > fallbacks) once per open,
	// as soon as the config query settles (success or error — dead backend keeps fallbacks).
	const defaultsAppliedRef = useRef(false);
	useEffect(() => {
		if (!open) {
			defaultsAppliedRef.current = false;
			return;
		}
		if (defaultsAppliedRef.current || !(configFetched || configError)) return;
		defaultsAppliedRef.current = true;
		const d = resolveTranslationDefaults(
			config,
			readLastUsedTranslationPrefs(),
			{
				detectedSource: detectSourceLanguage(sampleSources ?? []),
				uiLocale: locale,
			},
		);
		setProviderId(coerceProviderId(d.providerId, providers, config));
		setSourceLang(d.sourceLang);
		setTargetLang(d.targetLang);
		setBatchSize(d.batchSize);
		setMaxConcurrent(d.maxConcurrent);
		setCostLimit(d.costLimit);
		setFallbackIds(readTranslationFallbacks());
	}, [open, config, configFetched, configError, providers, sampleSources, locale]);

	// If the current provider id is missing or not ready, fall back to the first ready one.
	useEffect(() => {
		if (!providers || providers.length === 0) return;
		setProviderId((prev) => coerceProviderId(prev, providers, config));
	}, [providers, config]);

	useEffect(() => {
		setHealthResult(null);
	}, [providerId]);

	useEffect(() => {
		setTranslationModalOpen(open);
		if (!open) {
			clearTranslationSnapshotIfIdle();
		} else {
			setStartError(null);
		}
	}, [open]);
	useEffect(
		() => () => {
			setTranslationModalOpen(false);
		},
		[],
	);

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

	const removeFallback = (id: string) => {
		setFallbackIds((prev) => prev.filter((x) => x !== id));
	};

	const handleStart = async () => {
		// Persist last-used selection for next time
		saveLastUsedTranslationPrefs({
			provider: providerId,
			source: sourceLang,
			target: targetLang,
			batchSize,
			maxConcurrent,
			costLimit,
		});
		saveTranslationFallbacks(fallbackIds);
		const chainLabel = [providerId, ...fallbackIds].join(" → ");
		addLog(
			"info",
			t("activity.translation.starting", { chain: chainLabel, source: sourceLang, target: targetLang, count: batchSize }),
			undefined,
			"translation",
		);
		try {
			const base = buildTranslationStartParams({
				providerId,
				fallbackIds,
				sourceLang,
				targetLang,
				batchSize,
				maxConcurrent,
				costLimit,
				gameContext,
			});
			const params = {
				...base,
				options: {
					...base.options,
					use_glossary: useGlossary,
					use_memory: useMemory,
				},
			};
			const projectName =
				useProjectStore.getState().project?.name ?? "Project";
			const providerLabel =
				providers?.find((p) => p.id === providerId)?.name ?? providerId;
			const result = await startSingleTranslation(params, { projectName, providerLabel });
			if (!result) return;
			addLog(
				"info",
				t("activity.translation.started", { chain: chainLabel }),
				undefined,
				"translation",
			);
		} catch (err: any) {
			addLog(
				"error",
				t("activity.translation.startFailed", { error: String(err.message ?? err) }),
				err.stack ?? String(err),
				"translation",
			);
			addToast("error", t("translate.toast.startFailed", { error: localizeApiError(err.message ?? String(err)) }));
			setStartError(localizeApiError(err.message ?? String(err)));
		}
	};

	const handleCancel = async () => {
		if (jobSnapshot?.cancelling) return;
		await requestTranslationCancel();
	};

	const handleClose = () => {
		onClose();
		if (jobSnapshot?.done || jobSnapshot?.error) onComplete();
	};

	const openSettings = (shortcut: OperationalShortcut) => {
		handleClose();
		navigate(operationalShortcutTarget(shortcut).path);
	};

	const providerReadiness = resolveProviderReadiness(
		providerId,
		providers,
		config,
	);

	const handleTestConnection = async () => {
		if (!providerId || testingHealth) return;
		setTestingHealth(true);
		setHealthResult(null);
		try {
			const result = await checkProviderHealth(providerId);
			setHealthResult(result);
		} catch (err: unknown) {
			const message = err instanceof Error ? err.message : String(err);
			setHealthResult({ ok: false, message });
		} finally {
			setTestingHealth(false);
		}
	};

	const handleReview = () => {
		onClose();
		onComplete();
		onReview?.();
	};

	if (!open) return null;

	const step = translationModalStep({
		isTranslating,
		snapshot: jobSnapshot,
	});
	const done = jobSnapshot?.done ?? false;
	const cancelled = jobSnapshot?.cancelled ?? false;
	const cancelling = jobSnapshot?.cancelling ?? false;
	const error = jobSnapshot?.error ?? null;
	const completed = jobSnapshot?.completed ?? 0;
	const total = jobSnapshot?.total ?? 0;
	const costSoFar = jobSnapshot?.costSoFar ?? 0;
	const lastTranslated = jobSnapshot?.lastTranslated ?? "";
	const activeProviderLabel = jobSnapshot?.activeProviderLabel ?? "";
	const progressPercent = total > 0 ? (completed / total) * 100 : 0;

	return (
		<div className={MODAL_BACKDROP_CLASS}>
			<div
				ref={dialogRef}
				{...dialogProps}
				className={modalPanelClass("max-w-lg p-6")}
			>
				<div className="flex justify-between items-center mb-4">
					<h2 {...titleProps} className="text-section font-bold">
						{step === "configure"
							? t("translate.title")
							: t("translate.progressTitle")}
					</h2>
					<button
						onClick={handleClose}
						aria-label={t("translate.closeAria")}
						className="text-text-muted hover:text-text"
					>
						<X aria-hidden="true" size={20} />
					</button>
				</div>

				{step === "configure" && (
					<div className="space-y-4">
						<div>
							<div className="flex items-end justify-between gap-2">
								<label className="text-body font-medium">{t("translate.provider")}</label>
								<button
									type="button"
									onClick={handleTestConnection}
									disabled={!providerId || testingHealth}
									className="text-caption font-medium text-accent-fg hover:underline disabled:opacity-50 disabled:cursor-not-allowed"
								>
									{testingHealth ? t("translate.testing") : t("translate.testConnection")}
								</button>
							</div>
							<select
								value={providerId}
								onChange={(e) => {
									setProviderId(e.target.value);
									setFallbackIds((prev) =>
										prev.filter((id) => id !== e.target.value),
									);
								}}
								className="mt-1 w-full p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
							>
								{providers?.map((p) => (
									<option key={p.id} value={p.id}>
										{formatProviderOptionLabel(p)}
									</option>
								))}
							</select>
							{!providerReadiness.ready &&
								providerReadiness.reason === "missing_key" && (
									<div className="mt-2 p-2 rounded border border-warning bg-warning-muted text-body text-warning">
										{t("translate.needsKey")}{" "}
										<button
											type="button"
											onClick={() => openSettings("provider-settings")}
											className="font-medium text-accent-fg hover:underline"
										>
											{t("translate.openSettings")}
										</button>
									</div>
								)}
							{healthResult && (
								<div
									className={`mt-2 p-2 rounded border text-body ${
										healthResult.ok
											? "border-success bg-success-muted text-success"
											: "border-danger bg-danger-muted text-danger"
									}`}
								>
									{healthResult.message}
								</div>
							)}
							<button
								type="button"
								onClick={() => openSettings("provider-settings")}
								className="mt-1 text-caption font-medium text-accent-fg hover:underline"
							>
								{t("translate.providerSettings")}
							</button>
						</div>
						<div>
							<label className="text-body font-medium">
								{t("translate.fallbacks")}
							</label>
							<p className="text-caption text-text-muted mt-0.5 mb-1">
								{t("translate.fallbacksHint")}
							</p>
							<div className="flex gap-2">
								<select
									value={fallbackPick}
									onChange={(e) => setFallbackPick(e.target.value)}
									className="flex-1 p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
								>
									<option value="">{t("translate.addFallback")}</option>
									{providers
										?.filter(
											(p) => p.id !== providerId && !fallbackIds.includes(p.id),
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
									className="px-3 py-2 bg-surface-muted text-text hover:bg-surface rounded text-body font-medium disabled:opacity-50"
								>
									{t("common.add")}
								</button>
							</div>
							{fallbackIds.length > 0 && (
								<ol className="mt-2 space-y-1">
									{fallbackIds.map((id, i) => {
										const name =
											providers?.find((p) => p.id === id)?.name ?? id;
										return (
											<li
												key={id}
												className="flex items-center justify-between text-body px-2 py-1 bg-surface-muted rounded"
											>
												<span>
													<span className="text-text-muted mr-2">{i + 1}.</span>
													{name}
												</span>
												<button
													type="button"
													onClick={() => removeFallback(id)}
													className="text-danger hover:underline text-caption font-medium"
												>
													{t("common.remove")}
												</button>
											</li>
										);
									})}
								</ol>
							)}
						</div>
						<div className="grid grid-cols-2 gap-3">
							<div>
								<label className="text-body font-medium">{t("translate.source")}</label>
								<select
									value={sourceLang}
									onChange={(e) => setSourceLang(e.target.value)}
									className="mt-1 w-full p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
								>
									<option value="auto">{t("translate.autoDetect")}</option>
									{LANGUAGES.map((l) => (
										<option key={l.code} value={l.code}>
											{l.label}
										</option>
									))}
								</select>
							</div>
							<div>
								<label className="text-body font-medium">{t("translate.target")}</label>
								<select
									value={targetLang}
									onChange={(e) => setTargetLang(e.target.value)}
									className="mt-1 w-full p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
								>
									{LANGUAGES.map((l) => (
										<option key={l.code} value={l.code}>
											{l.label}
										</option>
									))}
								</select>
							</div>
						</div>
						<div>
							<label className="text-body font-medium">{t("translate.gameContext")}</label>
							<textarea
								value={gameContext}
								onChange={(e) => setGameContext(e.target.value)}
								rows={2}
								placeholder={t("translate.gameContextPlaceholder")}
								className="mt-1 w-full p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
							/>
						</div>
						<div className="flex items-center gap-4">
							<label className="flex items-center gap-2 text-body">
								<input
									type="checkbox"
									checked={useGlossary}
									onChange={(e) => setUseGlossary(e.target.checked)}
								/>{" "}
								{t("translate.useGlossary")}
							</label>
							<button
								type="button"
								onClick={() => openSettings("manage-glossary")}
								className="text-caption font-medium text-accent-fg hover:underline"
							>
								{t("translate.manageGlossary")}
							</button>
							<label className="flex items-center gap-2 text-body">
								<input
									type="checkbox"
									checked={useMemory}
									onChange={(e) => setUseMemory(e.target.checked)}
								/>{" "}
								{t("translate.useMemory")}
							</label>
						</div>
                        {(providerId === "grok" || providerId === "grok-sub") && (
                            <button type="button" className="text-body text-accent-fg underline"
                                onClick={() => { setBatchSize(10); setMaxConcurrent(10); }}>
                                {t("translate.grokThroughputPreset")}
                            </button>
                        )}
						<div className="grid grid-cols-3 gap-3">
							<div>
								<label className="text-body font-medium">{t("translate.batchSize")}</label>
								<input
									type="number"
									value={batchSize}
									onChange={(e) => setBatchSize(+e.target.value)}
									min={1}
									max={100}
									className="mt-1 w-full p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
								/>
							</div>
							<div>
								<label
									className="text-body font-medium"
									title={t("translate.parallelTitle")}
								>
									{t("translate.parallelRequests")}
								</label>
								<select
									value={maxConcurrent}
									onChange={(e) => setMaxConcurrent(+e.target.value)}
									className="mt-1 w-full p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
								>
									{Array.from({ length: 10 }, (_, i) => i + 1).map((n) => (
										<option key={n} value={n}>
											{n}
										</option>
									))}
								</select>
							</div>
							<div>
								<label className="text-body font-medium">{t("translate.costLimit")}</label>
								<input
									value={costLimit}
									onChange={(e) => setCostLimit(e.target.value)}
									placeholder={t("translate.noLimit")}
									className="mt-1 w-full p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
								/>
							</div>
						</div>
						<p className="text-body text-text-muted">
							{t("translate.pending", { count: totalPending })}
						</p>
						{startError && (
							<div className="p-2 bg-danger-muted border border-danger rounded text-body text-danger">
								{startError}
							</div>
						)}
						<button
							onClick={handleStart}
							className="w-full py-2 bg-accent hover:bg-accent-hover text-white rounded font-medium transition-colors"
						>
							{t("translate.start")}
						</button>
					</div>
				)}

				{step === "progress" && (
					<div className="space-y-4">
						<div className="w-full bg-border rounded-full h-3">
							<div
								className="bg-accent h-3 rounded-full transition-all"
								style={{ width: `${progressPercent}%` }}
							/>
						</div>
						<div className="text-center text-body">
							{done
								? t("translate.complete")
								: cancelled
									? t("translate.cancelled")
									: `${completed} / ${total}`}
							{` · ${formatObservedCost(costSoFar, jobSnapshot?.costIsComplete, t)}`}
						</div>
						{activeProviderLabel && !done && !cancelled && !error && (
							<div className="text-caption text-center text-accent-fg">
								{t("translate.usingProvider", { name: activeProviderLabel })}
							</div>
						)}
						{lastTranslated && !done && !cancelled && !error && (
							<div className="text-caption text-text-muted truncate">
								{t("translate.last", { text: lastTranslated })}
							</div>
						)}
						{error && (
							<div className="p-2 bg-danger-muted border border-danger rounded text-body text-danger">
								{error}
							</div>
						)}
						{!done && !cancelled && !error && (
							<button
								onClick={handleCancel}
								disabled={cancelling}
								className="w-full py-2 bg-red-600 hover:bg-red-700 disabled:opacity-50 disabled:cursor-not-allowed text-white rounded font-medium transition-colors"
							>
								{cancelling ? t("translate.cancelling") : t("translate.cancel")}
							</button>
						)}
						{(done || cancelled || error) && (
							<div className="flex gap-2">
								<button
									onClick={handleClose}
									className={`w-full py-2 rounded font-medium ${
										done && onReview
											? "bg-surface-muted text-text hover:bg-surface"
											: "bg-accent hover:bg-accent-hover text-white"
									}`}
								>
									{t("common.close")}
								</button>
								{done && onReview && (
									<button
										onClick={handleReview}
										className="w-full py-2 bg-accent hover:bg-accent-hover text-white rounded font-medium"
									>
										{t("translate.review")}
									</button>
								)}
							</div>
						)}
					</div>
				)}
			</div>
		</div>
	);
}
