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

const IS_TAURI = "__TAURI_INTERNALS__" in window;

const INJECT_LANG_KEY = "locust.inject.langs";

interface InjectModalProps {
	open: boolean;
	onClose: () => void;
	/** Optional: open Patch modal on the Pack tab after a successful direct inject. */
	onOpenPack?: () => void;
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
		savedLangs ?? ["es"],
	);
	const [outputDir, setOutputDir] = useState("");
	const [loading, setLoading] = useState(false);
	const [result, setResult] = useState<MultiLangReport | null>(null);
	const [regLoading, setRegLoading] = useState(false);
	const [regReports, setRegReports] = useState<
		{ lang: string; label: string; report: RegisterLangReport }[]
	>([]);
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
		setSelectedLangs((prev) =>
			prev.includes(code) ? prev.filter((l) => l !== code) : [...prev, code],
		);
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
					`Registered ${code} (${label}) in game UI`,
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
			addLog("error", "Could not register language in game UI", msg, "inject");
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
						`Inject preflight: ${binary} ExceedsBinarySlot`,
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

			const destInfo = isDirect
				? `Direct inject into ${project.path}` +
					(report.backup_path ? `\nBackup: ${report.backup_path}` : "")
				: mode === "replace"
					? `Output: ${outputDir}`
					: `Added translation folders in ${project.path}`;

			addLog(
				"info",
				`Inject complete: ${report.languages_processed.join(", ")} (${report.mode} mode)`,
				`${destInfo}\n${
					report.languages_failed?.length
						? `Failed: ${report.languages_failed.map(([l, e]) => `${l}: ${e}`).join(", ")}`
						: "All languages succeeded"
				}`,
				"inject",
			);
			addToast(
				"success",
				isDirect
					? t("inject.toast.direct", { count: report.strings_written ?? 0 })
					: t("inject.toast.injected", { count: report.languages_processed.length }),
			);

			// Optional: register selected lang(s) in RM multi-lang UI after inject.
			if (autoRegisterAfterInject && isRpgMaker) {
				await runRegisterLang(true);
			}
		} catch (err: unknown) {
			const msg = err instanceof Error ? err.message : String(err);
			addLog("error", "Inject failed", msg, "inject");
			addToast("error", t("inject.toast.failed", { error: msg }));
		} finally {
			setLoading(false);
		}
	};

	const gameName =
		project.path.split(/[\\/]/).filter(Boolean).pop() ?? project.name;
	const isDirectResult = result?.mode === "direct";

	return (
		<div className={MODAL_BACKDROP_CLASS}>
			<div
				ref={dialogRef}
				{...dialogProps}
				className={modalPanelClass("max-w-lg p-6 max-h-[90vh] overflow-y-auto")}
			>
				<div className="flex justify-between items-center mb-4">
					<h2 {...titleProps} className="text-lg font-bold">
						{t("inject.title")}
					</h2>
					<button
						onClick={onClose}
						className="text-gray-400 hover:text-gray-600"
					>
						<X size={20} />
					</button>
				</div>

				{!result ? (
					<div className="space-y-4">
						<div>
							<label className="text-sm font-medium">{t("inject.mode")}</label>
							<select
								value={mode}
								onChange={(e) => setMode(e.target.value as InjectUiMode)}
								className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
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
							<p className="text-xs text-gray-500 mt-1">
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
							<div className="p-3 bg-amber-50 dark:bg-amber-950/40 border border-amber-200 dark:border-amber-800 rounded text-sm text-amber-900 dark:text-amber-100 space-y-1">
								<p className="font-medium flex items-center gap-1.5">
									<AlertCircle size={16} /> {t("inject.direct.warnTitle")}
								</p>
								<ul className="list-disc pl-5 text-xs space-y-0.5">
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
							<label className="text-sm font-medium">
								{t("inject.languages")}
								{mode === "direct" && (
									<span className="font-normal text-gray-500">
										{" "}
										{t("inject.recordingKey")}
									</span>
								)}
							</label>
							<div className="mt-1 grid grid-cols-3 gap-2 p-2 border rounded dark:border-gray-600 max-h-40 overflow-y-auto">
								{LANGUAGES.map((l) => (
									<label
										key={l.code}
										className="flex items-center gap-1 text-sm cursor-pointer"
									>
										<input
											type="checkbox"
											checked={selectedLangs.includes(l.code)}
											onChange={() => toggleLang(l.code)}
										/>
										<span>{l.label}</span>
									</label>
								))}
							</div>
							<p className="text-xs text-gray-500 mt-1">
								{t("inject.selected", {
									langs:
										selectedLangs.length === 0
											? t("inject.selectedNone")
											: selectedLangs.join(", "),
								})}
								{mode === "direct" && selectedLangs.length > 1 && (
									<span className="block mt-0.5">
										{t("inject.multiRecording")}
									</span>
								)}
							</p>
						</div>

						{mode === "replace" && (
							<div>
								<label className="text-sm font-medium">
									{t("inject.outputFolder")} <span className="text-red-500">*</span>
								</label>
								<div className="flex gap-2 mt-1">
									<input
										value={outputDir}
										onChange={(e) => setOutputDir(e.target.value)}
										placeholder={t("inject.outputPlaceholder")}
										className="flex-1 p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
									/>
									<button
										onClick={handlePickFolder}
										className="px-3 py-2 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded text-sm transition-colors"
										title={t("inject.browseTitle")}
									>
										<FolderOpen size={16} />
									</button>
								</div>
								{!outputDir.trim() && (
									<p className="flex items-center gap-1 text-xs text-amber-500 mt-1">
										<AlertCircle size={12} />
										{t("inject.outputRequired")}
									</p>
								)}
								{outputDir.trim() && (
									<p className="text-xs text-gray-500 mt-1">
										{t("inject.willCreate", {
											path: `${outputDir}/${gameName}-${selectedLangs[0] || "lang"}/`,
										})}
									</p>
								)}
							</div>
						)}

						<div className="pt-2 flex items-center gap-3 text-xs text-gray-500">
							<FileCheck size={14} />
							<span>
								{t("inject.source", { name: project.name, format: project.format_id })}
							</span>
						</div>

						<button
							onClick={handleInject}
							disabled={loading || !canInject}
							className="w-full py-2 bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50 disabled:cursor-not-allowed text-white rounded font-medium transition-colors"
						>
							{loading
								? t("inject.injecting")
								: mode === "direct"
									? t("inject.directAction")
									: t("inject.injectAction")}
						</button>

						{isRpgMaker && (
							<div className="pt-1 border-t dark:border-gray-700 space-y-2">
								<p className="text-xs text-gray-500">
									{t("inject.rpgHint")}
								</p>
								<label className="flex items-center gap-2 text-xs text-gray-600 dark:text-gray-400 cursor-pointer">
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
										<label className="text-xs font-medium text-gray-600 dark:text-gray-400">
											{t("inject.menuLabel")}
										</label>
										<input
											type="text"
											value={regLabelOverride}
											onChange={(e) => setRegLabelAndRemember(e.target.value)}
											placeholder={defaultLabelFor(selectedLangs[0])}
											className="w-full mt-0.5 p-1.5 text-sm border rounded dark:bg-gray-800 dark:border-gray-600"
										/>
										<p className="text-[11px] text-gray-500 mt-0.5">
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
									className="w-full py-1.5 text-sm border border-violet-300 dark:border-violet-700 text-violet-800 dark:text-violet-200 hover:bg-violet-50 dark:hover:bg-violet-950/40 disabled:opacity-50 rounded font-medium"
								>
									{regLoading
										? t("inject.registering")
										: t("inject.registerOnly", {
												langs: selectedLangs.join(", ") || "lang",
											})}
								</button>
								{regReports.length > 0 && (
									<div className="text-xs text-violet-700 dark:text-violet-300 space-y-0.5">
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
						<div className="p-3 bg-emerald-50 dark:bg-emerald-900/20 border border-emerald-200 dark:border-emerald-800 rounded text-sm">
							<p className="font-medium text-emerald-700 dark:text-emerald-300">
								{t("inject.injectionComplete")}
							</p>
							<p className="text-emerald-600 dark:text-emerald-400 mt-1">
								{t("inject.languagesLine", {
									langs: result.languages_processed.join(", "),
								})}
							</p>
							<p className="text-emerald-600 dark:text-emerald-400">
								{t("inject.modeLine", { mode: result.mode })}
							</p>
							{isDirectResult && (
								<>
									<p className="text-emerald-600 dark:text-emerald-400">
										{t("inject.stringsWritten", {
											written: result.strings_written ?? 0,
											files: result.files_modified ?? 0,
											skipped: result.strings_skipped ?? 0,
										})}
									</p>
									{result.backup_path && (
										<p className="text-xs text-emerald-700 dark:text-emerald-300 mt-1 break-all">
											{t("inject.backup", { path: result.backup_path })}
										</p>
									)}
								</>
							)}
							{!isDirectResult && mode === "replace" && outputDir && (
								<p className="text-emerald-600 dark:text-emerald-400">
									{t("inject.output", { path: outputDir })}
								</p>
							)}
							{result.reports && Object.keys(result.reports).length > 0 && (
								<div className="mt-2 space-y-1">
									{Object.entries(result.reports).map(([lang, report]) => (
										<p key={lang} className="text-xs text-emerald-500">
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

						{isDirectResult && (
							<div className="p-3 bg-sky-50 dark:bg-sky-950/30 border border-sky-200 dark:border-sky-800 rounded text-sm text-sky-900 dark:text-sky-100">
								<p className="font-medium flex items-center gap-1.5">
									<Package size={16} /> {t("inject.recordingSaved")}
								</p>
								<p className="text-xs mt-1">
									{t("inject.recordingHint")}
								</p>
								{onOpenPack && (
									<button
										type="button"
										onClick={() => {
											onClose();
											onOpenPack();
										}}
										className="mt-2 text-xs font-medium text-sky-700 dark:text-sky-300 hover:underline"
									>
										{t("inject.openPack")}
									</button>
								)}
							</div>
						)}

						{result.languages_failed?.length > 0 && (
							<div className="p-3 bg-red-50 dark:bg-red-900/20 border border-red-200 rounded text-sm">
								<p className="font-medium text-red-700">{t("inject.failedLanguages")}</p>
								{result.languages_failed.map(([lang, err]) => (
									<p key={lang} className="text-red-600 text-xs mt-1">
										{lang}: {err}
									</p>
								))}
							</div>
						)}

						{isRpgMaker && (
							<div className="p-3 bg-violet-50 dark:bg-violet-950/30 border border-violet-200 dark:border-violet-800 rounded text-sm space-y-2">
								<p className="font-medium text-violet-900 dark:text-violet-100">
									{t("inject.registerInUi")}
								</p>
								<p className="text-xs text-violet-800 dark:text-violet-200">
									{t("inject.registerInUiHint")}
									<code className="px-0.5">*.bak-locust</code>
									).
								</p>
								{selectedLangs.length === 1 && (
									<div>
										<label className="text-xs font-medium text-violet-900 dark:text-violet-100">
											{t("inject.menuLabel")}
										</label>
										<input
											type="text"
											value={regLabelOverride}
											onChange={(e) => setRegLabelAndRemember(e.target.value)}
											placeholder={defaultLabelFor(selectedLangs[0])}
											className="w-full mt-0.5 p-1.5 text-sm border border-violet-200 dark:border-violet-700 rounded dark:bg-gray-900"
										/>
										<p className="text-[11px] text-violet-700 dark:text-violet-300 mt-0.5">
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
									className="w-full py-1.5 text-sm bg-violet-600 hover:bg-violet-700 disabled:opacity-50 text-white rounded font-medium"
								>
									{regLoading
										? t("inject.registering")
										: t("inject.registerUi", {
												langs: selectedLangs.join(", ") || "lang",
											})}
								</button>
								{regReports.length > 0 && (
									<div className="text-xs text-violet-700 dark:text-violet-300 space-y-1">
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
							className="text-xs font-medium text-emerald-700 dark:text-emerald-400 hover:underline"
						>
							{t("inject.manageBackups")}
						</button>
						<button
							onClick={onClose}
							className="w-full py-2 bg-emerald-600 hover:bg-emerald-700 text-white rounded font-medium"
						>
							{t("common.close")}
						</button>
					</div>
				)}
			</div>
		</div>
	);
}
