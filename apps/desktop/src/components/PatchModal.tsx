import { IS_TAURI } from "../lib/runtime";
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
	patchIdentity,
	patchRollback,
	patchStatus,
	getPatchRecordings,
	patchVerify,
	type PatchApplyResult,
	type PatchPackResult,
	type PatchRollbackResult,
	type PatchStatusResult,
	type PatchVerifyResult,
} from "../lib/api";
import {
	canPackFromRecordings,
	isPackLangSelected,
	packLangFromRecording,
	preferredPackLang,
	type RecordedLang,
} from "../lib/patchRecordings";
import {
	isHttpPatchUrl,
	loadRememberedPatchSource,
	patchSourceReady,
	patchUrlLooksLikeZip,
	rememberPatchSource,
	resolvePatchSource,
} from "../lib/patchSource";
import { defaultEntryPath, patchPublishFields, publishCommand } from "../lib/patchPublish";
import { subscribeToJob } from "../lib/ws";
import { addLog } from "../stores/logStore";
import { addToast } from "../stores/toastStore";
import {
	useModalA11y,
	MODAL_BACKDROP_CLASS,
	modalPanelClass,
} from "../lib/modalA11y";
import { useT } from "../lib/i18n";

type Tab = "apply" | "pack";

interface PatchModalProps {
	open: boolean;
	onClose: () => void;
	/** Optional default game path (e.g. current project folder). */
	defaultGamePath?: string;
	initialZipPath?: string;
	/** Exact pre-injection backup, scoped to defaultGamePath. */
	initialBackupId?: string;
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
	initialZipPath,
	initialBackupId,
	initialTab = "apply",
	onPatchStateChanged,
	allowPack = true,
}: PatchModalProps) {
	const t = useT();
	const patchValue = (value: string): string => {
		const labels: Record<string, string> = {
			pristine: t("patch.status.pristine"),
			patched: t("patch.status.patched"),
			interrupted: t("patch.status.interrupted"),
			unknown: t("patch.status.unknown"),
			strict: t("patch.value.strict"),
			structural: t("patch.value.structural"),
			legacy: t("patch.value.legacy"),
			unverified: t("patch.value.unverified"),
			clean: t("patch.value.clean"),
			alreadyapplied: t("patch.value.alreadyApplied"),
			upgradeavailable: t("patch.value.upgradeAvailable"),
		};
		return labels[value.toLowerCase()] ?? value;
	};
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
	const [rjCode, setRjCode] = useState("");
	const rjEdited = useRef(false);
	const [detectId, setDetectId] = useState(true);
	const [gameVersion, setGameVersion] = useState("");
	const [createEntry, setCreateEntry] = useState(() => {
		try { return localStorage.getItem("locust.patch.createEntry") === "true"; }
		catch { return false; }
	});
	const [entryPathOverride, setEntryPathOverride] = useState<string | null>(null);
	const entryPath = entryPathOverride ?? defaultEntryPath(outputPath);
	const [languages, setLanguages] = useState("");
	const [recordings, setRecordings] = useState<RecordedLang[]>([]);
	const [pristineRecordings, setPristineRecordings] = useState<RecordedLang[]>([]);
	const [pristine, setPristine] = useState(false);
	const [pristinePath, setPristinePath] = useState("");
	const [force, setForce] = useState(false);
	const [confirmLegacy, setConfirmLegacy] = useState(false);
	const [dryRun, setDryRun] = useState(false);
	const [loading, setLoading] = useState(false);
	const [verify, setVerify] = useState<PatchVerifyResult | null>(null);
	const [applyResult, setApplyResult] = useState<PatchApplyResult | null>(null);
	const [rollbackPreview, setRollbackPreview] = useState<{
		gamePath: string;
		force: boolean;
		report: PatchRollbackResult;
	} | null>(null);
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
		if (initialBackupId) { setPristine(true); setPristinePath(""); }
		if (initialZipPath) { setZipPath(initialZipPath); setZipUrl(""); setVerify(null); setApplyResult(null); }
		setTab(allowPack ? initialTab : "apply");
		setError(null);
		setRollbackPreview(null);
		if (!allowPack) {
			setRecordings([]);
			setPristineRecordings([]);
			return;
		}
		let cancelled = false;
		void Promise.all([getPatchRecordings(), getConfig()])
			.then(([rec, cfg]) => {
				if (cancelled) return;
				setRecordings(rec.languages);
				setPristineRecordings(rec.pristine_languages ?? []);
				setLanguages((prev) =>
					preferredPackLang(rec.languages, cfg.default_target_lang, prev),
				);
			})
			.catch(() => {
				if (cancelled) return;
				setRecordings([]);
				setPristineRecordings([]);
			});
		return () => { cancelled = true; };
	}, [open, defaultGamePath, initialZipPath, initialBackupId, initialTab, allowPack]);

	useEffect(() => {
		if (!open || !allowPack || tab !== "pack") return;
		let cancelled = false;
		if (!rjEdited.current) setRjCode("");
		if (gamePath.trim() && detectId) {
			void patchIdentity(gamePath.trim()).then((identity) => {
				if (!cancelled && !rjEdited.current) setRjCode(identity.detected_dlsite_code ?? "");
			}).catch(() => {
				// Pack still validates and detects identity if the preview is unavailable.
			});
		}
		return () => { cancelled = true; };
	}, [open, allowPack, tab, gamePath, detectId]);

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
	// A plan is a snapshot for the exact game and Force choice that was previewed.
	const preview = rollbackPreview?.gamePath === gamePath.trim() && rollbackPreview.force === force
		? rollbackPreview.report : null;
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

	const pickEntry = async () => {
		if (IS_TAURI) {
			const { save } = await import("@tauri-apps/plugin-dialog");
			const selected = await save({
				title: t("patch.publish.saveEntry"),
				defaultPath: entryPath || "locust-patch.md",
				filters: [{ name: t("patch.publish.entryFilter"), extensions: ["md"] }],
			});
			if (typeof selected === "string" && selected) setEntryPathOverride(selected);
		} else {
			const path = prompt(t("patch.publish.entryPath"), entryPath || "locust-patch.md");
			if (path) setEntryPathOverride(path);
		}
	};

	const copyPublishCommand = async () => {
		if (!packResult?.entry_path) return;
		try {
			await navigator.clipboard.writeText(publishCommand(packResult.output_path, packResult.entry_path));
			addToast("success", t("common.copied"));
		} catch {
			addToast("error", t("patch.publish.copyFailed"));
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
		setRollbackPreview(null);
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
				t("activity.patch.verified", { outcome: patchValue(report.outcome) }),
				report.messages?.join("\n") || "",
				"patch",
			);
			addToast("success", t("patch.toast.verify", { outcome: patchValue(report.outcome) }));
		} catch (err: any) {
			setError(err.message);
			addLog("error", t("activity.patch.verifyFailed"), err.message, "patch");
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
			t(report.dry_run ? "activity.patch.dryRun" : "activity.patch.applied", {
				id: report.patch_id, version: report.patch_version,
			}),
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
				t("activity.patch.cancelledPartial"),
				undefined,
				"patch",
			);
			addToast("warning", t("patch.toast.applyCancelled"));
		} else {
			const message = opts.error ?? t("ws.patchJobStreamLost");
			setError(message);
			addLog("error", t("activity.patch.applyFailed"), message, "patch");
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
						addLog("error", t("activity.patch.applyFailed"), message, "patch");
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
			addLog("error", t("activity.patch.applyFailed"), err.message, "patch");
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
				t("activity.patch.cancelRequested", { jobId }),
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

	const handlePreviewRollback = async () => {
		if (!gamePath.trim()) {
			setError(t("patch.toast.selectGame"));
			return;
		}
		const scope = { gamePath: gamePath.trim(), force };
		setLoading(true);
		setError(null);
		setRollbackPreview(null);
		try {
			const report = await patchRollback({
				game_path: scope.gamePath,
				force: scope.force,
				dry_run: true,
			});
			setRollbackPreview({ ...scope, report });
		} catch (err: any) {
			setError(err.message);
		} finally {
			setLoading(false);
		}
	};

	const handleRollback = async () => {
		if (!gamePath.trim()) {
			addToast("error", t("patch.toast.selectGame"));
			return;
		}
		setLoading(true);
		setError(null);
		setRollbackPreview(null);
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
				t("activity.patch.rollback"),
				report.messages?.join("\n") ||
					`restored ${report.restored}, deleted ${report.deleted}`,
				"patch",
			);
			setApplyResult(null);
			setVerify(null);
			await refreshStatus();
		} catch (err: any) {
			setError(err.message);
			addLog("error", t("activity.patch.rollbackFailed"), err.message, "patch");
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
		if (createEntry && !entryPath.trim()) {
			addToast("error", t("patch.publish.chooseEntry"));
			return;
		}
		if (!canPackFromRecordings(recordings, languages)) {
			addToast("error", t("patch.recordedLangsEmpty"));
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
				...patchPublishFields({
					// Let pack detect again from the current folder if its preview is still loading.
					rjCode: rjEdited.current ? rjCode : "",
					gameVersion, detectId, createEntry, entryPath,
				}),
				game_path: gamePath.trim(),
				output_path: outputPath.trim(),
				languages: langs,
				pristine,
				pristine_path: pristinePath.trim() || undefined,
				pristine_backup_id: pristine && !pristinePath.trim() && gamePath.trim() === defaultGamePath?.trim() ? initialBackupId : undefined,
			});
			setPackResult(report);
			try { localStorage.setItem("locust.patch.createEntry", String(createEntry)); }
			catch { /* Preferences are optional when storage is unavailable. */ }
			addLog(
				"info",
				t("activity.patch.packed", { id: report.patch_id, version: report.patch_version }),
				`${report.files_packed} file(s), ${report.size_bytes} bytes, tier ${report.tier}`,
				"patch",
			);
			addToast(
				"success",
				t("patch.toast.packed", { count: report.files_packed, tier: patchValue(report.tier) }),
			);
		} catch (err: any) {
			setError(err.message);
			addLog("error", t("activity.patch.packFailed"), err.message, "patch");
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
			className={`px-3 py-1.5 text-body font-medium rounded-t border-b-2 ${
				tab === id
					? "border-accent-fg text-accent-fg"
					: "border-transparent text-text-muted hover:text-text"
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
						className="text-section font-bold flex items-center gap-2"
					>
						<Package size={20} /> {t("patch.title")}
					</h2>
					<button
						onClick={onClose}
						aria-label={t("common.close")}
						className="text-text-muted hover:text-text"
					>
						<X size={20} />
					</button>
				</div>

				{allowPack ? (
					<div className="flex gap-1 border-b border-border mb-4">
						{tabBtn("apply", t("patch.apply"))}
						{tabBtn("pack", t("patch.pack"))}
					</div>
				) : (
					<div className="mb-4" />
				)}

				<div className="space-y-4">
					<div>
						<label className="text-body font-medium">
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
								className="flex-1 p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
							/>
							<button
								onClick={pickGame}
								className="px-3 py-2 bg-surface-muted text-text hover:bg-surface rounded"
								title={t("common.browse")}
							>
								<FolderOpen size={16} />
							</button>
						</div>
					</div>

					{tab === "apply" && (
						<>
							<div>
								<label className="text-body font-medium">{t("patch.zipLocal")}</label>
								<div className="flex gap-2 mt-1">
									<input
										value={zipPath}
										onChange={(e) => {
											setZipPath(e.target.value);
											if (e.target.value.trim()) setZipUrl("");
										}}
										placeholder={t("patch.zipPlaceholder")}
										className="flex-1 p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
									/>
									<button
										onClick={pickZip}
										className="px-3 py-2 bg-surface-muted text-text hover:bg-surface rounded"
										title={t("common.browse")}
									>
										<FileArchive size={16} />
									</button>
								</div>
							</div>

							<div>
								<label className="text-body font-medium">{t("patch.orUrl")}</label>
								<input
									value={zipUrl}
									onChange={(e) => {
										setZipUrl(e.target.value);
										if (e.target.value.trim()) setZipPath("");
									}}
									placeholder={t("patch.urlPlaceholder")}
									className={`w-full mt-1 p-2 border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg ${
										urlFieldError
											? "border-danger"
											: zipUrl.trim() && isHttpPatchUrl(zipUrl)
												? "border-success"
												: "border-border"
									}`}
								/>
								{urlFieldError ? (
									<p className="text-caption text-danger mt-1">
										{urlFieldError}
									</p>
								) : urlZipHint ? (
									<p className="text-caption text-warning mt-1">
										{urlZipHint}
									</p>
								) : (
									<p className="text-caption text-text-muted mt-1">
										{t("patch.urlHelp")}
									</p>
								)}
								{patchSourceReady(resolvedSource) && (
									<p className="text-caption text-success mt-0.5">
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

							<div className="space-y-2 text-body">
								<label className="flex items-start gap-2 cursor-pointer">
									<input
										type="checkbox"
										className="mt-0.5 border border-border bg-surface text-text accent-accent focus:outline-none focus:ring-2 focus:ring-accent-fg"
										checked={force}
										onChange={(e) => setForce(e.target.checked)}
									/>
									<span>
										<span className="font-medium">{t("patch.force")}</span>
										<span className="block text-caption text-text-muted mt-0.5">
											{t("patch.forceHint")}
										</span>
									</span>
								</label>
								<label className="flex items-start gap-2 cursor-pointer">
									<input
										type="checkbox"
										className="mt-0.5 border border-border bg-surface text-text accent-accent focus:outline-none focus:ring-2 focus:ring-accent-fg"
										checked={confirmLegacy}
										onChange={(e) => setConfirmLegacy(e.target.checked)}
									/>
									<span>
										<span className="font-medium">{t("patch.legacy")}</span>
										<span className="block text-caption text-text-muted mt-0.5">
											{t("patch.legacyHint")}
										</span>
									</span>
								</label>
								<label className="flex items-start gap-2 cursor-pointer">
									<input
										type="checkbox"
										className="mt-0.5 border border-border bg-surface text-text accent-accent focus:outline-none focus:ring-2 focus:ring-accent-fg"
										checked={dryRun}
										onChange={(e) => setDryRun(e.target.checked)}
									/>
									<span>
										<span className="font-medium">{t("patch.dryRun")}</span>
										<span className="block text-caption text-text-muted mt-0.5">
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
									className="flex items-center gap-1.5 px-3 py-2 bg-surface-muted text-text hover:bg-surface rounded text-body font-medium disabled:opacity-50"
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
									className="flex items-center gap-1.5 px-3 py-2 bg-accent hover:bg-accent-hover text-white rounded text-body font-medium disabled:opacity-50"
								>
									{applying ? (
										<Loader2 size={16} className="animate-spin" />
									) : (
										<Package size={16} />
									)}{" "}
									{dryRun ? t("patch.planApply") : t("patch.applyBtn")}
								</button>
								<button
									type="button"
									onClick={handlePreviewRollback}
									disabled={loading || applying || !gamePath.trim()}
									className="flex items-center gap-1.5 px-3 py-2 border border-warning text-warning bg-surface hover:bg-warning-muted rounded text-body font-medium disabled:opacity-50"
								>
									<RotateCcw size={16} /> {t("patch.rollbackPreview")}
								</button>
								<button
									onClick={handleRollback}
									disabled={loading || applying}
									className={
										showPartialWarning
											? "flex items-center gap-1.5 px-3 py-2 bg-amber-700 hover:bg-amber-800 text-white rounded text-body font-medium disabled:opacity-50 ring-2 ring-warning ring-offset-1 ring-offset-surface"
											: "flex items-center gap-1.5 px-3 py-2 bg-warning-muted hover:bg-warning-muted/80 text-warning rounded text-body font-medium disabled:opacity-50"
									}
								>
									<RotateCcw size={16} /> {t("patch.rollback")}
								</button>
								<button
									onClick={refreshStatus}
									disabled={loading || applying || !gamePath.trim()}
									className="px-3 py-2 text-body text-text-muted hover:underline disabled:opacity-50"
								>
									{t("patch.refreshStatus")}
								</button>
							</div>

							{applying && (
								<div className="p-3 border border-success bg-success-muted rounded text-body space-y-2">
									<div className="flex items-center gap-2 font-medium">
										<Loader2
											size={16}
											className="animate-spin text-accent-fg"
										/>
										{t("patch.applying")}
									</div>
									{applyProgress && (
										<>
											{applyProgress.phase && (
												<div className="text-caption text-text-muted">
													{t("patch.progress.phase", {
														phase: applyProgress.phase,
													})}
												</div>
											)}
											<div className="w-full bg-border rounded-full h-2">
												<div
													className="bg-accent h-2 rounded-full transition-all"
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
											<div className="text-caption tabular-nums">
												{t("patch.progress.counts", {
													current: applyProgress.current,
													total: applyProgress.total,
												})}
											</div>
											{applyProgress.path && (
												<div className="text-caption text-text-muted truncate" title={applyProgress.path}>
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
										className="w-full py-1.5 bg-red-600 hover:bg-red-700 disabled:opacity-50 text-white rounded text-body font-medium"
									>
										{cancellingApply
											? t("patch.cancelling")
											: t("patch.cancelApply")}
									</button>
								</div>
							)}

							{!applying && showPartialWarning && (
								<div className="p-3 border border-warning bg-warning-muted rounded text-body space-y-2">
									<div className="flex gap-2 text-warning">
										<AlertCircle size={16} className="shrink-0 mt-0.5" />
										<span>{t("patch.partialWarning")}</span>
									</div>
									<button
										type="button"
										onClick={handlePreviewRollback}
										disabled={loading || !gamePath.trim()}
										className="w-full flex items-center justify-center gap-1.5 py-2 border border-warning text-warning bg-surface hover:bg-warning-muted rounded text-body font-medium disabled:opacity-50"
									>
										<RotateCcw size={16} /> {t("patch.rollbackPreview")}
									</button>
									<button
										type="button"
										onClick={handleRollback}
										disabled={loading}
										className="w-full flex items-center justify-center gap-1.5 py-2 bg-amber-700 hover:bg-amber-800 disabled:opacity-50 text-white rounded text-body font-medium"
									>
										<RotateCcw size={16} /> {t("patch.rollbackNow")}
									</button>
								</div>
							)}

							{preview && (
								<div role="status" className="p-3 border border-warning rounded text-body space-y-2">
									<h3 className="font-medium">{t("patch.rollbackPreview.title")}</h3>
									<p className="text-caption">{t("patch.rollbackPreview.hint")}</p>
									<p>{t("patch.rollbackPreview.counts", { restored: preview.restored, deleted: preview.deleted })}</p>
									{preview.messages?.map((message, index) => (
										<p key={index} className="text-caption break-words">{message}</p>
									))}
									{preview.aborted_edited?.length > 0 && (
										<div className="text-warning">
											<p>{t("patch.rollbackPreview.blocked")}</p>
											<ul className="list-disc pl-5 text-caption break-all">
												{preview.aborted_edited.map((path) => <li key={path}>{path}</li>)}
											</ul>
										</div>
									)}
									{preview.torn_deleted?.length > 0 && (
										<div className="text-warning">
											<p>{t("patch.rollbackPreview.forcedDeletes")}</p>
											<ul className="list-disc pl-5 text-caption break-all">
												{preview.torn_deleted.map((path) => <li key={path}>{path}</li>)}
											</ul>
										</div>
									)}
								</div>
							)}

							{status && (
								<div className="p-3 bg-surface-muted rounded text-body space-y-1">
									<div className="font-medium">{t("patch.status", { status: patchValue(status.status) })}</div>
									{status.status === "patched" && (
										<div className="text-caption text-text-muted">
											{t("patch.statusDetail", {
												id: status.patch_id ?? "",
												version: status.patch_version ?? "",
												engine: status.engine ?? "",
												language: status.language ?? "",
												baseline: patchValue(status.baseline ?? ""),
												replaced: status.replaced ?? 0,
												added: status.added ?? 0,
											})}
										</div>
									)}
									{status.status === "interrupted" && (
										<div className="text-caption text-warning">
											{t("patch.interrupted", { id: status.patch_id ?? "" })}
										</div>
									)}
								</div>
							)}

							{verify && (
								<div className="p-3 border border-border rounded text-body space-y-1">
									<div className="font-medium">{t("patch.verifyLine", { outcome: patchValue(verify.outcome) })}</div>
									{verify.tier && (
										<div className="text-caption">{t("patch.tier", { tier: patchValue(verify.tier) })}</div>
									)}
									<div className="text-caption text-text-muted">
										{t("patch.plan", {
											replace: verify.replaced?.length ?? 0,
											add: verify.added?.length ?? 0,
										})}
										{(verify.conflicts?.length ?? 0) > 0 &&
											t("patch.conflicts", { count: verify.conflicts.length })}
									</div>
									{verify.backup_compromised && (
										<div className="text-caption text-warning">
											{t("patch.backupCompromised")}
										</div>
									)}
									{verify.messages?.map((m, i) => (
										<div key={i} className="text-caption text-text-muted">
											{m}
										</div>
									))}
								</div>
							)}

							{applyResult && (
								<div className="p-3 border border-success bg-success-muted rounded text-body space-y-1">
									<div className="font-medium">
										{t(
											applyResult.dry_run ? "patch.planned" : "patch.applied",
											{
												id: applyResult.patch_id,
												version: applyResult.patch_version,
											},
										)}
									</div>
									<div className="text-caption">
										{t("patch.applyDetail", {
											replaced: applyResult.replaced,
											added: applyResult.added,
											baseline: patchValue(applyResult.baseline),
										})}
									</div>
									{applyResult.user_edits_overwritten?.length > 0 && (
										<div className="text-caption text-warning">
											{t("patch.overwrote", {
												files: applyResult.user_edits_overwritten.join(", "),
											})}
										</div>
									)}
								</div>
							)}

							<p className="text-caption text-text-muted">
								{t("patch.applyHelp")}
							</p>
						</>
					)}

					{tab === "pack" && (
						<>
							<div>
								<label className="text-body font-medium">{t("patch.outputZip")}</label>
								<div className="flex gap-2 mt-1">
									<input
										value={outputPath}
										onChange={(e) => setOutputPath(e.target.value)}
										placeholder={t("patch.outputPlaceholder")}
										className="flex-1 p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
									/>
									<button
										onClick={pickOutputZip}
										className="px-3 py-2 bg-surface-muted text-text hover:bg-surface rounded"
										title={t("common.saveAs")}
									>
										<FileArchive size={16} />
									</button>
								</div>
							</div>

							<div>
								<label className="text-body font-medium">{t("patch.languages")}</label>
								<input
									value={languages}
									onChange={(e) => setLanguages(e.target.value)}
									placeholder={t("patch.languagesPlaceholder")}
									className="mt-1 w-full p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
								/>
								<p className="text-caption font-medium text-text-muted mt-2">
									{t("patch.recordedLangs")}
								</p>
								{recordings.length === 0 ? (
									<p className="text-caption text-warning mt-1">
										{t("patch.recordedLangsEmpty")}
									</p>
								) : (
									<div className="flex flex-wrap gap-1.5 mt-1">
										{recordings.map((lang) => {
											const key = lang ?? "(unspecified)";
											const selected = isPackLangSelected(languages, lang);
											return (
												<button
													key={key}
													type="button"
													onClick={() =>
														setLanguages(packLangFromRecording(lang))
													}
													className={`px-2 py-0.5 rounded text-caption font-medium border ${
														selected
															? "border-accent-fg bg-accent-muted text-accent-fg"
															: "border-border text-text hover:bg-surface-muted"
													}`}
												>
													{lang ?? t("patch.recordedUnspecified")}
												</button>
											);
										})}
									</div>
								)}
								<p className="text-caption text-text-muted mt-1">
									{t("patch.languagesHint")}
								</p>
							</div>

							<label className="flex items-start gap-2 text-body cursor-pointer">
								<input
									type="checkbox"
									className="mt-0.5 border border-border bg-surface text-text accent-accent focus:outline-none focus:ring-2 focus:ring-accent-fg"
									checked={pristine}
									onChange={(e) => setPristine(e.target.checked)}
								/>
								<span>
									<span className="font-medium">{t("patch.pristine")}</span>
									<span className="block text-caption text-text-muted mt-0.5">
										{t("patch.pristineHint")}
									</span>
								</span>
							</label>

							<div>
							<label className="text-body font-medium">{t("patch.pristinePath")}</label>
							{pristine && (initialBackupId || pristineRecordings.some(lang => isPackLangSelected(languages, lang))) && gamePath.trim() === defaultGamePath?.trim() && !pristinePath.trim() && <p className="mt-1 text-caption text-success">{t("patch.injectionBackup")}</p>}
								<div className="flex gap-2 mt-1">
									<input
										value={pristinePath}
										onChange={(e) => {
											const value = e.target.value;
											setPristinePath(value);
											if (value.trim()) setPristine(true);
										}}
										placeholder={t("patch.pristinePathPlaceholder")}
										className="flex-1 p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
									/>
									<button
										type="button"
										onClick={() => {
											void pickPristineFolder();
										}}
										className="px-3 py-2 bg-surface-muted text-text hover:bg-surface rounded"
										title={t("common.browse")}
									>
										<FolderOpen size={16} />
									</button>
								</div>
								<p className="text-caption text-text-muted mt-1">
									{t("patch.pristinePathHint")}
								</p>
							</div>

							<details className="border border-border rounded p-3">
								<summary className="text-body font-medium cursor-pointer">{t("patch.publish.title")}</summary>
								<div className="mt-3 space-y-3">
									<label className="block text-body">
										{t("patch.publish.rjCode")}
										<input value={rjCode} onChange={(e) => { rjEdited.current = true; setRjCode(e.target.value); }} className="block w-full mt-1 p-2 border border-border rounded bg-surface text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg" />
									</label>
									<label className="flex items-center gap-2 text-body cursor-pointer">
										<input type="checkbox" checked={!detectId} onChange={(e) => setDetectId(!e.target.checked)} />
										{t("patch.publish.noDetect")}
									</label>
									<label className="block text-body">
										{t("patch.publish.gameVersion")}
										<input value={gameVersion} onChange={(e) => setGameVersion(e.target.value)} className="block w-full mt-1 p-2 border border-border rounded bg-surface text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg" />
									</label>
									<label className="flex items-center gap-2 text-body cursor-pointer">
										<input type="checkbox" checked={createEntry} onChange={(e) => setCreateEntry(e.target.checked)} />
										{t("patch.publish.createEntry")}
									</label>
									{createEntry && <div className="flex items-end gap-2">
										<label className="flex-1 min-w-0 text-body">
											{t("patch.publish.entryPath")}
											<input value={entryPath} onChange={(e) => setEntryPathOverride(e.target.value)} className="block w-full mt-1 p-2 border border-border rounded bg-surface text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg" />
										</label>
										<button type="button" onClick={() => { void pickEntry(); }} title={t("common.browse")} aria-label={t("patch.publish.saveEntry")} className="px-3 py-2 bg-surface-muted text-text hover:bg-surface rounded"><FolderOpen size={16} /></button>
									</div>}
								</div>
							</details>

							<button
								onClick={handlePack}
								disabled={
									loading ||
									applying ||
									!canPackFromRecordings(recordings, languages)
								}
								className="flex items-center gap-1.5 px-3 py-2 bg-accent hover:bg-accent-hover text-white rounded text-body font-medium disabled:opacity-50"
							>
								<Archive size={16} /> {t("patch.packBtn")}
							</button>

							{packResult && (
								<div className="p-3 border border-success bg-success-muted rounded text-body space-y-1">
									<div className="font-medium">
										{t("patch.packed", {
											id: packResult.patch_id,
											version: packResult.patch_version,
										})}
									</div>
									<div className="text-caption text-text-muted space-y-0.5">
										<div>
											{t("patch.packedStats", {
												files: packResult.files_packed,
												bytes: packResult.size_bytes,
												tier: patchValue(packResult.tier),
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
									<div className="text-caption">
										{t("patch.publish.identity", { code: packResult.game?.store_ids.dlsite ?? t("patch.publish.noCode"), count: packResult.game?.fingerprint_count ?? 0 })}
									</div>
									{packResult.entry_path && <div className="pt-2 space-y-2">
										<p className="text-caption">{t("patch.publish.commandHint")}</p>
										<pre className="text-caption whitespace-pre-wrap break-all select-text">{publishCommand(packResult.output_path, packResult.entry_path)}</pre>
										<button type="button" onClick={() => { void copyPublishCommand(); }} className="px-3 py-1 bg-surface-muted text-text hover:bg-surface rounded text-caption">{t("common.copy")}</button>
									</div>}
									{packResult.messages?.map((m, i) => (
										<div
											key={i}
											className="text-caption text-warning"
										>
											{m}
										</div>
									))}
								</div>
							)}

							<p className="text-caption text-text-muted">
								{t("patch.packHelp")}
							</p>
						</>
					)}

					{error && (
						<div className="flex gap-2 p-3 bg-danger-muted border border-danger rounded text-body text-danger">
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
