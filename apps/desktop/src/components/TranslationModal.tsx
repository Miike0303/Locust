import { useState, useEffect, useRef } from "react";
import { X } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import {
	getProviders,
	getConfig,
	startTranslation,
	cancelTranslation,
	checkProviderHealth,
} from "../lib/api";
import {
	JOB_STREAM_LOST_MESSAGE,
	jobStreamCloseAction,
	subscribeToJob,
} from "../lib/ws";
import {
	resolveTranslationDefaults,
	coerceProviderId,
	readLastUsedTranslationPrefs,
	saveLastUsedTranslationPrefs,
} from "../lib/translationDefaults";
import { LANGUAGES } from "../lib/languages";
import { useEditorStore } from "../stores/editorStore";
import { useQueueStore } from "../stores/queueStore";
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

interface TranslationModalProps {
	open: boolean;
	onClose: () => void;
	totalPending: number;
	onComplete: () => void;
	onReview?: () => void;
}

const FALLBACK_STORAGE_KEY = "locust.translation.fallbacks";

export default function TranslationModal({
	open,
	onClose,
	totalPending,
	onComplete,
	onReview,
}: TranslationModalProps) {
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
	const { setJob, setTranslating } = useEditorStore();
	const { dialogRef, dialogProps, titleProps } = useModalA11y({
		open,
		ownEscape: false,
	});

	const [providerId, setProviderId] = useState("");
	const savedFallbacks: string[] = (() => {
		try {
			const v = JSON.parse(localStorage.getItem(FALLBACK_STORAGE_KEY) || "[]");
			return Array.isArray(v)
				? v.filter((x: unknown) => typeof x === "string")
				: [];
		} catch {
			return [];
		}
	})();
	const [sourceLang, setSourceLang] = useState("auto");
	const [targetLang, setTargetLang] = useState("es");
	const [fallbackIds, setFallbackIds] = useState<string[]>(savedFallbacks);
	const [fallbackPick, setFallbackPick] = useState("");
	const [batchSize, setBatchSize] = useState(40);
	const [maxConcurrent, setMaxConcurrent] = useState(1);
	const [costLimit, setCostLimit] = useState("");
	const [gameContext, setGameContext] = useState("");
	const [useGlossary, setUseGlossary] = useState(true);
	const [useMemory, setUseMemory] = useState(true);

	// Progress state
	const [step, setStep] = useState<"configure" | "progress">("configure");
	const [completed, setCompleted] = useState(0);
	const [total, setTotal] = useState(0);
	const [costSoFar, setCostSoFar] = useState(0);
	const [lastTranslated, setLastTranslated] = useState("");
	const [activeProviderLabel, setActiveProviderLabel] = useState("");
	const [done, setDone] = useState(false);
	const [cancelled, setCancelled] = useState(false);
	const [cancelling, setCancelling] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [testingHealth, setTestingHealth] = useState(false);
	const [healthResult, setHealthResult] = useState<{
		ok: boolean;
		message: string;
	} | null>(null);

	// Live job bookkeeping (refs so WS callbacks see current values)
	const unsubRef = useRef<(() => void) | null>(null);
	const jobIdRef = useRef<string | null>(null);
	const finishedRef = useRef(false);
	const cancelRequestedRef = useRef(false);

	useEffect(() => {
		if (open) {
			setStep("configure");
			setCompleted(0);
			setTotal(0);
			setCostSoFar(0);
			setDone(false);
			setCancelled(false);
			setCancelling(false);
			setError(null);
			setLastTranslated("");
			setActiveProviderLabel("");
			jobIdRef.current = null;
			finishedRef.current = false;
			cancelRequestedRef.current = false;
		}
	}, [open]);

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
		);
		setProviderId(coerceProviderId(d.providerId, providers, config));
		setSourceLang(d.sourceLang);
		setTargetLang(d.targetLang);
		setBatchSize(d.batchSize);
		setCostLimit(d.costLimit);
	}, [open, config, configFetched, configError, providers]);

	// If the current provider id is missing or not ready, fall back to the first ready one.
	useEffect(() => {
		if (!providers || providers.length === 0) return;
		setProviderId((prev) => coerceProviderId(prev, providers, config));
	}, [providers, config]);

	useEffect(() => {
		setHealthResult(null);
	}, [providerId]);

	// Clean up the progress WebSocket on modal close / unmount.
	useEffect(() => {
		if (!open) {
			unsubRef.current?.();
			unsubRef.current = null;
		}
	}, [open]);
	useEffect(
		() => () => {
			unsubRef.current?.();
			unsubRef.current = null;
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
			costLimit,
		});
		try {
			localStorage.setItem(FALLBACK_STORAGE_KEY, JSON.stringify(fallbackIds));
		} catch {}
		const chainLabel = [providerId, ...fallbackIds].join(" → ");
		addLog(
			"info",
			`Starting translation with chain: ${chainLabel}, source: ${sourceLang}, target: ${targetLang}, batch: ${batchSize}`,
			undefined,
			"translation",
		);
		try {
			const params = {
				provider_id: providerId,
				...(fallbackIds.length > 0
					? { fallback_provider_ids: fallbackIds }
					: {}),
				options: {
					source_lang: sourceLang,
					target_lang: targetLang,
					batch_size: batchSize,
					max_concurrent: maxConcurrent,
					cost_limit_usd: costLimit ? parseFloat(costLimit) : null,
					game_context: gameContext || null,
					use_glossary: useGlossary,
					use_memory: useMemory,
					skip_approved: true,
				},
			};
			addLog(
				"info",
				`Calling startTranslation API...`,
				JSON.stringify(params, null, 2),
				"translation",
			);
			const result = await startTranslation(params);
			addLog("info", `Got job_id: ${result.job_id}`, undefined, "translation");

			setJob(result.job_id);
			jobIdRef.current = result.job_id;
			finishedRef.current = false;
			cancelRequestedRef.current = false;
			setTranslating(true);
			setStep("progress");
			setActiveProviderLabel(
				providers?.find((p) => p.id === providerId)?.name ?? providerId,
			);
			addLog(
				"info",
				`Translation started (${chainLabel}), subscribing to WebSocket...`,
				undefined,
				"translation",
			);

			const projectName = useProjectStore.getState().project?.name ?? "Project";

			unsubRef.current = subscribeToJob(result.job_id, {
				onStarted: (e) => {
					setTotal(e.total);
					useQueueStore.getState().setGlobalProgress({
						projectName,
						completed: 0,
						total: e.total,
						costSoFar: 0,
						startedAt: Date.now(),
					});
				},
				onBatchCompleted: (e) => {
					setCompleted(e.completed);
					setCostSoFar(e.cost_so_far);
					useQueueStore.getState().setGlobalProgress({
						projectName,
						completed: e.completed,
						total: e.total,
						costSoFar: e.cost_so_far,
						startedAt:
							useQueueStore.getState().globalProgress?.startedAt ?? Date.now(),
					});
				},
				onStringTranslated: (e) => setLastTranslated(e.translation),
				onProviderSwitched: (e) => {
					setActiveProviderLabel(e.provider_name);
					addLog(
						"info",
						`Switched to provider ${e.provider_name} (${e.remaining_pending} still pending)`,
						undefined,
						"translation",
					);
					addToast("info", `Switched to ${e.provider_name}`);
				},
				onCompleted: (e) => {
					finishedRef.current = true;
					setDone(true);
					setTranslating(false);
					setJob(null);
					useQueueStore.getState().setGlobalProgress(null);
					addLog(
						"info",
						`Translation complete: ${e.total_translated} strings, $${e.total_cost?.toFixed(4) ?? "0"}`,
						undefined,
						"translation",
					);
					addToast(
						"success",
						`Translation complete: ${e.total_translated} strings`,
					);
				},
				onFailed: (e) => {
					finishedRef.current = true;
					setError(e.error);
					setTranslating(false);
					setJob(null);
					useQueueStore.getState().setGlobalProgress(null);
					addLog("error", `Translation failed`, e.error, "translation");
					addToast("error", `Translation failed: ${e.error}`);
				},
				onClosed: () => {
					const action = jobStreamCloseAction(
						finishedRef.current,
						cancelRequestedRef.current,
					);
					if (action === "ignore") return;
					finishedRef.current = true;
					setTranslating(false);
					setJob(null);
					useQueueStore.getState().setGlobalProgress(null);
					if (action === "cancelled") {
						setCancelled(true);
						setCancelling(false);
						addLog("info", "Translation cancelled", undefined, "translation");
						addToast("info", "Translation cancelled");
						return;
					}
					setError(JOB_STREAM_LOST_MESSAGE);
					addLog(
						"error",
						"Translation failed",
						JOB_STREAM_LOST_MESSAGE,
						"translation",
					);
					addToast("error", `Translation failed: ${JOB_STREAM_LOST_MESSAGE}`);
				},
			});
		} catch (err: any) {
			addLog(
				"error",
				`Translation start failed: ${err.message ?? err}`,
				err.stack ?? String(err),
				"translation",
			);
			addToast("error", `Translation failed to start: ${err.message ?? err}`);
			setError(err.message ?? String(err));
		}
	};

	const handleCancel = async () => {
		const jobId = jobIdRef.current;
		if (!jobId || cancelling) return;
		cancelRequestedRef.current = true;
		setCancelling(true);
		try {
			await cancelTranslation(jobId);
			addLog(
				"info",
				`Cancel requested for job ${jobId}`,
				undefined,
				"translation",
			);
			addToast("info", "Cancelling translation…");
		} catch (err: any) {
			cancelRequestedRef.current = false;
			setCancelling(false);
			addToast("error", `Cancel failed: ${err.message ?? err}`);
		}
	};

	const handleClose = () => {
		onClose();
		if (done || (step === "progress" && error)) onComplete();
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

	const progressPercent = total > 0 ? (completed / total) * 100 : 0;

	return (
		<div className={MODAL_BACKDROP_CLASS}>
			<div
				ref={dialogRef}
				{...dialogProps}
				className={modalPanelClass("max-w-lg p-6")}
			>
				<div className="flex justify-between items-center mb-4">
					<h2 {...titleProps} className="text-lg font-bold">
						{step === "configure"
							? "Start Translation"
							: "Translation Progress"}
					</h2>
					<button
						onClick={handleClose}
						aria-label="Close translation dialog"
						className="text-gray-400 hover:text-gray-600"
					>
						<X aria-hidden="true" size={20} />
					</button>
				</div>

				{step === "configure" && (
					<div className="space-y-4">
						<div>
							<div className="flex items-end justify-between gap-2">
								<label className="text-sm font-medium">Provider</label>
								<button
									type="button"
									onClick={handleTestConnection}
									disabled={!providerId || testingHealth}
									className="text-xs font-medium text-emerald-700 dark:text-emerald-400 hover:underline disabled:opacity-50 disabled:cursor-not-allowed"
								>
									{testingHealth ? "Testing…" : "Test connection"}
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
								className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
							>
								{providers?.map((p) => (
									<option key={p.id} value={p.id}>
										{formatProviderOptionLabel(p)}
									</option>
								))}
							</select>
							{!providerReadiness.ready &&
								providerReadiness.reason === "missing_key" && (
									<div className="mt-2 p-2 rounded border border-amber-200 bg-amber-50 dark:border-amber-800 dark:bg-amber-900/20 text-sm text-amber-800 dark:text-amber-200">
										This provider needs an API key — add it in Settings.{" "}
										<button
											type="button"
											onClick={() => openSettings("provider-settings")}
											className="font-medium text-emerald-700 dark:text-emerald-400 hover:underline"
										>
											Open Settings
										</button>
									</div>
								)}
							{healthResult && (
								<div
									className={`mt-2 p-2 rounded border text-sm ${
										healthResult.ok
											? "border-emerald-200 bg-emerald-50 text-emerald-800 dark:border-emerald-800 dark:bg-emerald-900/20 dark:text-emerald-200"
											: "border-red-200 bg-red-50 text-red-700 dark:border-red-800 dark:bg-red-900/20 dark:text-red-300"
									}`}
								>
									{healthResult.message}
								</div>
							)}
							<button
								type="button"
								onClick={() => openSettings("provider-settings")}
								className="mt-1 text-xs font-medium text-emerald-700 dark:text-emerald-400 hover:underline"
							>
								Provider health &amp; settings
							</button>
						</div>
						<div>
							<label className="text-sm font-medium">
								Fallback providers (optional)
							</label>
							<p className="text-xs text-gray-500 mt-0.5 mb-1">
								Tried in order if the primary stops making progress on pending
								strings.
							</p>
							<div className="flex gap-2">
								<select
									value={fallbackPick}
									onChange={(e) => setFallbackPick(e.target.value)}
									className="flex-1 p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
								>
									<option value="">Add fallback…</option>
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
									className="px-3 py-2 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded text-sm font-medium disabled:opacity-50"
								>
									Add
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
												className="flex items-center justify-between text-sm px-2 py-1 bg-gray-50 dark:bg-gray-800/60 rounded"
											>
												<span>
													<span className="text-gray-400 mr-2">{i + 1}.</span>
													{name}
												</span>
												<button
													type="button"
													onClick={() => removeFallback(id)}
													className="text-red-500 hover:text-red-700 text-xs font-medium"
												>
													Remove
												</button>
											</li>
										);
									})}
								</ol>
							)}
						</div>
						<div className="grid grid-cols-2 gap-3">
							<div>
								<label className="text-sm font-medium">Source</label>
								<select
									value={sourceLang}
									onChange={(e) => setSourceLang(e.target.value)}
									className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
								>
									<option value="auto">Auto-detect</option>
									{LANGUAGES.map((l) => (
										<option key={l.code} value={l.code}>
											{l.label}
										</option>
									))}
								</select>
							</div>
							<div>
								<label className="text-sm font-medium">Target</label>
								<select
									value={targetLang}
									onChange={(e) => setTargetLang(e.target.value)}
									className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
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
							<label className="text-sm font-medium">Game context</label>
							<textarea
								value={gameContext}
								onChange={(e) => setGameContext(e.target.value)}
								rows={2}
								placeholder="Describe genre, tone, setting..."
								className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
							/>
						</div>
						<div className="flex items-center gap-4">
							<label className="flex items-center gap-2 text-sm">
								<input
									type="checkbox"
									checked={useGlossary}
									onChange={(e) => setUseGlossary(e.target.checked)}
								/>{" "}
								Use glossary
							</label>
							<button
								type="button"
								onClick={() => openSettings("manage-glossary")}
								className="text-xs font-medium text-emerald-700 dark:text-emerald-400 hover:underline"
							>
								Manage glossary
							</button>
							<label className="flex items-center gap-2 text-sm">
								<input
									type="checkbox"
									checked={useMemory}
									onChange={(e) => setUseMemory(e.target.checked)}
								/>{" "}
								Use memory
							</label>
						</div>
						<div className="grid grid-cols-3 gap-3">
							<div>
								<label className="text-sm font-medium">Batch size</label>
								<input
									type="number"
									value={batchSize}
									onChange={(e) => setBatchSize(+e.target.value)}
									min={1}
									max={100}
									className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
								/>
							</div>
							<div>
								<label
									className="text-sm font-medium"
									title="Parallel requests. Use 1 for local models (LM Studio/Ollama); higher values only speed up remote APIs."
								>
									Parallel requests
								</label>
								<select
									value={maxConcurrent}
									onChange={(e) => setMaxConcurrent(+e.target.value)}
									className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
								>
									{Array.from({ length: 10 }, (_, i) => i + 1).map((n) => (
										<option key={n} value={n}>
											{n}
										</option>
									))}
								</select>
							</div>
							<div>
								<label className="text-sm font-medium">Cost limit ($)</label>
								<input
									value={costLimit}
									onChange={(e) => setCostLimit(e.target.value)}
									placeholder="No limit"
									className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
								/>
							</div>
						</div>
						<p className="text-sm text-gray-500">
							{totalPending} pending strings to translate
						</p>
						<button
							onClick={handleStart}
							className="w-full py-2 bg-emerald-600 hover:bg-emerald-700 text-white rounded font-medium transition-colors"
						>
							Start Translation
						</button>
					</div>
				)}

				{step === "progress" && (
					<div className="space-y-4">
						<div className="w-full bg-gray-200 dark:bg-gray-700 rounded-full h-3">
							<div
								className="bg-emerald-500 h-3 rounded-full transition-all"
								style={{ width: `${progressPercent}%` }}
							/>
						</div>
						<div className="text-center text-sm">
							{done
								? "Complete!"
								: cancelled
									? "Cancelled"
									: `${completed} / ${total}`}
							{costSoFar > 0 && ` · $${costSoFar.toFixed(4)}`}
						</div>
						{activeProviderLabel && !done && !cancelled && !error && (
							<div className="text-xs text-center text-emerald-700 dark:text-emerald-400">
								Using provider: {activeProviderLabel}
							</div>
						)}
						{lastTranslated && !done && !cancelled && !error && (
							<div className="text-xs text-gray-500 truncate">
								Last: {lastTranslated}
							</div>
						)}
						{error && (
							<div className="p-2 bg-red-50 dark:bg-red-900/20 border border-red-200 rounded text-sm text-red-600">
								{error}
							</div>
						)}
						{!done && !cancelled && !error && (
							<button
								onClick={handleCancel}
								disabled={cancelling}
								className="w-full py-2 bg-red-600 hover:bg-red-700 disabled:opacity-50 disabled:cursor-not-allowed text-white rounded font-medium transition-colors"
							>
								{cancelling ? "Cancelling…" : "Cancel"}
							</button>
						)}
						{(done || cancelled || error) && (
							<div className="flex gap-2">
								<button
									onClick={handleClose}
									className={`w-full py-2 rounded font-medium ${
										done && onReview
											? "bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700"
											: "bg-emerald-600 hover:bg-emerald-700 text-white"
									}`}
								>
									Close
								</button>
								{done && onReview && (
									<button
										onClick={handleReview}
										className="w-full py-2 bg-emerald-600 hover:bg-emerald-700 text-white rounded font-medium"
									>
										Review translations
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
