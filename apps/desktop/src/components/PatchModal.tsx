import { AlertCircle, FolderOpen, Package, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import {
	cancelPatchApply,
	getConfig,
	getPatchRecordings,
	patchApply,
	patchIdentity,
	patchPack,
	patchRollback,
	patchStatus,
	patchVerify,
	type PatchApplyResult,
	type PatchPackResult,
	type PatchRollbackResult,
	type PatchStatusResult,
	type PatchVerifyResult,
} from "../lib/api";
import { ApiError, localizeApiError } from "../lib/apiError";
import { useT } from "../lib/i18n";
import { MODAL_BACKDROP_CLASS, MODAL_HEADER_CLASS, MODAL_BODY_CLASS, MODAL_FOOTER_CLASS, modalPanelClass, useModalA11y } from "../lib/modalA11y";
import { defaultEntryPath, patchPublishFields, publishCommand } from "../lib/patchPublish";
import { canPackFromRecordings, preferredPackLang, type RecordedLang } from "../lib/patchRecordings";
import {
	isHttpPatchUrl,
	loadRememberedPatchSource,
	patchSourceReady,
	patchUrlLooksLikeZip,
	rememberPatchSource,
	resolvePatchSource,
} from "../lib/patchSource";
import { IS_TAURI } from "../lib/runtime";
import { patchVerificationSummary } from "../lib/patchVerification";
import { subscribeToJob } from "../lib/ws";
import { addLog } from "../stores/logStore";
import { addToast } from "../stores/toastStore";
import PatchApplyTab, { PatchApplyActions } from "./patch/PatchApplyTab";
import PatchPackTab, { PatchPackActions } from "./patch/PatchPackTab";

type Tab = "apply" | "pack";

interface PatchModalProps {
	open: boolean;
	onClose: () => void;
	/** Optional default game path (e.g. current project folder). */
	defaultGamePath?: string;
	/** File sources never imply an apply destination directory. */
	defaultGamePathIsFile?: boolean;
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
	defaultGamePathIsFile = false,
	initialZipPath,
	initialBackupId,
	initialTab = "apply",
	onPatchStateChanged,
	allowPack = true,
}: PatchModalProps) {
	const t = useT();
	const patchValue = (value: string): string => {
		const labels: Record<string, string> = {
			not_patched: t("patch.status.notPatched"),
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
	const [packGamePath, setPackGamePath] = useState(defaultGamePath ?? "");
	const [applyGamePath, setApplyGamePath] = useState(defaultGamePathIsFile ? "" : defaultGamePath ?? "");
	const [recordingRoots, setRecordingRoots] = useState<Record<string, string>>({});
	const gamePath = tab === "pack" ? packGamePath : applyGamePath;
	const setGamePath = tab === "pack" ? setPackGamePath : setApplyGamePath;
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
	const [statusSnapshot, setStatusSnapshot] = useState<{
		gamePath: string;
		report: PatchStatusResult;
	} | null>(null);
	const status = statusSnapshot?.gamePath === gamePath.trim() ? statusSnapshot.report : null;
	const [errorSnapshot, setErrorSnapshot] = useState<{
		gamePath: string;
		message: string;
		technical?: string;
	} | null>(null);
	const error = errorSnapshot?.gamePath === gamePath.trim() ? errorSnapshot.message : null;
	const setError = (message: string | null, technical?: string) => {
		setErrorSnapshot(message === null ? null : { gamePath: gamePath.trim(), message, technical });
	};
	const [applying, setApplying] = useState(false);
	const [cancellingApply, setCancellingApply] = useState(false);
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
		setPackGamePath(defaultGamePath ?? "");
		setApplyGamePath(defaultGamePathIsFile ? "" : defaultGamePath ?? "");
		setRecordings([]);
		setPristineRecordings([]);
		setRecordingRoots({});
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
				setRecordingRoots(rec.roots ?? {});
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
	}, [open, defaultGamePath, defaultGamePathIsFile, initialZipPath, initialBackupId, initialTab, allowPack]);

	useEffect(() => {
		const root = recordingRoots[languages.trim()];
		if (root) setPackGamePath(root);
	}, [recordingRoots, languages]);

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
		// Capture the requested game before awaiting; selection can change in flight.
		const requestedGamePath = gamePath.trim();
		if (!requestedGamePath) return null;
		setError(null);
		setRollbackPreview(null);
		try {
			const report = await patchStatus({ game_path: requestedGamePath });
			setStatusSnapshot({ gamePath: requestedGamePath, report });
			onPatchStateChanged?.();
			return report;
		} catch (err: any) {
			setStatusSnapshot(null);
			setError(err.message);
			return null;
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
			const display = patchVerificationSummary(report);
			const summary = t(display.summaryKey, display.params);
			addLog(
				display.severity === "success" ? "info" : display.severity,
				summary,
				display.technical.join("\n"),
				"patch",
			);
			addToast(display.severity, summary);
		} catch (err: any) {
			const raw = err instanceof ApiError ? err.raw : err.message;
			const message = t("patch.toast.verifyFailed", { error: localizeApiError(raw) });
			setError(message, raw);
			addLog("error", message, raw, "patch");
			addToast("error", message);
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
		try {
			// A refusal or lost stream does not prove writes occurred. The journal
			// for the apply's game is the authority for recovery, even after selection changes.
			const currentStatus = await refreshStatus();
			const partial = currentStatus?.status === "interrupted";
			const unknown = !currentStatus || currentStatus.status === "unknown";
			const raw = opts.error ?? t("ws.patchJobStreamLost");
			const message = unknown ? t("patch.stateUnknown") : opts.cancelled
				? t(partial ? "patch.toast.applyCancelled" : "patch.toast.applyCancelledSafe")
				: t(partial ? "patch.toast.applyFailedPartial" : "patch.toast.applyFailed", { error: localizeApiError(raw) });
			setError(message, opts.error);
			if (opts.cancelled) {
				addLog(
					partial || unknown ? "warning" : "info",
					partial ? t("activity.patch.cancelledPartial") : message,
					opts.error,
					"patch",
				);
			} else {
				addLog("error", message, raw, "patch");
			}
			addToast(partial || opts.cancelled ? "warning" : "error", message);
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
		setStatusSnapshot(null);
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
						void finishApplyInterrupted({
							cancelled: applyCancelRequestedRef.current,
							error: t("ws.patchJobStreamLost"),
						});
					},
				},
				"patch",
			);
		} catch (err: any) {
			applyFinishedRef.current = true;
			await finishApplyInterrupted({ cancelled: false, error: err instanceof ApiError ? err.raw : err.message });
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
				pristine_path: pristine ? pristinePath.trim() || undefined : undefined,
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

	const showPartialWarning = status?.status === "interrupted";

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

	const applyProps = {
		t,
		zipPath,
		setZipPath,
		setZipUrl,
		pickZip,
		zipUrl,
		urlFieldError,
		urlZipHint,
		resolvedSource,
		force,
		setForce,
		confirmLegacy,
		setConfirmLegacy,
		dryRun,
		setDryRun,
		handleVerify,
		loading,
		applying,
		canVerifyApply,
		gamePath,
		sourceOk,
		handleApply,
		handlePreviewRollback,
		handleRollback,
		showPartialWarning,
		refreshStatus: async () => { await refreshStatus(); },
		applyProgress,
		handleCancelApply,
		cancellingApply,
		preview,
		status,
		patchValue,
		verify,
		applyResult
	};
	const packProps = {
		t,
		outputPath,
		setOutputPath,
		pickOutputZip,
		languages,
		setLanguages,
		recordings,
		pristine,
		setPristine,
		pristineRecordings,
		initialBackupId,
		gamePath,
		defaultGamePath,
		pristinePath,
		setPristinePath,
		pickPristineFolder,
		rjCode,
		rjEdited,
		setRjCode,
		detectId,
		setDetectId,
		gameVersion,
		setGameVersion,
		createEntry,
		setCreateEntry,
		entryPath,
		setEntryPathOverride,
		pickEntry,
		handlePack,
		loading,
		applying,
		packResult,
		patchValue,
		copyPublishCommand
	};

	return (
		<div className={MODAL_BACKDROP_CLASS}>
			<div
				ref={dialogRef}
				{...dialogProps}
				className={modalPanelClass("max-w-xl")}
			>
				<header className={`${MODAL_HEADER_CLASS} px-6 pt-6`}>
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

				</header>
				<div className={`${MODAL_BODY_CLASS} px-6 pb-6 space-y-4`}>
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
						<PatchApplyTab {...applyProps} />
					)}

					{tab === "pack" && (
						<PatchPackTab {...packProps} />
					)}

					{error && (
						<div className="flex gap-2 p-3 bg-danger-muted border border-danger rounded text-body text-danger">
							<AlertCircle size={16} className="shrink-0 mt-0.5" />
							<div className="min-w-0">
								<pre className="whitespace-pre-wrap break-words font-sans">
									{error}
								</pre>
								{errorSnapshot?.technical && (
									<details className="mt-2">
										<summary className="cursor-pointer">{t("patch.technicalDetails")}</summary>
										<pre className="whitespace-pre-wrap break-words">{errorSnapshot.technical}</pre>
									</details>
								)}
							</div>
						</div>
					)}
				</div>
				<footer className={MODAL_FOOTER_CLASS}>
					{tab === "apply" ? <PatchApplyActions {...applyProps} /> : <PatchPackActions {...packProps} />}
				</footer>
			</div>
		</div>
	);
}
