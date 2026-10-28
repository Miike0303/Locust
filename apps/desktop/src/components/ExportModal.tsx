import { IS_TAURI } from "../lib/runtime";
import { useState, useRef, useEffect, useId } from "react";
import { useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import { X, Download, Upload, FolderOpen } from "lucide-react";
import {
	exportTranslations,
	importTranslations,
	getStringFacets,
	getFileStats,
	type ExportFormat,
	type ImportResult,
	type StringStatus,
} from "../lib/api";
import { LANGUAGES } from "../lib/languages";
import { useProjectStore } from "../stores/projectStore";
import { addLog } from "../stores/logStore";
import { addToast } from "../stores/toastStore";
import {
	useModalA11y,
	MODAL_BACKDROP_CLASS,
	MODAL_BODY_CLASS,
	MODAL_FOOTER_CLASS,
	modalPanelClass,
} from "../lib/modalA11y";
import { useT } from "../lib/i18n";
import { draftProjectKey, projectScopedQueryFn, useProjectScopedQueryEnabled } from "../lib/projectScope";

type Mode = "export" | "import";
const STATUSES: StringStatus[] = ["pending", "translated", "reviewed", "approved", "error"];

function relativeFilePath(file: string, projectPath: string): string {
	const path = file.replace(/\\/g, "/");
	const root = projectPath.replace(/\\/g, "/").replace(/\/+$/, "") + "/";
	return path.startsWith(root) ? path.slice(root.length) : path;
}

interface ExportModalProps {
	open: boolean;
	onClose: () => void;
	onImported?: () => void;
}

export default function ExportModal({
	open,
	onClose,
	onImported,
}: ExportModalProps) {
	const t = useT();
	const { project } = useProjectStore();
	const [mode, setMode] = useState<Mode>("export");
	const [format, setFormat] = useState<ExportFormat>("po");
	const [lang, setLang] = useState("es");
	const [loading, setLoading] = useState(false);
	const [file, setFile] = useState("");
	const [statuses, setStatuses] = useState<StringStatus[]>(STATUSES);
	const scopeId = useId();
	const projectKey = project ? draftProjectKey(project) : "";
	const scopeEnabled = useProjectScopedQueryEnabled(projectKey, open && !!project && mode === "export");
	const facetsQuery = useQuery({
		queryKey: ["string-facets", projectKey],
		queryFn: projectScopedQueryFn(projectKey, () => getStringFacets()),
		enabled: scopeEnabled,
		staleTime: 0,
		retry: false,
	});
	// The existing aggregate API counts by exact stored file and status in SQL.
	const filesQuery = useQuery({
		queryKey: ["file-stats", projectKey],
		queryFn: projectScopedQueryFn(projectKey, () => getFileStats()),
		enabled: scopeEnabled,
		staleTime: 0,
		retry: false,
	});
	useEffect(() => {
		setFile("");
		setStatuses(STATUSES);
	}, [open, projectKey]);
	const count = filesQuery.data?.filter(row => !file || row.file_path === file)
		.reduce((sum, row) => sum + statuses.reduce((total, status) => total + row[status], 0), 0);
	const scopePending = facetsQuery.isPending || filesQuery.isPending || filesQuery.isFetching;
	const scopeFailed = facetsQuery.isError || filesQuery.isError;
	const exportDisabled = loading || !scopeEnabled() || scopePending || scopeFailed || count === undefined || count === 0;
	const fileInputRef = useRef<HTMLInputElement>(null);
	const { dialogRef, dialogProps, titleProps } = useModalA11y({
		open,
		ownEscape: false,
	});

	if (!open || !project) return null;

	const defaultName = `translation_${lang}.${format === "po" ? "po" : "xliff"}`;

	const handleExport = async () => {
		if (exportDisabled) return;
		setLoading(true);
		try {
			let path: string | undefined;
			if (IS_TAURI) {
				const { save } = await import("@tauri-apps/plugin-dialog");
				const selected = await save({
					title: t("export.dialog.export"),
					defaultPath: defaultName,
					filters: [
						format === "po"
							? { name: t("export.filter.po"), extensions: ["po"] }
							: { name: t("export.filter.xliff"), extensions: ["xliff", "xlf"] },
					],
				});
				if (typeof selected !== "string" || !selected) {
					setLoading(false);
					return;
				}
				path = selected;
			}

			const result = await exportTranslations(format, lang, path, { file: file || undefined, status: statuses });
			addToast("success", t("export.toast.exported", { format: format.toUpperCase(), path: result.path }));
			addLog(
				"info",
				t("activity.export.completed", { format, language: lang, count: result.bytes }),
				result.path,
				"export",
			);
			onClose();
		} catch (err: unknown) {
			const msg = err instanceof Error ? err.message : String(err);
			addToast("error", t("export.toast.exportFailed", { error: msg }));
			addLog("error", t("activity.export.failed"), msg, "export");
		} finally {
			setLoading(false);
		}
	};

	const reportImport = (result: ImportResult, name: string) => {
		const message = result.skipped
			? t("export.toast.importedSkipped", { count: result.imported, skipped: result.skipped })
			: t("export.toast.imported", { count: result.imported });
		const stale = result.stale_sources ? ` ${t("export.toast.staleSources", { count: result.stale_sources })}` : "";
		addToast(result.skipped ? "warning" : "success", message + stale);
		addLog(result.skipped ? "warning" : "info",
			t("activity.import.completed", {
				format,
				applied: t("activity.import.applied", { count: result.imported }),
				skipped: t("activity.import.skipped", { count: result.skipped }),
				outdated: t("activity.import.outdated", { count: result.stale_sources ?? 0 }),
			}), name, "import");
		onImported?.();
		onClose();
	};

	const runImportFromPath = async (path: string) => {
		setLoading(true);
		try {
			const result = await importTranslations(format, path);
			reportImport(result, result.path);
		} catch (err: unknown) {
			const msg = err instanceof Error ? err.message : String(err);
			addToast("error", t("export.toast.importFailed", { error: msg }));
			addLog("error", t("activity.import.failed"), msg, "import");
		} finally {
			setLoading(false);
		}
	};

	const handleImport = async () => {
		if (IS_TAURI) {
			const { open: openDialog } = await import("@tauri-apps/plugin-dialog");
			const selected = await openDialog({
				title: t("export.dialog.import"),
				multiple: false,
				filters: [
					format === "po"
						? { name: t("export.filter.po"), extensions: ["po"] }
						: { name: t("export.filter.xliff"), extensions: ["xliff", "xlf", "xml"] },
				],
			});
			if (typeof selected !== "string" || !selected) return;
			await runImportFromPath(selected);
			return;
		}
		fileInputRef.current?.click();
	};

	const handleBrowserFile = async (file: File | null) => {
		if (!file) return;
		const text = await file.text();
		setLoading(true);
		try {
			const result = await importTranslations(format, text);
			reportImport(result, file.name);
		} catch (err: unknown) {
			const msg = err instanceof Error ? err.message : String(err);
			addToast("error", t("export.toast.importFailed", { error: msg }));
			addLog("error", t("activity.import.failed"), msg, "import");
		} finally {
			setLoading(false);
		}
	};

	return (
		<div className={MODAL_BACKDROP_CLASS}>
			<div
				ref={dialogRef}
				{...dialogProps}
				className={modalPanelClass("max-w-md p-6")}
			>
				<div className="flex justify-between items-center mb-4">
					<h2
						{...titleProps}
						className="text-section font-bold flex items-center gap-2"
					>
						{mode === "export" ? <Download size={18} /> : <Upload size={18} />}
						{mode === "export" ? t("export.exportTitle") : t("export.importTitle")}
					</h2>
					<button
						onClick={onClose}
						aria-label={t("common.close")}
						className="text-text-muted hover:text-text"
					>
						<X size={20} />
					</button>
				</div>

				<div className="flex gap-1 mb-4 p-1 bg-surface-muted rounded">
					<button
						type="button"
						onClick={() => setMode("export")}
						className={`flex-1 py-1.5 text-body rounded transition-colors ${
							mode === "export"
								? "bg-surface text-text shadow font-medium"
								: "text-text-muted hover:text-text"
						}`}
					>
						{t("export.export")}
					</button>
					<button
						type="button"
						onClick={() => setMode("import")}
						className={`flex-1 py-1.5 text-body rounded transition-colors ${
							mode === "import"
								? "bg-surface text-text shadow font-medium"
								: "text-text-muted hover:text-text"
						}`}
					>
						{t("export.import")}
					</button>
				</div>

				<div className={clsx(MODAL_BODY_CLASS, "space-y-4")}>
					<div>
						<label htmlFor={`${scopeId}-format`} className="text-body font-medium">{t("export.format")}</label>
						<select
							id={`${scopeId}-format`}
							value={format}
							onChange={(e) => setFormat(e.target.value as ExportFormat)}
							className="mt-1 w-full p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
						>
							<option value="po">Gettext PO (.po)</option>
							<option value="xliff">XLIFF 1.2 (.xliff)</option>
						</select>
					</div>

					{mode === "export" && (
						<fieldset className="space-y-3" aria-describedby={`${scopeId}-count`} disabled={loading}>
							<legend className="text-body font-medium">{t("export.scope.title")}</legend>
							<div>
								<label htmlFor={`${scopeId}-file`} className="text-body font-medium">{t("export.scope.file")}</label>
								<select
									id={`${scopeId}-file`}
									value={file}
									onChange={event => setFile(event.target.value)}
									className="mt-1 w-full p-2 border border-border rounded bg-surface text-body text-text focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
								>
									<option value="">{t("export.scope.allFiles")}</option>
									{facetsQuery.data?.file_paths.map(path => (
										<option key={path} value={path}>{relativeFilePath(path, project.path)}</option>
									))}
								</select>
							</div>
							<fieldset>
								<legend className="text-body font-medium mb-1">{t("export.scope.status")}</legend>
								<div className="flex flex-wrap gap-x-4 gap-y-2">
									{STATUSES.map(status => (
										<label key={status} className="flex items-center gap-2 text-body">
											<input
												type="checkbox"
												checked={statuses.includes(status)}
												onChange={event => {
													const checked = event.target.checked;
													setStatuses(previous => STATUSES.filter(value => value === status ? checked : previous.includes(value)));
												}}
												className="focus:outline-none focus:ring-2 focus:ring-accent-fg"
											/>
											{t(`filter.${status}`)}
										</label>
									))}
								</div>
							</fieldset>
							<p id={`${scopeId}-count`} role="status" className="text-caption text-text-muted">
								{scopeFailed ? t("export.scope.loadFailed") : scopePending || count === undefined
									? t("export.scope.loading") : t("export.scope.count", { count })}
							</p>
							{!scopeFailed && !scopePending && count === 0 && (
								<p className="text-caption text-text-muted">{t("export.scope.empty")}</p>
							)}
						</fieldset>
					)}

					{mode === "export" && (
						<div>
							<label htmlFor={`${scopeId}-lang`} className="text-body font-medium">{t("export.targetLang")}</label>
							<select
								id={`${scopeId}-lang`}
								value={lang}
								onChange={(e) => setLang(e.target.value)}
								className="mt-1 w-full p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
							>
								{LANGUAGES.map((l) => (
									<option key={l.code} value={l.code}>
										{l.label} ({l.code})
									</option>
								))}
							</select>
						</div>
					)}

					{mode === "import" && (
						<p className="text-caption text-text-muted">
							{t("export.importHint")}
						</p>
					)}

					<input
						ref={fileInputRef}
						type="file"
						accept={
							format === "po"
								? ".po,text/plain"
								: ".xliff,.xlf,.xml,application/xml"
						}
						className="hidden"
						onChange={(e) => {
							void handleBrowserFile(e.target.files?.[0] ?? null);
							e.target.value = "";
						}}
					/>
				</div>

				<div className={clsx(MODAL_FOOTER_CLASS, "mt-4 -mx-6 -mb-6")}>
					<button
						onClick={onClose}
						className="px-3 py-2 text-body rounded border border-border bg-surface text-text hover:bg-surface-muted"
					>
						{t("common.cancel")}
					</button>
					<button
						onClick={() => {
							void (mode === "export" ? handleExport() : handleImport());
						}}
						disabled={mode === "export" ? exportDisabled : loading}
						className="flex items-center gap-1.5 px-4 py-2 text-body font-medium rounded bg-accent hover:bg-accent-hover disabled:opacity-50 text-white"
					>
						{loading ? (
							mode === "export" ? (
								t("export.exporting")
							) : (
								t("export.importing")
							)
						) : mode === "export" ? (
							<>
								<FolderOpen size={16} />
								{IS_TAURI ? t("common.saveAs") : t("export.download")}
							</>
						) : (
							<>
								<Upload size={16} />
								{IS_TAURI ? t("export.chooseFile") : t("export.uploadFile")}
							</>
						)}
					</button>
				</div>
			</div>
		</div>
	);
}
