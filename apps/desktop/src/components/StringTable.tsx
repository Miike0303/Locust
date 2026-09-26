import { useState, useRef, useMemo, useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import {
	useReactTable,
	getCoreRowModel,
	getSortedRowModel,
	flexRender,
	type ColumnDef,
	type SortingState,
	type VisibilityState,
} from "@tanstack/react-table";
import clsx from "clsx";
import type { StringEntry } from "../lib/api";
import { getConfig, patchString } from "../lib/api";
import { clampTableRowHeight, showSourceColumnEnabled } from "../lib/appearance";
import { useEditorStore } from "../stores/editorStore";
import { useT, type MessageKey } from "../lib/i18n";
import { useProjectStore } from "../stores/projectStore";
import { acknowledgeDraft, draftEntryKey, draftProjectKey, editDraft, saveDraft, useDraftStore } from "../stores/draftStore";

const statusLabel: Record<string, MessageKey> = {
	pending: "detail.status.pending",
	translated: "detail.status.translated",
	reviewed: "detail.status.reviewed",
	approved: "detail.status.approved",
	error: "detail.status.error",
};

const statusBadge: Record<string, string> = {
	pending: "bg-gray-200 text-gray-700 dark:bg-gray-700 dark:text-gray-200",
	translated:
		"bg-blue-100 text-blue-700 dark:bg-blue-900/60 dark:text-blue-200",
	reviewed:
		"bg-amber-100 text-amber-800 dark:bg-amber-900/60 dark:text-amber-200",
	approved:
		"bg-green-100 text-green-800 dark:bg-green-900/60 dark:text-green-200",
	error: "bg-red-100 text-red-700 dark:bg-red-900/60 dark:text-red-200",
};

function InlineEdit({
	entry,
	onSave,
}: {
	entry: StringEntry;
	onSave: () => void;
}) {
	const [editing, setEditing] = useState(false);
	const t = useT();
	const project = useProjectStore(state => state.project);
	const projectKey = project ? draftProjectKey(project) : "";
	const draftKey = draftEntryKey(projectKey, entry.id);
	const draft = useDraftStore(state => state.drafts[draftKey]);
	const persistenceIssue = useDraftStore(state => state.persistenceIssues[draftKey] ?? state.persistenceIssue);
	const value = draft?.text ?? entry.translation ?? "";
	const saving = draft?.saving ?? false;
	const initialValue = useRef(value);
	const cancelled = useRef(false);
	useEffect(() => { acknowledgeDraft(draftKey, entry.translation || ""); }, [draftKey, entry.translation, saving]);
	const isCurrentProject = () => {
		const current = useProjectStore.getState().project;
		return !!current && draftProjectKey(current) === projectKey;
	};

	const handleBlur = async () => {
		if (cancelled.current) return;
		setEditing(false);
		if (!isCurrentProject()) return;
		await saveDraft(draftKey, entry.translation || "", async text => {
			if (!isCurrentProject()) throw new Error(t("detail.projectChanged"));
			await patchString(entry.id, { translation: text });
			if (isCurrentProject()) onSave();
		});
	};

	if (editing) {
		return (
			<textarea
				aria-label={t("table.editTranslation")}
				value={value}
				disabled={saving}
				onChange={(e) => editDraft(draftKey, e.target.value)}
				onBlur={handleBlur}
				onClick={e => e.stopPropagation()}
				onKeyDown={(e) => {
					if (e.key === "Enter" && e.ctrlKey) { e.preventDefault(); void handleBlur(); }
					if (e.key === "Escape") {
						e.preventDefault(); e.stopPropagation();
						cancelled.current = true;
						editDraft(draftKey, initialValue.current);
						acknowledgeDraft(draftKey, entry.translation || "");
						setEditing(false);
					}
				}}
				autoFocus
				className="w-full p-1 text-xs border border-emerald-400 rounded bg-white dark:bg-gray-800 focus:outline-none resize-none"
				rows={2}
			/>
		);
	}

	return (
		<div onClick={e => e.stopPropagation()}>
		<button type="button" disabled={!project || saving}
			aria-label={t("table.editTranslation")}
			onClick={(e) => {
				e.stopPropagation();
				cancelled.current = false;
				initialValue.current = value;
				setEditing(true);
			}}
			className="block w-full text-left cursor-text text-xs truncate disabled:cursor-wait"
		>
			{value || (
				<span className="text-gray-500 dark:text-gray-400 italic">
					{t("table.clickToEdit")}
				</span>
			)}
		</button>
		{draft && <span role="status" className="block text-[11px] text-amber-800 dark:text-amber-200">{saving ? t("table.saving") : t("table.unsaved")}</span>}
		{draft?.error && <div className="text-xs text-red-700 dark:text-red-300 break-words"><p role="alert">{draft.error}</p><button type="button" disabled={saving} onClick={() => void handleBlur()} className="underline">{t("common.retry")}</button></div>}
		{persistenceIssue && <p role="alert" className="text-xs text-red-700 dark:text-red-300">{t("detail.draftStorageFailed")}</p>}
		</div>
	);
}

interface StringTableProps {
	data: StringEntry[];
	onRefetch: () => void;
	hasActiveFilters?: boolean;
}

export default function StringTable({
	data,
	onRefetch,
	hasActiveFilters = false,
}: StringTableProps) {
	const t = useT();
	const { selectedEntryId, setSelected } = useEditorStore();
	const [sorting, setSorting] = useState<SortingState>([]);
	const { data: config } = useQuery({ queryKey: ["config"], queryFn: getConfig });
	const showSource = showSourceColumnEnabled(config?.ui.show_source_column);
	const rowHeight = clampTableRowHeight(config?.ui.table_row_height);
	const columnVisibility = useMemo<VisibilityState>(
		() => ({ source: showSource }),
		[showSource],
	);

	const columns = useMemo<ColumnDef<StringEntry, any>[]>(
		() => [
			{
				accessorKey: "status",
				header: t("table.status"),
				size: 90,
				cell: ({ getValue }) => {
					const status = getValue() as string;
					return (
						<span
							className={clsx(
								"px-1.5 py-0.5 rounded-full text-[11px] font-semibold uppercase tracking-wide",
								statusBadge[status] ||
									"bg-gray-100 dark:bg-gray-700 dark:text-gray-300",
							)}
						>
							{statusLabel[status] ? t(statusLabel[status]) : status}
						</span>
					);
				},
			},
			{
				accessorKey: "source",
				header: t("table.source"),
				size: 300,
				cell: ({ getValue }) => (
					<div
						className="text-xs line-clamp-2 text-gray-800 dark:text-gray-200"
						title={getValue() as string}
					>
						{getValue() as string}
					</div>
				),
			},
			{
				accessorKey: "translation",
				header: t("table.translation"),
				size: 300,
				cell: ({ row }) => (
					<InlineEdit entry={row.original} onSave={onRefetch} />
				),
			},
			{
				accessorKey: "file_path",
				header: t("table.file"),
				size: 140,
				cell: ({ getValue }) => {
					const full = getValue() as string;
					const name = full.split(/[/\\]/).pop() || full;
					return (
						<span
							className="text-[11px] text-gray-500 dark:text-gray-400"
							title={full}
						>
							{name}
						</span>
					);
				},
			},
			{
				accessorKey: "tags",
				header: t("table.tags"),
				size: 110,
				cell: ({ getValue }) => (
					<div className="flex gap-0.5 flex-wrap">
						{(getValue() as string[]).map((t) => (
							<span
								key={t}
								className="px-1 py-0.5 bg-gray-100 dark:bg-gray-700 text-gray-600 dark:text-gray-300 rounded text-[10px]"
							>
								{t}
							</span>
						))}
					</div>
				),
			},
		],
		[onRefetch, t],
	);

	const table = useReactTable({
		data,
		columns,
		state: { sorting, columnVisibility },
		onSortingChange: setSorting,
		getCoreRowModel: getCoreRowModel(),
		getSortedRowModel: getSortedRowModel(),
	});

	return (
		<div className="overflow-auto flex-1">
			<table className="w-full text-left">
				<thead className="sticky top-0 z-10 bg-gray-50 dark:bg-gray-800 border-b border-gray-200 dark:border-gray-700 shadow-sm">
					{table.getHeaderGroups().map((hg) => (
						<tr key={hg.id}>
							{hg.headers.map((header) => (
								<th
									key={header.id}
									onClick={header.column.getToggleSortingHandler()}
									className="px-2 py-1.5 text-[11px] font-semibold text-gray-500 dark:text-gray-400 uppercase tracking-wide cursor-pointer hover:bg-gray-100 dark:hover:bg-gray-700 select-none"
									style={{ width: header.getSize() }}
								>
									{flexRender(
										header.column.columnDef.header,
										header.getContext(),
									)}
									{{ asc: " ↑", desc: " ↓" }[
										header.column.getIsSorted() as string
									] ?? ""}
								</th>
							))}
						</tr>
					))}
				</thead>
				<tbody>
					{table.getRowModel().rows.map((row) => (
						<tr
							key={row.id}
							onClick={() => setSelected(row.original.id)}
							style={{ height: rowHeight }}
							className={clsx(
								"border-b border-gray-100 dark:border-gray-800/80 cursor-pointer transition-colors",
								selectedEntryId === row.original.id
									? "bg-emerald-50 dark:bg-emerald-950/50 border-l-2 border-l-emerald-500"
									: "hover:bg-gray-50 dark:hover:bg-gray-800/60 border-l-2 border-l-transparent",
							)}
						>
							{row.getVisibleCells().map((cell) => (
								<td
									key={cell.id}
									className="px-2 py-1"
									style={{ maxWidth: cell.column.getSize() }}
								>
									{flexRender(cell.column.columnDef.cell, cell.getContext())}
								</td>
							))}
						</tr>
					))}
				</tbody>
			</table>
			{data.length === 0 && (
				<div className="flex flex-col items-center justify-center h-40 gap-1 text-sm text-gray-500 dark:text-gray-400">
					<p className="font-medium text-gray-600 dark:text-gray-300">
						{hasActiveFilters
							? t("table.noMatch")
							: t("table.noStrings")}
					</p>
					{hasActiveFilters && (
						<p className="text-xs">
							{t("table.clearHint")}
						</p>
					)}
				</div>
			)}
		</div>
	);
}
