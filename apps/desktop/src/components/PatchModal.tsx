import { useEffect, useRef, useState } from "react";
import {
	X,
	FolderOpen,
	FileArchive,
	Package,
	RotateCcw,
	AlertCircle,
	ShieldCheck,
	Archive,
	Loader2,
} from "lucide-react";
import {
	getConfig,
	cancelPatchApply,
	patchApply,
	patchPack,
	patchRollback,
	patchStatus,
	patchVerify,
	type PatchApplyResult,
	type PatchPackResult,
	type PatchStatusResult,
	type PatchVerifyResult,
} from "../lib/api";
import {
	isHttpPatchUrl,
	loadRememberedPatchSource,
	patchSourceReady,
	patchUrlLooksLikeZip,
	rememberPatchSource,
	resolvePatchSource,
} from "../lib/patchSource";
import { subscribeToJob } from "../lib/ws";
import { addLog } from "../stores/logStore";
import { addToast } from "../stores/toastStore";
import {
	useModalA11y,
	MODAL_BACKDROP_CLASS,
	modalPanelClass,
} from "../lib/modalA11y";
import { useT } from "../lib/i18n";

const IS_TAURI = "__TAURI_INTERNALS__" in window;

type Tab = "apply" | "pack";

interface PatchModalProps {
	open: boolean;
	onClose: () => void;
	/** Optional default game path (e.g. current project folder). */
	defaultGamePath?: string;
	/** Open on Apply or Pack (e.g. after direct inject). */
	initialTab?: Tab;
	/** Called after apply/rollback refresh so ambient indicators can update. */
	onPatchStateChanged?: () => void;
	/** Pack needs an inject recording from an open project. Hide when none. */
	allowPack?: boolean;
}

export default function PatchModal({
	open,
	onClose,
	defaultGamePath,
	initialTab = "apply",
	onPatchStateChanged,
	allowPack = true,
}: PatchModalProps) {
	const t = useT();
	const remembered = (() => {
		try {
			return loadRememberedPatchSource();
		} catch {
			return { zipPath: "", zipUrl: "" };
		}
	})();
	const [tab, setTab] = useState<Tab>(initialTab);
	const [gamePath, setGamePath] = useState(defaultGamePath ?? "");
	const [zipPath, setZipPath] = useState(remembered.zipPath);
	const [zipUrl, setZipUrl] = useState(remembered.zipUrl);
	const [outputPath, setOutputPath] = useState("");
	const [languages, setLanguages] = useState("");
	const [pristine, setPristine] = useState(false);
	const [pristinePath, setPristinePath] = useState("");
	const [force, setForce] = useState(false);
	const [confirmLegacy, setConfirmLegacy] = useState(false);
	const [dryRun, setDryRun] = useState(false);
	const [loading, setLoading] = useState(false);
	const [verify, setVerify] = useState<PatchVerifyResult | null>(null);
	const [applyResult, setApplyResult] = useState<PatchApplyResult | null>(null);
	const [packResult, setPackResult] = useState<PatchPackResult | null>(null);
	const [status, setStatus] = useState<PatchStatusResult | null>(null);
	const [error, setError] = useState<string | null>(null);
	const [applying, setApplying] = useState(false);
	const [cancellingApply, setCancellingApply] = useState(false);
	const [needsRollback, setNeedsRollback] = useState(false);
	const [applyProgress, setApplyProgress] = useState<{
		current: number;
		total: number;
		path: string;
		phase: string;
	} | null>(null);
	const applyUnsubRef = useRef<(() => void) | null>(null);
	const applyJobIdRef = useRef<string | null>(null);
	const applyFinishedRef = useRef(false);
	const applyCancelRequestedRef = useRef(false);
	const { dialogRef, dialogProps, titleProps } = useModalA11y({
		open,
		ownEscape: false,
	});

	useEffect(() => {
		if (!open) return;
		if (applyJobIdRef.current) return;
		if (defaultGamePath) setGamePath(defaultGamePath);
		setTab(allowPack ? initialTab : "apply");
		setError(null);
		// Prefill pack language from config target lang
		if (!allowPack) return;
		getConfig()
			.then((cfg) => {
				if (cfg.default_target_lang) {
					setLanguages((prev) =>
						prev.trim() ? prev : cfg.default_target_lang,
					);
				}
			})
			.catch(() => {
				/* config optional for apply tab */
			});
	}, [open, defaultGamePath, initialTab, allowPack]);

	useEffect(() => {
		if (!allowPack && tab === "pack") setTab("apply");
	}, [allowPack, tab]);

	useEffect(
		() => () => {
			applyUnsubRef.current?.();
			applyUnsubRef.current = null;
		},
		[],
	);

	const resolvedSource = resolvePatchSource(zipPath, zipUrl);
	const sourceOk = patchSourceReady(resolvedSource);
	const canVerifyApply = Boolean(gamePath.trim()) && sourceOk;
	const urlFieldError =
		zipUrl.trim() && !isHttpPatchUrl(zipUrl)
			? t("patch.urlError")
			: resolvedSource && "error" in resolvedSource
				? t(resolvedSource.error)
				: null;
	// Soft hint only — signed CDN links may omit `.zip` in the path.
	const urlZipHint =
		!urlFieldError &&
		zipUrl.trim() &&
		isHttpPatchUrl(zipUrl) &&
		!patchUrlLooksLikeZip(zipUrl)
			? t("patch.urlHint")
			: null;

	if (!open) return null;

	const pickGame = async () => {
		if (IS_TAURI) {
			const { open: openDialog } = await import("@tauri-apps/plugin-dialog");
			const selected = await openDialog({
				title:
					tab === "pack"
						? t("patch.dialog.packFolder")
						: t("patch.dialog.applyFolder"),
				directory: true,
			});
			if (typeof selected === "string") setGamePath(selected);
		} else {
			const path = prompt(t("patch.prompt.gameFolder"));
			if (path) setGamePath(path);
		}
	};

	const pickZip = async () => {
		if (IS_TAURI) {
			const { open: openDialog } = await import("@tauri-apps/plugin-dialog");
			const selected = await openDialog({
				title: t("patch.dialog.selectZip"),
				multiple: false,
				filters: [{ name: t("patch.filter.patchZip"), extensions: ["zip"] }],
			});
			if (typeof selected === "string") {
				setZipPath(selected);
				setZipUrl(""); // mutual exclusion with URL field
			}
		} else {
			const path = prompt(t("patch.prompt.zipPath"));
			if (path) {
				setZipPath(path);
				setZipUrl("");
			}
		}
	};

	const pickOutputZip = async () => {
		if (IS_TAURI) {
			const { save } = await import("@tauri-apps/plugin-dialog");
			const selected = await save({
				title: t("patch.dialog.saveZip"),
				defaultPath: "locust-patch.zip",
				filters: [{ name: t("patch.filter.patchZip"), extensions: ["zip"] }],
			});
			if (typeof selected === "string" && selected) setOutputPath(selected);
		} else {
			const path = prompt(
				t("patch.prompt.outputZip"),
				outputPath || "locust-patch.zip",
			);
			if (path) setOutputPath(path);
		}
	};

	const pickPristineFolder = async () => {
		if (IS_TAURI) {
			const { open: openDialog } = await import("@tauri-apps/plugin-dialog");
			const selected = await openDialog({
				title: t("patch.dialog.pristineFolder"),
				directory: true,
			});
			if (typeof selected === "string") {
				setPristinePath(selected);
				setPristine(true);
			}
		} else {
			const path = prompt(t("patch.prompt.pristineFolder"));
			if (path) {
				setPristinePath(path);
				setPristine(true);
			}
		}
	};

	const refreshStatus = async () => {
		if (!gamePath.trim()) return;
		try {
			const s = await patchStatus({ game_path: gamePath.trim() });
			setStatus(s);
			onPatchStateChanged?.();
		} catch (err: any) {
			setStatus(null);
			setError(err.message);
		}
	};

	const handleVerify = async () => {
		if (!gamePath.trim()) {
			addToast("error", t("patch.toast.selectGame"));
			return;
		}
		if (!resolvedSource) {
			addToast("error", t("patch.toast.selectSource"));
			return;
		}
		if ("error" in resolvedSource) {
			addToast("error", t(resolvedSource.error));
			return;
		}
		setLoading(true);
		setError(null);
		setVerify(null);
		setApplyResult(null);
		try {
			const report = await patchVerify({
				game_path: gamePath.trim(),
				...resolvedSource,
			});
			rememberPatchSource(resolvedSource);
			setVerify(report);
			await refreshStatus();
			addLog(
				"info",
				`Patch verify: ${report.outcome}`,
				report.messages?.join("\n") || "",
				"patch",
			);
			addToast("success", t("patch.toast.verify", { outcome: report.outcome }));
		} catch (err: any) {
			setError(err.message);
			addLog("error", "Patch verify failed", err.message, "patch");
			addToast("error", t("patch.toast.verifyFailed", { error: err.message }));
		} finally {
			setLoading(false);
		}
	};

	const finishApplyReport = async (
		report: PatchApplyResult,
		source: { zip_path: string } | { zip_url: string },
	) => {
		rememberPatchSource(source);
		setApplyResult(report);
		await refreshStatus();
		addLog(
			"info",
			report.dry_run
				? `Patch dry-run: ${report.patch_id}@${report.patch_version}`
				: `Patch applied: ${report.patch_id}@${report.patch_version}`,
			`replaced ${report.replaced}, added ${report.added}, baseline ${report.baseline}`,
			"patch",
		);
		addToast(
			"success",
			report.dry_run
				? t("patch.toast.planned", { count: report.replaced + report.added })
				: t("patch.toast.applied", { count: report.replaced + report.added }),
		);
	};

	const finishApplyInterrupted = async (opts: {
		cancelled: boolean;
		error?: string;
	}) => {
		setNeedsRollback(true);
		if (opts.cancelled) {
			addLog(
				"warning",
				"Patch apply cancelled — game left partly patched",
				undefined,
				"patch",
			);
			addToast("warning", t("patch.toast.applyCancelled"));
		} else {
			const message = opts.error ?? t("ws.patchJobStreamLost");
			setError(message);
			addLog("error", "Patch apply failed", message, "patch");
			addToast(
				"warning",
				t("patch.toast.applyFailedPartial", { error: message }),
			);
		}
		try {
			await refreshStatus();
		} finally {
			setApplying(false);
			setCancellingApply(false);
			setApplyProgress(null);
		}
	};

	const handleApply = async () => {
		if (!gamePath.trim()) {
			addToast("error", t("patch.toast.selectGame"));
			return;
		}
		if (!resolvedSource) {
			addToast("error", t("patch.toast.selectSource"));
			return;
		}
		if ("error" in resolvedSource) {
			addToast("error", t(resolvedSource.error));
			return;
		}
		const source = resolvedSource;
		setError(null);
		setApplyResult(null);
		setApplyProgress(null);
		setNeedsRollback(false);
		setApplying(true);
		setCancellingApply(false);
		applyFinishedRef.current = false;
		applyCancelRequestedRef.current = false;
		applyUnsubRef.current?.();
		applyUnsubRef.current = null;
		try {
			const started = await patchApply({
				game_path: gamePath.trim(),
				...source,
				force,
				confirm_legacy: confirmLegacy,
				dry_run: dryRun,
			});
			applyJobIdRef.current = started.job_id;
			applyUnsubRef.current = subscribeToJob(
				started.job_id,
				{
					onProgress: (e) => {
						setApplyProgress({
							current: e.current,
							total: e.total,
							path: e.path,
							phase: e.phase,
						});
					},
					onDone: (e) => {
						if (applyFinishedRef.current) return;
						applyFinishedRef.current = true;
						applyUnsubRef.current?.();
						applyUnsubRef.current = null;
						applyJobIdRef.current = null;
						void finishApplyReport(e.report, source).finally(() => {
							setApplying(false);
							setCancellingApply(false);
							setApplyProgress(null);
						});
					},
					onError: (e) => {
						if (applyFinishedRef.current) return;
						applyFinishedRef.current = true;
						applyUnsubRef.current?.();
						applyUnsubRef.current = null;
						applyJobIdRef.current = null;
						void finishApplyInterrupted(
							applyCancelRequestedRef.current
								? { cancelled: true }
								: { cancelled: false, error: e.message },
						);
					},
					onClosed: () => {
						if (applyFinishedRef.current) return;
						applyFinishedRef.current = true;
						applyUnsubRef.current = null;
						applyJobIdRef.current = null;
						if (applyCancelRequestedRef.current) {
							void finishApplyInterrupted({ cancelled: true });
							return;
						}
						const message = t("ws.patchJobStreamLost");
						setError(message);
						addLog("error", "Patch apply failed", message, "patch");
						addToast(
							"error",
							t("patch.toast.applyFailed", { error: message }),
						);
						setApplying(false);
						setCancellingApply(false);
					},
				},
				"patch",
			);
		} catch (err: any) {
			applyFinishedRef.current = true;
			setError(err.message);
			addLog("error", "Patch apply failed", err.message, "patch");
			addToast("error", t("patch.toast.applyFailed", { error: err.message }));
			setApplying(false);
			setCancellingApply(false);
		}
	};

	const handleCancelApply = async () => {
		const jobId = applyJobIdRef.current;
		if (!jobId || cancellingApply) return;
		applyCancelRequestedRef.current = true;
		setCancellingApply(true);
		try {
			await cancelPatchApply(jobId);
			addLog(
				"info",
				`Cancel requested for patch job ${jobId}`,
				undefined,
				"patch",
			);
			addToast("info", t("patch.toast.applyCancelling"));
		} catch (err: any) {
			applyCancelRequestedRef.current = false;
			setCancellingApply(false);
			addToast(
				"error",
				t("patch.toast.applyCancelFailed", { error: err.message ?? err }),
			);
		}
	};

	const handleRollback = async () => {
		if (!gamePath.trim()) {
			addToast("error", t("patch.toast.selectGame"));
			return;
		}
		setLoading(true);
		setError(null);
		try {
			const report = await patchRollback({
				game_path: gamePath.trim(),
				force,
			});
			if (report.aborted_edited?.length) {
				addToast(
					"error",
					t("patch.toast.rollbackForce", { count: report.aborted_edited.length }),
				);
			} else {
				addToast(
					"success",
					t("patch.toast.rollback", {
						restored: report.restored,
						deleted: report.deleted,
					}),
				);
				setNeedsRollback(false);
			}
			addLog(
				"info",
				"Patch rollback",
				report.messages?.join("\n") ||
					`restored ${report.restored}, deleted ${report.deleted}`,
				"patch",
			);
			setApplyResult(null);
			setVerify(null);
			await refreshStatus();
		} catch (err: any) {
			setError(err.message);
			addLog("error", "Patch rollback failed", err.message, "patch");
			addToast("error", t("patch.toast.rollbackFailed", { error: err.message }));
		} finally {
			setLoading(false);
		}
	};

	const handlePack = async () => {
		if (!gamePath.trim()) {
			addToast("error", t("patch.toast.selectRecorded"));
			return;
		}
		if (!outputPath.trim()) {
			addToast("error", t("patch.toast.chooseOutput"));
			return;
		}
		setLoading(true);
		setError(null);
		setPackResult(null);
		try {
			const langs = languages
				.split(/[,\s]+/)
				.map((s) => s.trim())
				.filter(Boolean);
			const report = await patchPack({
				game_path: gamePath.trim(),
				output_path: outputPath.trim(),
				languages: langs,
				pristine,
				pristine_path: pristinePath.trim() || undefined,
			});
			setPackResult(report);
			addLog(
				"info",
				`Patch packed: ${report.patch_id}@${report.patch_version}`,
				`${report.files_packed} file(s), ${report.size_bytes} bytes, tier ${report.tier}`,
				"patch",
			);
			addToast(
				"success",
				t("patch.toast.packed", { count: report.files_packed, tier: report.tier }),
			);
		} catch (err: any) {
			setError(err.message);
			addLog("error", "Patch pack failed", err.message, "patch");
			addToast("error", t("patch.toast.packFailed", { error: err.message }));
		} finally {
			setLoading(false);
		}
	};

	const showPartialWarning =
		needsRollback || status?.status === "interrupted";

	const tabBtn = (id: Tab, label: string) => (
		<button
			type="button"
			onClick={() => {
				setTab(id);
				setError(null);
			}}
			className={`px-3 py-1.5 text-sm font-medium rounded-t border-b-2 ${
				tab === id
					? "border-emerald-600 text-emerald-700 dark:text-emerald-400"
					: "border-transparent text-gray-500 hover:text-gray-700 dark:hover:text-gray-300"
			}`}
		>
			{label}
		</button>
	);

	return (
		<div className={MODAL_BACKDROP_CLASS}>
			<div
				ref={dialogRef}
				{...dialogProps}
				className={modalPanelClass("max-w-xl p-6 max-h-[90vh] overflow-y-auto")}
			>
				<div className="flex justify-between items-center mb-2">
					<h2
						{...titleProps}
						className="text-lg font-bold flex items-center gap-2"
					>
						<Package size={20} /> {t("patch.title")}
					</h2>
					<button
						onClick={onClose}
						className="text-gray-400 hover:text-gray-600"
					>
						<X size={20} />
					</button>
				</div>

				{allowPack ? (
					<div className="flex gap-1 border-b dark:border-gray-700 mb-4">
						{tabBtn("apply", t("patch.apply"))}
						{tabBtn("pack", t("patch.pack"))}
					</div>
				) : (
					<div className="mb-4" />
				)}

				<div className="space-y-4">
					<div>
						<label className="text-sm font-medium">
							{tab === "pack" ? t("patch.recordedFolder") : t("patch.gameFolder")}
						</label>
						<div className="flex gap-2 mt-1">
							<input
								value={gamePath}
								onChange={(e) => setGamePath(e.target.value)}
								placeholder={
									tab === "pack"
										? t("patch.placeholder.recorded")
										: t("patch.placeholder.game")
								}
								className="flex-1 p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
							/>
							<button
								onClick={pickGame}
								className="px-3 py-2 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded"
								title={t("common.browse")}
							>
								<FolderOpen size={16} />
							</button>
						</div>
					</div>

					{tab === "apply" && (
						<>
							<div>
								<label className="text-sm font-medium">{t("patch.zipLocal")}</label>
								<div className="flex gap-2 mt-1">
									<input
										value={zipPath}
										onChange={(e) => {
											setZipPath(e.target.value);
											if (e.target.value.trim()) setZipUrl("");
										}}
										placeholder={t("patch.zipPlaceholder")}
										className="flex-1 p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
									/>
									<button
										onClick={pickZip}
										className="px-3 py-2 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded"
										title={t("common.browse")}
									>
										<FileArchive size={16} />
									</button>
								</div>
							</div>

							<div>
								<label className="text-sm font-medium">{t("patch.orUrl")}</label>
								<input
									value={zipUrl}
									onChange={(e) => {
										setZipUrl(e.target.value);
										if (e.target.value.trim()) setZipPath("");
									}}
									placeholder={t("patch.urlPlaceholder")}
									className={`w-full mt-1 p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm ${
										urlFieldError
											? "border-red-400 dark:border-red-600"
											: zipUrl.trim() && isHttpPatchUrl(zipUrl)
												? "border-emerald-400 dark:border-emerald-700"
												: ""
									}`}
								/>
								{urlFieldError ? (
									<p className="text-xs text-red-600 dark:text-red-400 mt-1">
										{urlFieldError}
									</p>
								) : urlZipHint ? (
									<p className="text-xs text-amber-600 dark:text-amber-400 mt-1">
										{urlZipHint}
									</p>
								) : (
									<p className="text-xs text-gray-500 mt-1">
										{t("patch.urlHelp")}
									</p>
								)}
								{patchSourceReady(resolvedSource) && (
									<p className="text-xs text-emerald-700 dark:text-emerald-400 mt-0.5">
										{"zip_url" in resolvedSource
											? t("patch.activeUrl", {
													url: `${resolvedSource.zip_url.slice(0, 48)}${
														resolvedSource.zip_url.length > 48 ? "…" : ""
													}`,
												})
											: t("patch.activeLocal")}
									</p>
								)}
							</div>

							<div className="space-y-2 text-sm">
								<label className="flex items-start gap-2 cursor-pointer">
									<input
										type="checkbox"
										className="mt-0.5"
										checked={force}
										onChange={(e) => setForce(e.target.checked)}
									/>
									<span>
										<span className="font-medium">{t("patch.force")}</span>
										<span className="block text-xs text-gray-500 mt-0.5">
											{t("patch.forceHint")}
										</span>
									</span>
								</label>
								<label className="flex items-start gap-2 cursor-pointer">
									<input
										type="checkbox"
										className="mt-0.5"
										checked={confirmLegacy}
										onChange={(e) => setConfirmLegacy(e.target.checked)}
									/>
									<span>
										<span className="font-medium">{t("patch.legacy")}</span>
										<span className="block text-xs text-gray-500 mt-0.5">
											{t("patch.legacyHint")}
										</span>
									</span>
								</label>
								<label className="flex items-start gap-2 cursor-pointer">
									<input
										type="checkbox"
										className="mt-0.5"
										checked={dryRun}
										onChange={(e) => setDryRun(e.target.checked)}
									/>
									<span>
										<span className="font-medium">{t("patch.dryRun")}</span>
										<span className="block text-xs text-gray-500 mt-0.5">
											{t("patch.dryRunHint")}
										</span>
									</span>
								</label>
							</div>

							<div className="flex flex-wrap gap-2">
								<button
									onClick={handleVerify}
									disabled={loading || applying || !canVerifyApply}
									title={
										!gamePath.trim()
											? t("patch.selectGame")
											: !sourceOk
												? t("patch.selectSource")
												: undefined
									}
									className="flex items-center gap-1.5 px-3 py-2 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded text-sm font-medium disabled:opacity-50"
								>
									<ShieldCheck size={16} /> {t("patch.verify")}
								</button>
								<button
									onClick={handleApply}
									disabled={loading || applying || !canVerifyApply}
									title={
										!gamePath.trim()
											? t("patch.selectGame")
											: !sourceOk
												? t("patch.selectSource")
												: undefined
									}
									className="flex items-center gap-1.5 px-3 py-2 bg-emerald-600 hover:bg-emerald-700 text-white rounded text-sm font-medium disabled:opacity-50"
								>
									{applying ? (
										<Loader2 size={16} className="animate-spin" />
									) : (
										<Package size={16} />
									)}{" "}
									{dryRun ? t("patch.planApply") : t("patch.applyBtn")}
								</button>
								<button
									onClick={handleRollback}
									disabled={loading || applying}
									className={
										showPartialWarning
											? "flex items-center gap-1.5 px-3 py-2 bg-amber-600 hover:bg-amber-700 text-white rounded text-sm font-medium disabled:opacity-50 ring-2 ring-amber-400 ring-offset-1 dark:ring-offset-gray-900"
											: "flex items-center gap-1.5 px-3 py-2 bg-amber-100 hover:bg-amber-200 text-amber-900 dark:bg-amber-900/40 dark:text-amber-100 dark:hover:bg-amber-900/60 rounded text-sm font-medium disabled:opacity-50"
									}
								>
									<RotateCcw size={16} /> {t("patch.rollback")}
								</button>
								<button
									onClick={refreshStatus}
									disabled={loading || applying || !gamePath.trim()}
									className="px-3 py-2 text-sm text-gray-600 hover:underline disabled:opacity-50"
								>
									{t("patch.refreshStatus")}
								</button>
							</div>

							{applying && (
								<div className="p-3 border border-emerald-200 dark:border-emerald-800 bg-emerald-50/50 dark:bg-emerald-950/30 rounded text-sm space-y-2">
									<div className="flex items-center gap-2 font-medium">
										<Loader2
											size={16}
											className="animate-spin text-emerald-600"
										/>
										{t("patch.applying")}
									</div>
									{applyProgress && (
										<>
											{applyProgress.phase && (
												<div className="text-xs text-gray-600 dark:text-gray-400">
													{t("patch.progress.phase", {
														phase: applyProgress.phase,
													})}
												</div>
											)}
											<div className="w-full bg-gray-200 dark:bg-gray-700 rounded-full h-2">
												<div
													className="bg-emerald-500 h-2 rounded-full transition-all"
													style={{
														width: `${
															applyProgress.total > 0
																? Math.min(
																		100,
																		(applyProgress.current /
																			applyProgress.total) *
																			100,
																	)
																: 0
														}%`,
													}}
												/>
											</div>
											<div className="text-xs tabular-nums">
												{t("patch.progress.counts", {
													current: applyProgress.current,
													total: applyProgress.total,
												})}
											</div>
											{applyProgress.path && (
												<div className="text-xs text-gray-500 truncate" title={applyProgress.path}>
													{t("patch.progress.file", {
														path: applyProgress.path,
													})}
												</div>
											)}
										</>
									)}
									<button
										type="button"
										onClick={handleCancelApply}
										disabled={cancellingApply}
										className="w-full py-1.5 bg-red-600 hover:bg-red-700 disabled:opacity-50 text-white rounded text-sm font-medium"
									>
										{cancellingApply
											? t("patch.cancelling")
											: t("patch.cancelApply")}
									</button>
								</div>
							)}

							{!applying && showPartialWarning && (
								<div className="p-3 border border-amber-400 dark:border-amber-700 bg-amber-50 dark:bg-amber-950/40 rounded text-sm space-y-2">
									<div className="flex gap-2 text-amber-950 dark:text-amber-100">
										<AlertCircle size={16} className="shrink-0 mt-0.5" />
										<span>{t("patch.partialWarning")}</span>
									</div>
									<button
										type="button"
										onClick={handleRollback}
										disabled={loading}
										className="w-full flex items-center justify-center gap-1.5 py-2 bg-amber-600 hover:bg-amber-700 disabled:opacity-50 text-white rounded text-sm font-medium"
									>
										<RotateCcw size={16} /> {t("patch.rollbackNow")}
									</button>
								</div>
							)}

							{status && (
								<div className="p-3 bg-gray-50 dark:bg-gray-800/60 rounded text-sm space-y-1">
									<div className="font-medium">{t("patch.status", { status: status.status })}</div>
									{status.status === "patched" && (
										<div className="text-xs text-gray-600 dark:text-gray-400">
											{t("patch.statusDetail", {
												id: status.patch_id ?? "",
												version: status.patch_version ?? "",
												engine: status.engine ?? "",
												language: status.language ?? "",
												baseline: status.baseline ?? "",
												replaced: status.replaced ?? 0,
												added: status.added ?? 0,
											})}
										</div>
									)}
									{status.status === "interrupted" && (
										<div className="text-xs text-amber-600">
											{t("patch.interrupted", { id: status.patch_id ?? "" })}
										</div>
									)}
								</div>
							)}

							{verify && (
								<div className="p-3 border rounded dark:border-gray-700 text-sm space-y-1">
									<div className="font-medium">{t("patch.verifyLine", { outcome: verify.outcome })}</div>
									{verify.tier && (
										<div className="text-xs">{t("patch.tier", { tier: verify.tier })}</div>
									)}
									<div className="text-xs text-gray-600 dark:text-gray-400">
										{t("patch.plan", {
											replace: verify.replaced?.length ?? 0,
											add: verify.added?.length ?? 0,
										})}
										{(verify.conflicts?.length ?? 0) > 0 &&
											t("patch.conflicts", { count: verify.conflicts.length })}
									</div>
									{verify.backup_compromised && (
										<div className="text-xs text-amber-600">
											{t("patch.backupCompromised")}
										</div>
									)}
									{verify.messages?.map((m, i) => (
										<div key={i} className="text-xs text-gray-500">
											{m}
										</div>
									))}
								</div>
							)}

							{applyResult && (
								<div className="p-3 border border-emerald-200 dark:border-emerald-800 bg-emerald-50/50 dark:bg-emerald-950/30 rounded text-sm space-y-1">
									<div className="font-medium">
										{t(
											applyResult.dry_run ? "patch.planned" : "patch.applied",
											{
												id: applyResult.patch_id,
												version: applyResult.patch_version,
											},
										)}
									</div>
									<div className="text-xs">
										{t("patch.applyDetail", {
											replaced: applyResult.replaced,
											added: applyResult.added,
											baseline: applyResult.baseline,
										})}
									</div>
									{applyResult.user_edits_overwritten?.length > 0 && (
										<div className="text-xs text-amber-700">
											{t("patch.overwrote", {
												files: applyResult.user_edits_overwritten.join(", "),
											})}
										</div>
									)}
								</div>
							)}

							<p className="text-xs text-gray-500">
								{t("patch.applyHelp")}
							</p>
						</>
					)}

					{tab === "pack" && (
						<>
							<div>
								<label className="text-sm font-medium">{t("patch.outputZip")}</label>
								<div className="flex gap-2 mt-1">
									<input
										value={outputPath}
										onChange={(e) => setOutputPath(e.target.value)}
										placeholder={t("patch.outputPlaceholder")}
										className="flex-1 p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
									/>
									<button
										onClick={pickOutputZip}
										className="px-3 py-2 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded"
										title={t("common.saveAs")}
									>
										<FileArchive size={16} />
									</button>
								</div>
							</div>

							<div>
								<label className="text-sm font-medium">{t("patch.languages")}</label>
								<input
									value={languages}
									onChange={(e) => setLanguages(e.target.value)}
									placeholder={t("patch.languagesPlaceholder")}
									className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
								/>
								<p className="text-xs text-gray-500 mt-1">
									{t("patch.languagesHint")}
								</p>
							</div>

							<label className="flex items-start gap-2 text-sm cursor-pointer">
								<input
									type="checkbox"
									className="mt-0.5"
									checked={pristine}
									onChange={(e) => setPristine(e.target.checked)}
								/>
								<span>
									<span className="font-medium">{t("patch.pristine")}</span>
									<span className="block text-xs text-gray-500 mt-0.5">
										{t("patch.pristineHint")}
									</span>
								</span>
							</label>

							<div>
								<label className="text-sm font-medium">{t("patch.pristinePath")}</label>
								<div className="flex gap-2 mt-1">
									<input
										value={pristinePath}
										onChange={(e) => {
											const value = e.target.value;
											setPristinePath(value);
											if (value.trim()) setPristine(true);
										}}
										placeholder={t("patch.pristinePathPlaceholder")}
										className="flex-1 p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
									/>
									<button
										type="button"
										onClick={() => {
											void pickPristineFolder();
										}}
										className="px-3 py-2 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded"
										title={t("common.browse")}
									>
										<FolderOpen size={16} />
									</button>
								</div>
								<p className="text-xs text-gray-500 mt-1">
									{t("patch.pristinePathHint")}
								</p>
							</div>

							<button
								onClick={handlePack}
								disabled={loading || applying}
								className="flex items-center gap-1.5 px-3 py-2 bg-emerald-600 hover:bg-emerald-700 text-white rounded text-sm font-medium disabled:opacity-50"
							>
								<Archive size={16} /> {t("patch.packBtn")}
							</button>

							{packResult && (
								<div className="p-3 border border-emerald-200 dark:border-emerald-800 bg-emerald-50/50 dark:bg-emerald-950/30 rounded text-sm space-y-1">
									<div className="font-medium">
										{t("patch.packed", {
											id: packResult.patch_id,
											version: packResult.patch_version,
										})}
									</div>
									<div className="text-xs text-gray-600 dark:text-gray-400 space-y-0.5">
										<div>
											{t("patch.packedStats", {
												files: packResult.files_packed,
												bytes: packResult.size_bytes,
												tier: packResult.tier,
											})}
										</div>
										<div>
											{t("patch.packedMeta", {
												engine: packResult.engine,
												language: packResult.language,
											})}
											{packResult.translated_strings != null &&
												t("patch.packedStrings", {
													count: packResult.translated_strings,
												})}
										</div>
										<div className="break-all">{packResult.output_path}</div>
									</div>
									{packResult.messages?.map((m, i) => (
										<div
											key={i}
											className="text-xs text-amber-700 dark:text-amber-300"
										>
											{m}
										</div>
									))}
								</div>
							)}

							<p className="text-xs text-gray-500">
								{t("patch.packHelp")}
							</p>
						</>
					)}

					{error && (
						<div className="flex gap-2 p-3 bg-red-50 dark:bg-red-950/40 border border-red-200 dark:border-red-800 rounded text-sm text-red-700 dark:text-red-300">
							<AlertCircle size={16} className="shrink-0 mt-0.5" />
							<pre className="whitespace-pre-wrap break-words font-sans">
								{error}
							</pre>
						</div>
					)}
				</div>
			</div>
		</div>
	);
}
