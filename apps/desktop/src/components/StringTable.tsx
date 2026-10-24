import { pathBasename } from "../lib/path";
import { useState, useRef, useMemo, useEffect, type Ref } from "react";
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
import { ChevronUp, ChevronDown, ChevronsUpDown } from "lucide-react";
import type { StringEntry } from "../lib/api";
import { getConfig, getString, patchString } from "../lib/api";
import { clampTableRowHeight, showSourceColumnEnabled } from "../lib/appearance";
import { useEditorStore } from "../stores/editorStore";
import { useT } from "../lib/i18n";
import { useProjectStore } from "../stores/projectStore";
import { acknowledgeDraft, draftEntryKey, draftProjectKey, editDraft, loadLatestDraft, saveDraft, useDraftStore } from "../stores/draftStore";

import { StatusBadge } from "../lib/statusStyles";

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

	const handleBlur = async (overwrite = false) => {
		if (cancelled.current && !overwrite) return;
		setEditing(false);
		if (!isCurrentProject()) return;
		await saveDraft(draftKey, entry.translation || "", async (text, expectedTranslation) => {
			if (!isCurrentProject()) throw new Error(t("detail.projectChanged"));
			await patchString(entry.id, { translation: text, ...(expectedTranslation === undefined ? {} : { expected_translation: expectedTranslation }) });
			if (isCurrentProject()) onSave();
		}, overwrite);
	};

	const handleLoadLatest = () => loadLatestDraft(draftKey, async () => {
		if (!isCurrentProject()) throw new Error(t("detail.projectChanged"));
		const latest = await getString(entry.id);
		if (!isCurrentProject()) throw new Error(t("detail.projectChanged"));
		onSave();
		return latest.translation;
	});

	if (editing) {
		return (
			<textarea
				aria-label={t("table.editTranslation")}
				value={value}
				disabled={saving}
				onChange={(e) => editDraft(draftKey, e.target.value, entry.translation)}
				onBlur={() => handleBlur()}
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
			tabIndex={-1}
			onClick={(e) => {
				e.stopPropagation();
				cancelled.current = false;
				initialValue.current = value;
				if (!useDraftStore.getState().drafts[draftKey]) editDraft(draftKey, value, entry.translation);
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
		{draft?.conflict && <div className="text-caption text-danger break-words">
			<p role="alert">{t("api.error.translationConflict")}</p>
			<div className="flex gap-3">
				<button type="button" disabled={saving} onClick={handleLoadLatest} className="underline">{t("detail.loadLatest")}</button>
				<button type="button" disabled={saving} onClick={() => handleBlur(true)} className="underline">{t("detail.overwrite")}</button>
			</div>
		</div>}
		{draft?.error && <div className="text-caption text-danger break-words"><p role="alert">{draft.error}</p>{!draft.conflict && <button type="button" disabled={saving} onClick={() => void handleBlur()} className="underline">{t("common.retry")}</button>}</div>}
		{persistenceIssue && <p role="alert" className="text-caption text-danger">{t("detail.draftStorageFailed")}</p>}
		</div>
	);
}

interface StringTableProps {
	data: StringEntry[];
	onRefetch: () => void;
	hasActiveFilters?: boolean;
	tableRef?: Ref<HTMLTableElement>;
}

export default function StringTable({
	data,
	onRefetch,
	hasActiveFilters = false,
	tableRef,
}: StringTableProps) {
	const t = useT();
	const { selectedEntryId, setSelected } = useEditorStore();
	const [sorting, setSorting] = useState<SortingState>([]);
	const [focusedEntryId, setFocusedEntryId] = useState<string | null>(null);
	const [bodyFocused, setBodyFocused] = useState(false);
	const bodyRef = useRef<HTMLTableSectionElement>(null);
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
					return <StatusBadge status={status} />;
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
		getRowId: entry => entry.id,
		state: { sorting, columnVisibility },
		onSortingChange: setSorting,
		getCoreRowModel: getCoreRowModel(),
		getSortedRowModel: getSortedRowModel(),
	});

	const rows = table.getRowModel().rows;
	const focusedRowId = rows.find(row => row.original.id === focusedEntryId)?.original.id ?? rows[0]?.original.id;

	return (
		<div className="overflow-auto flex-1 bg-surface">
			<table ref={tableRef} role="grid" tabIndex={-1} className="w-full text-left">
				<caption className="sr-only">{t("table.keyboardHelp")}</caption>
				<thead className="sticky top-0 z-10 bg-surface/95 backdrop-blur border-b border-border">
					{table.getHeaderGroups().map((hg) => (
						<tr key={hg.id}>
							{hg.headers.map((header) => (
								<th
									key={header.id}
									scope="col"
									aria-sort={header.column.getIsSorted() === "asc" ? "ascending" : header.column.getIsSorted() === "desc" ? "descending" : "none"}
									className="px-3 py-2 text-caption font-medium text-text-muted select-none"
									style={{ width: header.getSize() }}
								>
									<button type="button"
										onClick={header.column.getToggleSortingHandler()}
										title={t("table.sortPageBy", { column: String(header.column.columnDef.header) })}
										className="inline-flex items-center gap-1 rounded-sm hover:text-text focus:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg"
									>
										<span aria-hidden="true">{flexRender(header.column.columnDef.header, header.getContext())}</span>
										<span className="sr-only">{t("table.sortPageBy", { column: String(header.column.columnDef.header) })}</span>
										{header.column.getIsSorted() === "asc" ? <ChevronUp size={12} className="text-text-muted" aria-hidden="true" />
											: header.column.getIsSorted() === "desc" ? <ChevronDown size={12} className="text-text-muted" aria-hidden="true" />
											: <ChevronsUpDown size={12} className="text-text-muted" aria-hidden="true" />}
									</button>
								</th>
							))}
						</tr>
					))}
				</thead>
				<tbody ref={bodyRef} onFocus={() => setBodyFocused(true)}
					onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget)) setBodyFocused(false); }}>
					{rows.map((row, index) => (
						<tr
							key={row.id}
							tabIndex={focusedRowId === row.original.id ? 0 : -1}
							aria-selected={focusedRowId === row.original.id}
							onFocus={() => setFocusedEntryId(row.original.id)}
							onClick={event => {
								event.currentTarget.focus();
								setFocusedEntryId(row.original.id);
								setSelected(row.original.id);
							}}
							onKeyDown={event => {
								// Let nested controls and modified action hotkeys keep their own behavior.
								if (event.target !== event.currentTarget || event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return;
								if (event.key === "ArrowDown" || event.key === "ArrowUp") {
									event.preventDefault();
									const next = Math.max(0, Math.min(rows.length - 1, index + (event.key === "ArrowDown" ? 1 : -1)));
									setFocusedEntryId(rows[next].original.id);
									bodyRef.current?.rows[next]?.focus();
								} else if (event.key === "Enter") {
									event.preventDefault();
									setSelected(row.original.id);
								}
							}}
							style={{ height: rowHeight }}
							className={clsx(
								"group border-b border-border cursor-pointer transition-colors focus:outline-none focus:ring-1 focus:ring-accent-fg focus:bg-surface-muted",
								bodyFocused && focusedRowId === row.original.id && "ring-1 ring-accent-fg bg-surface-muted",
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
