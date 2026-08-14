import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { X, GitBranch, Loader2, FolderOpen } from "lucide-react";
import { openProjectDb, runPivot, type PivotResult } from "../lib/api";
import {
	defaultPivotFileName,
	errorMessage,
	isExistingOutputError,
	pivotOpenDbArgs,
	projectInfoAfterPivotOpen,
} from "../lib/pivot";
import { dropProjectQueries } from "../lib/openProjectFlow";
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
import clsx from "clsx";

const IS_TAURI = "__TAURI_INTERNALS__" in window;

interface PivotModalProps {
	open: boolean;
	onClose: () => void;
	carryOverCount: number | null;
}

export default function PivotModal({
	open,
	onClose,
	carryOverCount,
}: PivotModalProps) {
	const t = useT();
	const queryClient = useQueryClient();
	const { project, setProject } = useProjectStore();
	const [loading, setLoading] = useState(false);
	const [opening, setOpening] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [result, setResult] = useState<PivotResult | null>(null);
	const { dialogRef, dialogProps, titleProps } = useModalA11y({
		open,
		ownEscape: false,
	});

	useEffect(() => {
		if (!open) return;
		setLoading(false);
		setOpening(false);
		setError(null);
		setResult(null);
	}, [open]);

	if (!open || !project) return null;

	const pickAndCreate = async () => {
		if (carryOverCount === 0) {
			addToast("info", t("pivot.toast.noTranslations"));
			return;
		}
		setLoading(true);
		setError(null);
		try {
			let path: string | undefined;
			const defaultName = defaultPivotFileName(project.name);
			if (IS_TAURI) {
				const { save } = await import("@tauri-apps/plugin-dialog");
				const selected = await save({
					title: t("pivot.dialog.save"),
					defaultPath: defaultName,
					filters: [
						{ name: t("pivot.filter.db"), extensions: ["db"] },
					],
				});
				if (typeof selected !== "string" || !selected) {
					setLoading(false);
					return;
				}
				path = selected;
			} else {
				const typed = prompt(t("pivot.prompt.outputPath"), defaultName);
				if (!typed) {
					setLoading(false);
					return;
				}
				path = typed;
			}

			const created = await runPivot(path);
			setResult(created);
			addLog(
				"info",
				`Pivot: ${created.entries} entries → ${created.database_path}`,
				undefined,
				"project",
			);
			addToast(
				"success",
				t("pivot.toast.created", {
					count: created.entries,
					path: created.database_path,
				}),
			);
		} catch (err: unknown) {
			const msg = errorMessage(err);
			const display = isExistingOutputError(msg)
				? t("pivot.toast.exists")
				: t("pivot.toast.failed", { error: msg });
			setError(display);
			addToast("error", display);
			addLog("error", "Pivot failed", msg, "project");
		} finally {
			setLoading(false);
		}
	};

	const stayHere = () => {
		onClose();
	};

	const openCreated = async () => {
		if (!result || opening) return;
		const sourceProject = project;
		setOpening(true);
		setError(null);
		try {
			const args = pivotOpenDbArgs(result.database_path, sourceProject);
			const opened = await openProjectDb(
				args.databasePath,
				args.gamePath,
				args.formatId,
			);
			setProject(projectInfoAfterPivotOpen(sourceProject, opened));
			dropProjectQueries(queryClient);
			addToast("success", t("pivot.toast.opened", { path: result.database_path }));
			addLog(
				"info",
				`Opened pivoted project: ${result.database_path}`,
				undefined,
				"project",
			);
			onClose();
		} catch (err: unknown) {
			const msg = errorMessage(err);
			const display = t("pivot.toast.openFailed", { error: msg });
			setError(display);
			addToast("error", display);
			addLog(
				"error",
				"Open pivoted project failed; current project unchanged",
				msg,
				"project",
			);
		} finally {
			setOpening(false);
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
						<GitBranch size={18} />
						{t("pivot.title")}
					</h2>
					<button
						onClick={onClose}
						className="text-gray-400 hover:text-gray-600"
					>
						<X size={20} />
					</button>
				</div>

				{result ? (
					<div className="space-y-4">
						<p className="text-sm text-gray-700 dark:text-gray-300">
							{t("pivot.success", {
								count: result.entries,
								path: result.database_path,
							})}
						</p>
						<p className="text-xs text-gray-500 break-all">
							{result.database_path}
						</p>
						{error && (
							<p className="text-sm text-red-600 dark:text-red-400">{error}</p>
						)}
						<div className={clsx(MODAL_FOOTER_CLASS, "-mx-6 -mb-6")}>
							<button
								type="button"
								onClick={stayHere}
								disabled={opening}
								className="px-3 py-2 text-sm rounded bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700"
							>
								{t("pivot.stay")}
							</button>
							<button
								type="button"
								onClick={() => {
									void openCreated();
								}}
								disabled={opening}
								className="flex items-center gap-1.5 px-4 py-2 text-sm font-medium rounded text-white bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50"
							>
								{opening ? (
									<Loader2 size={16} className="animate-spin" />
								) : (
									<FolderOpen size={16} />
								)}
								{t("pivot.openNow")}
							</button>
						</div>
					</div>
				) : (
					<div className="space-y-4">
						<p className="text-sm text-gray-700 dark:text-gray-300">
							{t("pivot.explain")}
						</p>
						<p className="text-sm text-gray-600 dark:text-gray-400">
							{t("pivot.skipNote")}
						</p>
						{carryOverCount !== null && (
							<p className="text-sm font-medium">
								{t("pivot.carry", { count: carryOverCount })}
							</p>
						)}
						{error && (
							<p className="text-sm text-red-600 dark:text-red-400">{error}</p>
						)}
						<div className={clsx(MODAL_FOOTER_CLASS, "-mx-6 -mb-6")}>
							<button
								type="button"
								onClick={onClose}
								className="px-3 py-2 text-sm rounded bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700"
							>
								{t("common.cancel")}
							</button>
							<button
								type="button"
								onClick={() => {
									void pickAndCreate();
								}}
								disabled={loading || carryOverCount === 0}
								className="flex items-center gap-1.5 px-4 py-2 text-sm font-medium rounded text-white bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50"
							>
								{loading ? (
									<Loader2 size={16} className="animate-spin" />
								) : (
									<GitBranch size={16} />
								)}
								{loading ? t("pivot.creating") : t("pivot.create")}
							</button>
						</div>
					</div>
				)}
			</div>
		</div>
	);
}
