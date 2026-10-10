import { pathBasename } from "../lib/path";
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

/** Calm status marker: a colored dot plus a sentence-case label (never color alone). */
const statusDot: Record<string, string> = {
	pending: "bg-transparent ring-1 ring-inset ring-text-muted",
	translated: "bg-blue-500 dark:bg-blue-400",
	reviewed: "bg-warning",
	approved: "bg-success",
	error: "bg-danger",
};

const statusText: Record<string, string> = {
	pending: "text-text-muted",
	translated: "text-blue-700 dark:text-blue-300",
	reviewed: "text-warning",
	approved: "text-success",
	error: "text-danger",
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
	const selected = useEditorStore(state => state.selectedEntryId === entry.id);
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
				className="w-full p-1 text-body border border-accent-fg rounded bg-surface text-text focus:outline-none resize-none"
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
			className="block w-full truncate rounded-sm text-left text-body text-text cursor-text disabled:cursor-wait focus:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg"
		>
			{value || (
				<>
					{/* Quiet empty state; the edit hint appears on row hover/focus or selection. */}
					<span
						aria-hidden="true"
						className={clsx(
							"text-text-muted group-hover:hidden group-focus-within:hidden",
							selected && "hidden",
						)}
					>
						—
					</span>
					<span
						className={clsx(
							"italic text-text-muted group-hover:inline group-focus-within:inline",
							selected ? "inline" : "hidden",
						)}
					>
						{t("table.clickToEdit")}
					</span>
				</>
			)}
		</button>
		{draft && <span role="status" className="block text-caption text-warning">{saving ? t("table.saving") : t("table.unsaved")}</span>}
		{draft?.error && <div className="text-caption text-danger break-words"><p role="alert">{draft.error}</p><button type="button" disabled={saving} onClick={() => void handleBlur()} className="underline">{t("common.retry")}</button></div>}
		{persistenceIssue && <p role="alert" className="text-caption text-danger">{t("detail.draftStorageFailed")}</p>}
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
								"inline-flex items-center gap-1.5 whitespace-nowrap text-caption font-medium",
								statusText[status] || "text-text-muted",
							)}
						>
							<span
								aria-hidden="true"
								className={clsx(
									"size-2 shrink-0 rounded-full",
									statusDot[status] || "bg-text-muted",
								)}
							/>
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
						className="text-body line-clamp-2 text-text"
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
					const name = pathBasename(full);
					return (
						<span
							className="text-caption text-text-muted"
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
								className="px-1.5 py-px rounded border border-border text-text-muted text-caption"
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
		<div className="overflow-auto flex-1 bg-surface">
			<table className="w-full text-left">
				<thead className="sticky top-0 z-10 bg-surface/95 backdrop-blur border-b border-border">
					{table.getHeaderGroups().map((hg) => (
						<tr key={hg.id}>
							{hg.headers.map((header) => (
								<th
									key={header.id}
									onClick={header.column.getToggleSortingHandler()}
									className="px-3 py-2 text-caption font-medium text-text-muted cursor-pointer hover:text-text select-none"
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
								"group border-b border-border cursor-pointer transition-colors",
								selectedEntryId === row.original.id
									? "bg-accent-muted shadow-[inset_2px_0_0_0_var(--color-accent)]"
									: "hover:bg-surface-muted",
							)}
						>
							{row.getVisibleCells().map((cell) => (
								<td
									key={cell.id}
									className="px-3 py-1"
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
				<div className="flex flex-col items-center justify-center h-40 gap-1 text-body text-text-muted">
					<p className="font-medium text-text-muted">
						{hasActiveFilters
							? t("table.noMatch")
							: t("table.noStrings")}
					</p>
					{hasActiveFilters && (
						<p className="text-caption">
							{t("table.clearHint")}
						</p>
					)}
				</div>
			)}
		</div>
	);
}
