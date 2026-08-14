import { useState, useRef } from "react";
import clsx from "clsx";
import { X, Download, Upload, FolderOpen } from "lucide-react";
import {
	exportTranslations,
	importTranslations,
	type ExportFormat,
} from "../lib/api";
import { LANGUAGES } from "../lib/languages";
import { useProjectStore } from "../stores/projectStore";
import { addLog } from "../stores/logStore";
import { addToast } from "../stores/toastStore";
import {
	useModalA11y,
	MODAL_BACKDROP_CLASS,
	MODAL_FOOTER_CLASS,
	modalPanelClass,
} from "../lib/modalA11y";
import { useT } from "../lib/i18n";

const IS_TAURI = "__TAURI_INTERNALS__" in window;

type Mode = "export" | "import";

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
	const fileInputRef = useRef<HTMLInputElement>(null);
	const { dialogRef, dialogProps, titleProps } = useModalA11y({
		open,
		ownEscape: false,
	});

	if (!open || !project) return null;

	const defaultName = `translation_${lang}.${format === "po" ? "po" : "xliff"}`;

	const handleExport = async () => {
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

			const result = await exportTranslations(format, lang, path);
			addToast("success", t("export.toast.exported", { format: format.toUpperCase(), path: result.path }));
			addLog(
				"info",
				`Export ${format} (${lang}): ${result.bytes} bytes`,
				result.path,
				"export",
			);
			onClose();
		} catch (err: unknown) {
			const msg = err instanceof Error ? err.message : String(err);
			addToast("error", t("export.toast.exportFailed", { error: msg }));
			addLog("error", "Export failed", msg, "export");
		} finally {
			setLoading(false);
		}
	};

	const runImportFromPath = async (path: string) => {
		setLoading(true);
		try {
			const result = await importTranslations(format, path);
			addToast(
				"success",
				result.skipped
					? t("export.toast.importedSkipped", {
							count: result.imported,
							skipped: result.skipped,
						})
					: t("export.toast.imported", { count: result.imported }),
			);
			addLog(
				"info",
				`Import ${format}: ${result.imported} applied, ${result.skipped} skipped`,
				result.path,
				"import",
			);
			onImported?.();
			onClose();
		} catch (err: unknown) {
			const msg = err instanceof Error ? err.message : String(err);
			addToast("error", t("export.toast.importFailed", { error: msg }));
			addLog("error", "Import failed", msg, "import");
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
			addToast("success", t("export.toast.imported", { count: result.imported }));
			addLog(
				"info",
				`Import ${format}: ${result.imported} applied`,
				file.name,
				"import",
			);
			onImported?.();
			onClose();
		} catch (err: unknown) {
			const msg = err instanceof Error ? err.message : String(err);
			addToast("error", t("export.toast.importFailed", { error: msg }));
			addLog("error", "Import failed", msg, "import");
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
						className="text-lg font-bold flex items-center gap-2"
					>
						{mode === "export" ? <Download size={18} /> : <Upload size={18} />}
						{mode === "export" ? t("export.exportTitle") : t("export.importTitle")}
					</h2>
					<button
						onClick={onClose}
						className="text-gray-400 hover:text-gray-600"
					>
						<X size={20} />
					</button>
				</div>

				<div className="flex gap-1 mb-4 p-1 bg-gray-100 dark:bg-gray-800 rounded">
					<button
						type="button"
						onClick={() => setMode("export")}
						className={`flex-1 py-1.5 text-sm rounded transition-colors ${
							mode === "export"
								? "bg-white dark:bg-gray-700 shadow font-medium"
								: "text-gray-500 hover:text-gray-700"
						}`}
					>
						{t("export.export")}
					</button>
					<button
						type="button"
						onClick={() => setMode("import")}
						className={`flex-1 py-1.5 text-sm rounded transition-colors ${
							mode === "import"
								? "bg-white dark:bg-gray-700 shadow font-medium"
								: "text-gray-500 hover:text-gray-700"
						}`}
					>
						{t("export.import")}
					</button>
				</div>

				<div className="space-y-4">
					<div>
						<label className="text-sm font-medium">{t("export.format")}</label>
						<select
							value={format}
							onChange={(e) => setFormat(e.target.value as ExportFormat)}
							className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
						>
							<option value="po">Gettext PO (.po)</option>
							<option value="xliff">XLIFF 1.2 (.xliff)</option>
						</select>
					</div>

					{mode === "export" && (
						<div>
							<label className="text-sm font-medium">{t("export.targetLang")}</label>
							<select
								value={lang}
								onChange={(e) => setLang(e.target.value)}
								className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
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
						<p className="text-xs text-gray-500">
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

					<div className={clsx(MODAL_FOOTER_CLASS, "-mx-6 -mb-6")}>
						<button
							onClick={onClose}
							className="px-3 py-2 text-sm rounded bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700"
						>
							{t("common.cancel")}
						</button>
						<button
							onClick={() => {
								void (mode === "export" ? handleExport() : handleImport());
							}}
							disabled={loading}
							className="flex items-center gap-1.5 px-4 py-2 text-sm font-medium rounded bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50 text-white"
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
		</div>
	);
}
