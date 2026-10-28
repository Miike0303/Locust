import { IS_TAURI } from "../lib/runtime";
import { useState, useRef, useEffect, useId } from "react";
import { useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import { X, Download, Upload, FolderOpen } from "lucide-react";
import {
	exportTranslations,
	previewImportTranslations,
	applyImportPreview,
	getStringFacets,
	getFileStats,
	type ImportFormat,
	type ImportPreview,
	type ImportCounts,
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
import { ApiError } from "../lib/apiError";
import { StatusBadge } from "../lib/statusStyles";

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
	const [format, setFormat] = useState<ImportFormat>("po");
	const [lang, setLang] = useState("es");
	const [loading, setLoading] = useState(false);
	const [file, setFile] = useState("");
	const [statuses, setStatuses] = useState<StringStatus[]>(STATUSES);
	const [importSource, setImportSource] = useState<{ name: string; value: string } | null>(null);
	const [keepExisting, setKeepExisting] = useState(true);
	const [preview, setPreview] = useState<ImportPreview | null>(null);
	const [previewing, setPreviewing] = useState(false);
	const [importError, setImportError] = useState("");
	const [stalePlan, setStalePlan] = useState(false);
	const importGeneration = useRef(0);
	const previewGeneration = useRef(-1);
	const scopeId = useId();
	const projectKey = project ? draftProjectKey(project) : "";
	const importContext = JSON.stringify([open, projectKey, mode, format]);
	const currentImportContext = useRef(importContext);
	currentImportContext.current = importContext;
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
		setKeepExisting(true);
	}, [open, projectKey]);
	useEffect(() => {
		importGeneration.current++;
		setImportSource(null);
		setPreview(null);
		setPreviewing(false);
		setImportError("");
		setStalePlan(false);
	}, [open, projectKey, mode, format]);
	useEffect(() => () => { importGeneration.current++; }, []);
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
		if (exportDisabled || format === "tmx") return;
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

	const reportImport = (result: ImportCounts, name: string) => {
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

	const clearImportPreview = () => {
		importGeneration.current++;
		previewGeneration.current = -1;
		setPreview(null);
		setPreviewing(false);
		setImportError("");
		setStalePlan(false);
	};
	const isCurrentImport = (generation: number) =>
		generation === importGeneration.current && currentImportContext.current === importContext;

	const runPreview = async (source = importSource, keep = keepExisting) => {
		if (!source || loading || currentImportContext.current !== importContext) return;
		clearImportPreview();
		const generation = importGeneration.current;
		setPreviewing(true);
		try {
			const result = await previewImportTranslations(format, source.value, keep);
			if (isCurrentImport(generation)) {
				previewGeneration.current = generation;
				setPreview(result);
			}
		} catch (err: unknown) {
			if (isCurrentImport(generation)) setImportError(err instanceof Error ? err.message : String(err));
		} finally {
			if (isCurrentImport(generation)) setPreviewing(false);
		}
	};

	const handleChooseImport = async () => {
		if (loading) return;
		if (IS_TAURI) {
			const generation = importGeneration.current;
			try {
				const { open: openDialog } = await import("@tauri-apps/plugin-dialog");
				const selected = await openDialog({
					title: t("export.dialog.import"), multiple: false,
					filters: [format === "po"
						? { name: t("export.filter.po"), extensions: ["po"] }
						: format === "tmx" ? { name: "TMX", extensions: ["tmx", "xml"] }
							: { name: t("export.filter.xliff"), extensions: ["xliff", "xlf", "xml"] }],
				});
				if (typeof selected !== "string" || !selected || !isCurrentImport(generation)) return;
				clearImportPreview();
				setImportSource({ name: selected.split(/[/\\]/).pop() || selected, value: selected });
			} catch (err: unknown) {
				if (isCurrentImport(generation)) setImportError(err instanceof Error ? err.message : String(err));
			}
			return;
		}
		fileInputRef.current?.click();
	};

	const handleBrowserFile = async (file: File | null) => {
		if (!file) return;
		clearImportPreview();
		setImportSource(null);
		const generation = importGeneration.current;
		try {
			const bytes = await file.arrayBuffer();
			// Match native UTF-8 reads exactly: keep a BOM and reject malformed bytes.
			const value = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes);
			if (isCurrentImport(generation)) setImportSource({ name: file.name, value });
		} catch {
			if (isCurrentImport(generation)) setImportError(t("export.preview.readFailed"));
		}
	};

	const handleApplyImport = async () => {
		if (!preview || !importSource || previewing || loading
			|| previewGeneration.current !== importGeneration.current || currentImportContext.current !== importContext) return;
		const generation = importGeneration.current;
		previewGeneration.current = -1;
		setLoading(true);
		setImportError("");
		try {
			const result = await applyImportPreview(format, importSource.value, keepExisting, preview.plan_token);
			if (isCurrentImport(generation)) {
				setPreview(null);
				reportImport(result, importSource.name);
			}
		} catch (err: unknown) {
			if (!isCurrentImport(generation)) return;
			setPreview(null);
			const raw = err instanceof ApiError ? err.raw : err instanceof Error ? err.message : String(err);
			addLog("error", t("activity.import.failed"), raw, "import");
			const stale = /stale/i.test(raw);
			setStalePlan(stale);
			setImportError(stale ? t("export.preview.stale") : err instanceof Error ? err.message : String(err));
		} finally {
			setLoading(false);
		}
	};

	return (
		<div className={MODAL_BACKDROP_CLASS}>
			<div
				ref={dialogRef}
				{...dialogProps}
				className={modalPanelClass(mode === "import" ? "max-w-3xl p-6" : "max-w-md p-6")}
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
						onClick={() => { clearImportPreview(); onClose(); }}
						aria-label={t("common.close")}
						className="text-text-muted hover:text-text"
					>
						<X size={20} />
					</button>
				</div>

				<div className="flex gap-1 mb-4 p-1 bg-surface-muted rounded">
					<button
						type="button"
						onClick={() => { clearImportPreview(); if (format === "tmx") setFormat("po"); setMode("export"); }}
						disabled={loading}
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
						onClick={() => { clearImportPreview(); setMode("import"); }}
						disabled={loading}
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
							onChange={(e) => { clearImportPreview(); setImportSource(null); setFormat(e.target.value as ImportFormat); }}
							disabled={loading}
							className="mt-1 w-full p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
						>
							<option value="po">Gettext PO (.po)</option>
							<option value="xliff">XLIFF 1.2 (.xliff)</option>
							{mode === "import" && <option value="tmx">TMX (.tmx)</option>}
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
						<>
							<p className="text-caption text-text-muted">{t("export.importHint")}</p>
							<div className="flex flex-wrap items-center gap-3">
								<button type="button" onClick={() => { void handleChooseImport(); }} disabled={loading}
									className="flex items-center gap-1.5 px-3 py-2 text-body rounded border border-border bg-surface text-text hover:bg-surface-muted focus:outline-none focus:ring-2 focus:ring-accent-fg disabled:opacity-50">
									<FolderOpen size={16} aria-hidden="true" />{t("export.chooseFile")}
								</button>
								<span className="text-body break-all">{importSource?.name ?? t("export.preview.noFile")}</span>
							</div>
							<fieldset className="space-y-2" disabled={loading}>
								<legend className="text-body font-medium mb-2">{t("export.preview.existing")}</legend>
								{[true, false].map(keep => (
									<label key={String(keep)} className="flex items-center gap-2 text-body">
										<input type="radio" name={`${scopeId}-existing`} checked={keepExisting === keep}
											onChange={() => { setKeepExisting(keep); clearImportPreview(); if (importSource) void runPreview(importSource, keep); }}
											className="focus:outline-none focus:ring-2 focus:ring-accent-fg" />
										{t(keep ? "export.preview.keep" : "export.preview.replace")}
									</label>
								))}
							</fieldset>
							<div role="status" aria-live="polite" aria-atomic="true">
								{previewing && <p className="text-body text-text-muted">{t("export.preview.loading")}</p>}
								{preview && <div className="rounded border border-border bg-surface-muted p-3 text-body">
									<p>{t("export.preview.summary", { fill: preview.fill, replace: preview.replace, unchanged: preview.unchanged, unknown: preview.unknown_ids })}
										{keepExisting && ` · ${t("export.preview.kept", { count: preview.kept_existing })}`}</p>
									{preview.stale_sources > 0 && <p className="mt-1 text-caption">{t("export.preview.staleSources", { count: preview.stale_sources })}</p>}
								</div>}
							</div>
							{preview && preview.replacements.length > 0 && <div>
								<div className="max-h-64 overflow-auto rounded border border-border" role="region" aria-label={t("export.preview.replacements")} tabIndex={0}>
									<table className="w-full table-fixed text-left text-body">
										<caption className="sr-only">{t("export.preview.replacements")}</caption>
										<thead className="sticky top-0 bg-surface-muted"><tr>
											<th scope="col" className="w-1/4 p-2 font-medium">{t("export.scope.file")}</th>
											<th scope="col" className="p-2 font-medium">{t("export.preview.previous")}</th>
											<th scope="col" className="p-2 font-medium">{t("export.preview.new")}</th>
										</tr></thead>
										<tbody>{preview.replacements.map(row => <tr key={row.id} className="border-t border-border align-top">
											<td className="p-2 break-words">{relativeFilePath(row.file, project.path)}</td>
											<td className="p-2"><StatusBadge status={row.previous_status} /><p className="mt-1 whitespace-pre-wrap break-words">{row.previous_text}</p></td>
											<td className="p-2 whitespace-pre-wrap break-words">{row.new_text}</td>
										</tr>)}</tbody>
									</table>
								</div>
							</div>}
							{preview?.truncated && <p className="text-caption text-text-muted">{t("export.preview.truncated", { count: preview.replacements.length })}</p>}
							{importError && <div role="alert" className="rounded border border-danger bg-danger-muted p-3 text-body text-danger">
								<p>{importError}</p>
								{stalePlan && <button type="button" onClick={() => { void runPreview(); }} disabled={previewing || loading}
									className="mt-2 rounded underline underline-offset-2 focus:outline-none focus:ring-2 focus:ring-accent-fg disabled:opacity-50">{t("export.preview.again")}</button>}
							</div>}
						</>
					)}

					<input
						ref={fileInputRef}
						type="file"
						accept={
							format === "po"
								? ".po,text/plain"
								: format === "tmx" ? ".tmx,.xml,application/xml" : ".xliff,.xlf,.xml,application/xml"
						}
						className="hidden"
						aria-label={t("export.chooseFile")}
						onChange={(e) => {
							void handleBrowserFile(e.target.files?.[0] ?? null);
							e.target.value = "";
						}}
					/>
				</div>

				<div className={clsx(MODAL_FOOTER_CLASS, "mt-4 -mx-6 -mb-6")}>
					<button
						onClick={() => { clearImportPreview(); onClose(); }}
						className="px-3 py-2 text-body rounded border border-border bg-surface text-text hover:bg-surface-muted"
					>
						{t("common.cancel")}
					</button>
					{mode === "import" && <button type="button" onClick={() => { void runPreview(); }} disabled={!importSource || previewing || loading}
						className={clsx("px-4 py-2 text-body font-medium rounded focus:outline-none focus:ring-2 focus:ring-accent-fg disabled:opacity-50",
							preview ? "border border-border bg-surface text-text hover:bg-surface-muted" : "bg-accent hover:bg-accent-hover text-white")}>
						{previewing ? t("export.preview.loading") : t("export.preview.action")}
					</button>}
					{mode === "import" && <button type="button" onClick={() => { void handleApplyImport(); }} disabled={!preview || previewing || loading}
						className="px-4 py-2 text-body font-medium rounded bg-accent hover:bg-accent-hover disabled:opacity-50 text-white focus:outline-none focus:ring-2 focus:ring-accent-fg">
						{loading ? t("export.importing") : t("export.preview.apply")}
					</button>}
					{mode === "export" && <button
						onClick={() => {
							void handleExport();
						}}
						disabled={exportDisabled}
						className="flex items-center gap-1.5 px-4 py-2 text-body font-medium rounded bg-accent hover:bg-accent-hover disabled:opacity-50 text-white"
					>
						{loading ? t("export.exporting") : (
							<>
								<FolderOpen size={16} />
								{IS_TAURI ? t("common.saveAs") : t("export.download")}
							</>
						)}
					</button>}
				</div>
			</div>
		</div>
	);
}
