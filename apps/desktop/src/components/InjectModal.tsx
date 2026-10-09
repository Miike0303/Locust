import { IS_TAURI } from "../lib/runtime";
import { useEffect, useMemo, useState } from "react";
import { X, FolderOpen, FileCheck, AlertCircle, Package } from "lucide-react";
import { useNavigate } from "react-router-dom";
import {
	inject,
	registerLang,
	validate,
	type MultiLangReport,
	type RegisterLangReport,
} from "../lib/api";
import {
	availableInjectModes,
	coerceInjectMode,
	defaultInjectMode,
	type InjectUiMode,
} from "../lib/injectModes";
import {
	classifyInjectReport,
	collectFilesWritten,
	collectInjectWarnings,
	injectionRecoveryPath,
	isRedundantBackupRemoved,
	collectSkipReasons,
	skipReasonLabel,
	injectToastLevel,
	outcomeRecordingIssues,
	shouldOfferPackAfterInject,
	sumFilesModified,
	sumStringsSkipped,
	sumStringsWritten,
} from "../lib/injectOutcome";
import { LANGUAGES, languageLabel } from "../lib/languages";
import {
	loadRegLabelOverride,
	rememberRegLabelOverride,
} from "../lib/registerLangPrefs";
import { useProjectStore } from "../stores/projectStore";
import { addLog } from "../stores/logStore";
import { addToast } from "../stores/toastStore";
import {
	useModalA11y,
	MODAL_BACKDROP_CLASS,
	modalPanelClass,
} from "../lib/modalA11y";
import { operationalShortcutTarget } from "../lib/settingsNav";
import { useT } from "../lib/i18n";

const INJECT_LANG_KEY = "locust.inject.langs";

interface InjectModalProps {
	open: boolean;
	onClose: () => void;
	/** Optional: open Patch modal on the Pack tab after a successful direct inject. */
	onOpenPack?: (backupId?: string) => void;
}

export default function InjectModal({
	open,
	onClose,
	onOpenPack,
}: InjectModalProps) {
	const t = useT();
	const navigate = useNavigate();
	const { project } = useProjectStore();
	const { dialogRef, dialogProps, titleProps } = useModalA11y({
		open: open && !!project,
		ownEscape: false,
	});
	const injectModes = useMemo(
		() => availableInjectModes(project?.supported_modes),
		[project?.supported_modes],
	);
	const [mode, setMode] = useState<InjectUiMode>(() =>
		defaultInjectMode(undefined),
	);
	// When project/format modes change, drop illegal selections (e.g. Add on Unity).
	useEffect(() => {
		setMode((m) => coerceInjectMode(m, project?.supported_modes));
	}, [project?.supported_modes, project?.format_id]);
	const savedLangs = (() => {
		try {
			return JSON.parse(localStorage.getItem(INJECT_LANG_KEY) || "null") as
				| string[]
				| null;
		} catch {
			return null;
		}
	})();
	const [selectedLangs, setSelectedLangs] = useState<string[]>(
		savedLangs?.length ? savedLangs.slice(0, 1) : ["es"],
	);
	const [outputDir, setOutputDir] = useState("");
	const [loading, setLoading] = useState(false);
	const [result, setResult] = useState<MultiLangReport | null>(null);
	const [regLoading, setRegLoading] = useState(false);
	const [regReports, setRegReports] = useState<
		{ lang: string; label: string; report: RegisterLangReport }[]
	>([]);
	// A completed report belongs to this visit. Keep an in-flight operation
	// alive when hidden, then clear its result before the next visit.
	useEffect(() => {
		if (!open && !loading && !regLoading) {
			setResult(null);
			setRegReports([]);
		}
	}, [open, loading, regLoading]);
	/** Optional UI label override (CLI `--label`). Used when a single lang is selected. */
	const [regLabelOverride, setRegLabelOverride] = useState(() =>
		loadRegLabelOverride(),
	);
	/** After inject, also run register-lang for RPG Maker multi-lang UI. */
	const [autoRegisterAfterInject, setAutoRegisterAfterInject] = useState(() => {
		try {
			return localStorage.getItem("locust.inject.autoRegister") === "1";
		} catch {
			return false;
		}
	});

	const setRegLabelAndRemember = (value: string) => {
		setRegLabelOverride(value);
		rememberRegLabelOverride(value);
	};

	const toggleLang = (code: string) => {
		setSelectedLangs([code]);
	};

	const defaultLabelFor = (code: string) => languageLabel(code);

	/** Resolve menu label for register-lang (override only when one language is selected). */
	const labelForRegister = (code: string) => {
		const override = regLabelOverride.trim();
		if (override && selectedLangs.length === 1 && selectedLangs[0] === code) {
			return override;
		}
		return defaultLabelFor(code);
	};

	if (!open || !project) return null;

	const handlePickFolder = async () => {
		if (IS_TAURI) {
			const { open: openDialog } = await import("@tauri-apps/plugin-dialog");
			const selected = await openDialog({
				title: t("inject.dialog.outputFolder"),
				directory: true,
			});
			if (typeof selected === "string") setOutputDir(selected);
		} else {
			const path = prompt(t("inject.prompt.outputFolder"));
			if (path) setOutputDir(path);
		}
	};

	const canInject =
		selectedLangs.length > 0 &&
		(mode === "direct" ||
			mode === "add" ||
			(mode === "replace" && outputDir.trim() !== ""));

	const isRpgMaker =
		project.format_id === "rpgmaker-mv" ||
		project.format_id === "rpgmaker-mz" ||
		project.format_id.startsWith("rpgmaker");

	const runRegisterLang = async (quiet = false): Promise<boolean> => {
		if (selectedLangs.length === 0) {
			if (!quiet) addToast("error", t("inject.toast.selectLangRegister"));
			return false;
		}
		setRegLoading(true);
		setRegReports([]);
		const done: { lang: string; label: string; report: RegisterLangReport }[] =
			[];
		try {
			for (const code of selectedLangs) {
				const label = labelForRegister(code);
				const report = await registerLang({
					game_path: project.path,
					lang: code,
					label,
				});
				done.push({ lang: code, label, report });
				addLog(
					"info",
					t("activity.inject.registered", { code, label }),
					`plugins_js=${report.plugins_js} iavra=${report.iavra_languages} visumz=${report.visumz_options} maps=${report.maps_patched?.length ?? 0}` +
						(report.notes?.length ? `\n${report.notes.join("\n")}` : ""),
					"inject",
				);
			}
			setRegReports(done);
			const anyChange = done.some(
				(d) =>
					d.report.plugins_js ||
					d.report.iavra_languages ||
					d.report.visumz_options ||
					(d.report.maps_patched?.length ?? 0) > 0,
			);
			if (anyChange) {
				addToast(
					"success",
					t("inject.toast.registered", { count: done.length }),
				);
			} else {
				addToast(
					"warning",
					t("inject.toast.noPatterns"),
					8000,
				);
			}
			return anyChange;
		} catch (err: unknown) {
			const msg = err instanceof Error ? err.message : String(err);
			addLog("error", t("activity.inject.registerFailed"), msg, "inject");
			addToast("error", t("inject.toast.registerFailed", { error: msg }));
			return false;
		} finally {
			setRegLoading(false);
		}
	};

	const handleRegisterLang = async () => {
		await runRegisterLang(false);
	};

	const handleInject = async () => {
		if (mode === "replace" && !outputDir.trim()) {
			addToast("error", t("inject.toast.selectOutput"));
			return;
		}
		if (selectedLangs.length === 0) {
			addToast("error", t("inject.toast.selectLang"));
			return;
		}
		try {
			localStorage.setItem(INJECT_LANG_KEY, JSON.stringify(selectedLangs));
		} catch {
			/* ignore */
		}

		setLoading(true);
		setResult(null);
		try {
			try {
				const pre = await validate();
				const binary = pre.validation.by_kind?.ExceedsBinarySlot ?? 0;
				if (binary > 0) {
					addToast(
						"warning",
						t("inject.toast.binarySlots", { count: binary }),
						8000,
					);
					addLog(
						"warning",
						t("activity.inject.preflight", { count: binary }),
						"UTF-8 / UTF-16LE / Shift-JIS length must be ≤ source",
						"inject",
					);
				}
			} catch {
				/* validate optional */
			}

			const isDirect = mode === "direct";
			const report = await inject({
				project_path: project.path,
				format_id: project.format_id,
				mode: isDirect ? undefined : mode,
				languages: selectedLangs,
				output_dir: isDirect ? undefined : outputDir.trim() || undefined,
				direct: isDirect,
			});
			setResult(report);

			const outcome = classifyInjectReport(report);
			const written = sumStringsWritten(report);
			const warnings = collectInjectWarnings(report);
			const filesWritten = collectFilesWritten(report);
			const destInfo = isDirect
				? `Direct inject into ${project.path}` +
					(report.backup_path ? `\nBackup: ${report.backup_path}` : "")
				: mode === "replace"
					? `Output: ${outputDir}`
					: `Added translation folders in ${project.path}`;
			const failDetail = report.languages_failed?.length
				? `Failed: ${report.languages_failed.map(([l, e]) => `${l}: ${e}`).join(", ")}`
				: "No language failures";
			const warnDetail = warnings.length
				? `\nWarnings:\n${warnings.map((w) => `- ${w}`).join("\n")}`
				: "";
			const filesDetail = filesWritten.length
				? `\nFiles written:\n${filesWritten.map((f) => `- ${f}`).join("\n")}`
				: "";

			addLog(
				outcome === "empty" ? "error" : outcome === "partial" ? "warning" : "info",
				t(
					outcome === "unchanged" ? "activity.inject.unchanged"
						: outcome === "empty" ? "activity.inject.empty"
						: outcome === "partial" ? "activity.inject.partial"
						: "activity.inject.complete",
					{
						languages: report.languages_processed.join(", ") || t("inject.selectedNone"),
						mode: report.mode === "replace" ? t("activity.inject.modeReplace")
							: report.mode === "add" ? t("activity.inject.modeAdd")
							: report.mode === "direct" ? t("activity.inject.modeDirect") : report.mode,
					},
				),
				`${destInfo}\nStrings written: ${written}\n${failDetail}${warnDetail}${filesDetail}`,
				"inject",
			);

			const toastLevel = injectToastLevel(outcome);
			const toastMsg =
				outcome === "unchanged"
					? t("inject.toast.unchanged")
					: outcome === "empty"
					? t("inject.toast.nothingWritten")
					: outcome === "partial"
						? t("inject.toast.partial", {
								written,
								failed: report.languages_failed?.length ?? 0,
						  })
						: isDirect
							? t("inject.toast.direct", { count: written })
							: t("inject.toast.injected", {
									count: report.languages_processed.length,
							  });
			addToast(toastLevel, toastMsg);

			// Optional: register selected lang(s) in RM multi-lang UI after inject.
			// Skip when nothing was written — registering a language with no text is noise.
			if (autoRegisterAfterInject && isRpgMaker && written > 0) {
				await runRegisterLang(true);
			}
		} catch (err: unknown) {
			const msg = err instanceof Error ? err.message : String(err);
			addLog("error", t("activity.inject.failed"), msg, "inject");
			addToast("error", t("inject.toast.failed", { error: msg }));
		} finally {
			setLoading(false);
		}
	};

	const gameName =
		project.path.split(/[\\/]/).filter(Boolean).pop() ?? project.name;
	const isDirectResult = result?.mode === "direct";
	const resultKind = result ? classifyInjectReport(result) : null;
	const resultDiagnostics = result ? collectInjectWarnings(result) : [];
	const resultWarnings = resultDiagnostics.filter((message) => !injectionRecoveryPath(message) && !isRedundantBackupRemoved(message));
	const removedRedundantBackup = resultDiagnostics.some(isRedundantBackupRemoved);
	const recoveryPaths = [...new Set(resultDiagnostics.map(injectionRecoveryPath).filter((path): path is string => path !== null))];
	const resultFiles = result ? collectFilesWritten(result) : [];
	const resultRecording = result ? outcomeRecordingIssues(result) : [];
	const resultWritten = result ? sumStringsWritten(result) : 0;
	const resultSkips = result ? collectSkipReasons(result) : [];
	const offerPack =
		Boolean(result) && isDirectResult && shouldOfferPackAfterInject(result!);

	return (
		<div className={MODAL_BACKDROP_CLASS}>
			<div
				ref={dialogRef}
				{...dialogProps}
				className={modalPanelClass("max-w-lg p-6 max-h-[90vh] overflow-y-auto")}
			>
				<div className="flex justify-between items-center mb-4">
					<h2 {...titleProps} className="text-section font-bold">
						{t("inject.title")}
					</h2>
					<button
						onClick={onClose}
						aria-label={t("common.close")}
						className="text-text-muted hover:text-text"
					>
						<X size={20} />
					</button>
				</div>

				{!result ? (
					<div className="space-y-4">
						<div>
							<label htmlFor="inject-mode" className="text-body font-medium">{t("inject.mode")}</label>
							<select
								id="inject-mode"
								value={mode}
								onChange={(e) => setMode(e.target.value as InjectUiMode)}
								className="mt-1 w-full p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
							>
								{injectModes.includes("replace") && (
									<option value="replace">
										{t("inject.mode.replace")}
									</option>
								)}
								{injectModes.includes("add") && (
									<option value="add">
										{t("inject.mode.add")}
									</option>
								)}
								{injectModes.includes("direct") && (
									<option value="direct">
										{t("inject.mode.direct")}
									</option>
								)}
							</select>
							<p className="text-caption text-text-muted mt-1">
								{mode === "replace" &&
									t("inject.hint.replace", { gameName })}
								{mode === "add" && t("inject.hint.add")}
								{mode === "direct" && t("inject.hint.direct")}
								{!injectModes.includes("add") && (
									<span className="block mt-0.5">
										{t("inject.hint.noAdd")}
									</span>
								)}
							</p>
						</div>

						{mode === "direct" && (
							<div className="p-3 bg-warning-muted border border-warning rounded text-body text-warning space-y-1">
								<p className="font-medium flex items-center gap-1.5">
									<AlertCircle size={16} /> {t("inject.direct.warnTitle")}
								</p>
								<ul className="list-disc pl-5 text-caption space-y-0.5">
									<li>
										{t("inject.direct.li1")}
									</li>
									<li>
										{t("inject.direct.li2")}
									</li>
									<li>
										{t("inject.direct.li3")}
									</li>
								</ul>
							</div>
						)}

						<div>
							<label className="text-body font-medium">
								{t("inject.languages")}
								{mode === "direct" && (
									<span className="font-normal text-text-muted">
										{" "}
										{t("inject.recordingKey")}
									</span>
								)}
							</label>
							<div className="mt-1 grid grid-cols-3 gap-2 p-2 border border-border rounded bg-surface-muted max-h-40 overflow-y-auto">
								{LANGUAGES.map((l) => (
									<label
										key={l.code}
										className="flex items-center gap-1 text-body cursor-pointer"
									>
										<input
											type="radio"
											name="inject-target-language"
											checked={selectedLangs.includes(l.code)}
											onChange={() => toggleLang(l.code)}
										/>
										<span>{l.label}</span>
									</label>
								))}
							</div>
							<p className="text-caption text-text-muted mt-1">
								{t("inject.selected", {
									langs:
										selectedLangs.length === 0
											? t("inject.selectedNone")
											: selectedLangs.join(", "),
								})}
								<span className="block mt-0.5">
									{t("inject.multiRecording")}
								</span>
							</p>
						</div>

						{mode === "replace" && (
							<div>
								<label className="text-body font-medium">
									{t("inject.outputFolder")} <span className="text-danger">*</span>
								</label>
								<div className="flex gap-2 mt-1">
									<input
										value={outputDir}
										onChange={(e) => setOutputDir(e.target.value)}
										placeholder={t("inject.outputPlaceholder")}
										className="flex-1 p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
									/>
									<button
										onClick={handlePickFolder}
										className="px-3 py-2 bg-surface-muted text-text hover:bg-surface rounded text-body transition-colors"
										title={t("inject.browseTitle")}
									>
										<FolderOpen size={16} />
									</button>
								</div>
								{!outputDir.trim() && (
									<p className="flex items-center gap-1 text-caption text-warning mt-1">
										<AlertCircle size={12} />
										{t("inject.outputRequired")}
									</p>
								)}
								{outputDir.trim() && (
									<p className="text-caption text-text-muted mt-1">
										{t("inject.willCreate", {
											path: `${outputDir}/${gameName}-${selectedLangs[0] || "lang"}/`,
										})}
									</p>
								)}
							</div>
						)}

						<div className="pt-2 flex items-center gap-3 text-caption text-text-muted">
							<FileCheck size={14} />
							<span>
								{t("inject.source", { name: project.name, format: project.format_id })}
							</span>
						</div>

						<button
							onClick={handleInject}
							disabled={loading || !canInject}
							className="w-full py-2 bg-accent hover:bg-accent-hover disabled:opacity-50 disabled:cursor-not-allowed text-white rounded font-medium transition-colors"
						>
							{loading
								? t("inject.injecting")
								: mode === "direct"
									? t("inject.directAction")
									: t("inject.injectAction")}
						</button>

						{isRpgMaker && (
							<div className="pt-1 border-t border-border space-y-2">
								<p className="text-caption text-text-muted">
									{t("inject.rpgHint")}
								</p>
								<label className="flex items-center gap-2 text-caption text-text-muted cursor-pointer">
									<input
										type="checkbox"
										checked={autoRegisterAfterInject}
										onChange={(e) => {
											const on = e.target.checked;
											setAutoRegisterAfterInject(on);
											try {
												localStorage.setItem(
													"locust.inject.autoRegister",
													on ? "1" : "0",
												);
											} catch {
												/* ignore */
											}
										}}
									/>
									{t("inject.autoRegister")}
								</label>
								{selectedLangs.length === 1 && (
									<div>
										<label className="text-caption font-medium text-text-muted">
											{t("inject.menuLabel")}
										</label>
										<input
											type="text"
											value={regLabelOverride}
											onChange={(e) => setRegLabelAndRemember(e.target.value)}
											placeholder={defaultLabelFor(selectedLangs[0])}
											className="w-full mt-0.5 p-1.5 text-body border border-border rounded bg-surface text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
										/>
										<p className="text-caption text-text-muted mt-0.5">
											{t("inject.menuLabelHint", {
												label: defaultLabelFor(selectedLangs[0]),
											})}
										</p>
									</div>
								)}
								<button
									type="button"
									onClick={handleRegisterLang}
									disabled={regLoading || selectedLangs.length === 0}
									className="w-full py-1.5 text-body border border-accent-fg text-accent-fg hover:bg-accent-muted disabled:opacity-50 rounded font-medium"
								>
									{regLoading
										? t("inject.registering")
										: t("inject.registerOnly", {
												langs: selectedLangs.join(", ") || "lang",
											})}
								</button>
								{regReports.length > 0 && (
									<div className="text-caption text-accent-fg space-y-0.5">
										{regReports.map(({ lang, label, report }) => (
											<p key={lang}>
												{t("inject.regReport", {
													lang,
													label,
													plugins: report.plugins_js ? t("common.yes") : t("common.no"),
													maps: report.maps_patched?.length ?? 0,
												})}
											</p>
										))}
									</div>
								)}
							</div>
						)}
					</div>
				) : (
					<div className="space-y-4">
						<div
							className={
								resultKind === "unchanged"
									? "p-3 bg-sky-50 dark:bg-sky-950/30 border border-sky-200 dark:border-sky-800 rounded text-body"
									: resultKind === "empty"
									? "p-3 bg-danger-muted border border-danger rounded text-body"
									: resultKind === "partial"
										? "p-3 bg-warning-muted border border-warning rounded text-body"
										: "p-3 bg-success-muted border border-success rounded text-body"
							}
						>
							<p
								className={
									resultKind === "unchanged"
										? "font-medium text-sky-800 dark:text-sky-200"
										: resultKind === "empty"
										? "font-medium text-danger"
										: resultKind === "partial"
											? "font-medium text-warning"
											: "font-medium text-success"
								}
							>
								{resultKind === "unchanged"
									? t("inject.injectionUnchanged")
									: resultKind === "empty"
									? t("inject.injectionEmpty")
									: resultKind === "partial"
										? t("inject.injectionPartial")
										: t("inject.injectionComplete")}
							</p>
							<p className="text-body mt-1 opacity-90">
								{t("inject.languagesLine", {
									langs: result.languages_processed.join(", ") || t("inject.selectedNone"),
								})}
							</p>
							<p className="text-body opacity-90">
								{t("inject.modeLine", { mode: result.mode })}
							</p>
							<p className="text-body opacity-90">
								{t("inject.stringsWritten", {
									written: resultWritten,
									files: sumFilesModified(result),
									skipped: sumStringsSkipped(result),
								})}
							</p>
							{isDirectResult && result.backup_path && (
								<p className="text-caption mt-1 break-all opacity-90">
									{t("inject.backup", { path: result.backup_path })}
								</p>
							)}
							{!isDirectResult && mode === "replace" && outputDir && (
								<p className="text-body opacity-90">
									{t("inject.output", { path: outputDir })}
								</p>
							)}
							{result.reports && Object.keys(result.reports).length > 0 && (
								<div className="mt-2 space-y-1">
									{Object.entries(result.reports).map(([lang, report]) => (
										<p key={lang} className="text-caption opacity-80">
											{t("inject.langReport", {
												lang,
												written:
													(report as { strings_written?: number })
														.strings_written ?? 0,
												files:
													(report as { files_modified?: number })
														.files_modified ?? 0,
											})}
										</p>
									))}
								</div>
							)}
						</div>

						{resultSkips.length > 0 && (
                            <div className="p-3 bg-surface-muted rounded text-body">
                                <p className="font-medium">{t("inject.skipReasons")}</p>
                                <ul>{resultSkips.map(({lang, reason, count}) => {
                                    const label = skipReasonLabel(reason);
                                    return <li key={`${lang}:${reason}`}>{lang && `${lang}: `}{t(label.key, label.vars)}: {count}</li>;
                                })}</ul>
                            </div>
                        )}
						{resultFiles.length > 0 && (
							<div className="p-3 bg-surface-muted border border-border rounded text-body">
								<p className="font-medium text-text">
									{t("inject.filesWritten")}
								</p>
								<ul className="mt-1 space-y-0.5 text-caption text-text-muted break-all">
									{resultFiles.map((path) => (
										<li key={path}>{path}</li>
									))}
								</ul>
							</div>
						)}

						{recoveryPaths.length > 0 && (
							<div className="p-3 bg-surface-muted border border-border rounded text-body text-text">
								<p className="font-medium">{t("inject.recoveryOriginals")}</p>
								<p className="text-caption mt-1">{t("inject.recoveryOriginalsHint")}</p>
								<ul className="mt-1 text-caption break-all">{recoveryPaths.map((path) => <li key={path}>{path}</li>)}</ul>
							</div>
						)}

						{resultWarnings.length > 0 && (
							<div className="p-3 bg-warning-muted border border-warning rounded text-body">
								<p className="font-medium text-warning flex items-center gap-1.5">
									<AlertCircle size={16} /> {t("inject.warnings")}
								</p>
								<ul className="mt-1 space-y-0.5 text-caption text-warning">
									{resultWarnings.map((w) => (
										<li key={w}>{w}</li>
									))}
								</ul>
							</div>
						)}

						{removedRedundantBackup && (
							<p className="text-body text-text-muted">{t("inject.redundantBackupRemoved")}</p>
						)}

						{resultRecording.length > 0 && (
							<div className={resultKind === "unchanged"
								? "p-3 bg-surface-muted border border-border rounded text-body text-text"
								: "p-3 bg-warning-muted border border-warning rounded text-body text-warning"}>
								<p className="font-medium">
									{t("inject.recordingIssues")}
								</p>
								<ul className="mt-1 space-y-0.5 text-caption">
									{resultRecording.map((issue) => (
										<li key={`${issue.lang}-${issue.kind}`}>
											{issue.kind === "nothing"
												? t("inject.recordingNothing", { lang: issue.lang })
												: t("inject.recordingKept", {
														lang: issue.lang,
														at: issue.detail ?? "",
												  })}
										</li>
									))}
								</ul>
							</div>
						)}

						{offerPack && (
							<div className="p-3 bg-sky-50 dark:bg-sky-950/30 border border-sky-200 dark:border-sky-800 rounded text-body text-sky-900 dark:text-sky-100">
								<p className="font-medium flex items-center gap-1.5">
									<Package size={16} /> {t("inject.recordingSaved")}
								</p>
								<p className="text-caption mt-1">
									{t("inject.recordingHint")}
								</p>
								{onOpenPack && (
									<button
										type="button"
										onClick={() => {
											onClose();
											onOpenPack(result?.pristine_backup_id || undefined);
										}}
										className="mt-2 text-caption font-medium text-sky-700 dark:text-sky-300 hover:underline"
									>
										{t("inject.openPack")}
									</button>
								)}
							</div>
						)}

						{result.languages_failed?.length > 0 && (
							<div className="p-3 bg-danger-muted border border-danger rounded text-body">
								<p className="font-medium text-danger">{t("inject.failedLanguages")}</p>
								{result.languages_failed.map(([lang, err]) => (
									<p key={lang} className="text-danger text-caption mt-1">
										{lang}: {err}
									</p>
								))}
							</div>
						)}

						{isRpgMaker && (
							<div className="p-3 bg-accent-muted border border-accent-fg rounded text-body space-y-2">
								<p className="font-medium text-accent-fg">
									{t("inject.registerInUi")}
								</p>
								<p className="text-caption text-accent-fg">
									{t("inject.registerInUiHint")}
									<code className="px-0.5">*.bak-locust</code>
									).
								</p>
								{selectedLangs.length === 1 && (
									<div>
										<label className="text-caption font-medium text-accent-fg">
											{t("inject.menuLabel")}
										</label>
										<input
											type="text"
											value={regLabelOverride}
											onChange={(e) => setRegLabelAndRemember(e.target.value)}
											placeholder={defaultLabelFor(selectedLangs[0])}
											className="w-full mt-0.5 p-1.5 text-body border border-border rounded bg-surface text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
										/>
										<p className="text-caption text-accent-fg mt-0.5">
											{t("inject.menuLabelHintShort", {
												label: defaultLabelFor(selectedLangs[0]),
											})}
										</p>
									</div>
								)}
								<button
									type="button"
									onClick={handleRegisterLang}
									disabled={regLoading || selectedLangs.length === 0}
									className="w-full py-1.5 text-body bg-accent hover:bg-accent-hover disabled:opacity-50 text-white rounded font-medium"
								>
									{regLoading
										? t("inject.registering")
										: t("inject.registerUi", {
												langs: selectedLangs.join(", ") || "lang",
											})}
								</button>
								{regReports.length > 0 && (
									<div className="text-caption text-accent-fg space-y-1">
										{regReports.map(({ lang, label, report }) => (
											<p key={lang}>
												{t("inject.regReportFull", {
													lang,
													label,
													plugins: report.plugins_js ? t("common.yes") : t("common.no"),
													maps: report.maps_patched?.length ?? 0,
													backups: report.backups?.length ?? 0,
												})}
												{report.notes?.length
													? ` — ${report.notes.slice(0, 2).join("; ")}`
													: ""}
											</p>
										))}
									</div>
								)}
							</div>
						)}

						<button
							type="button"
							onClick={() => {
								onClose();
								navigate(operationalShortcutTarget("manage-backups").path);
							}}
							className="text-caption font-medium text-accent-fg hover:underline"
						>
							{t("inject.manageBackups")}
						</button>
						<button
							onClick={onClose}
							className="w-full py-2 bg-accent hover:bg-accent-hover text-white rounded font-medium"
						>
							{t("common.close")}
						</button>
					</div>
				)}
			</div>
		</div>
	);
}
