import { useState, useEffect, type KeyboardEvent } from "react";
import { useQuery } from "@tanstack/react-query";
import { Search, X, ChevronLeft, ChevronRight } from "lucide-react";
import clsx from "clsx";
import { useEditorStore } from "../stores/editorStore";
import { useProjectStore } from "../stores/projectStore";
import { getStringFacets, type StringStatus } from "../lib/api";
import {
	facetOptions,
	filePathFilterPatch,
	filePathOptionLabel,
	tagFilterPatch,
} from "../lib/stringFilterFacets";
import { useT } from "../lib/i18n";

import type { MessageKey } from "../lib/i18n";

const STATUSES: { labelKey: MessageKey; value: StringStatus | undefined }[] = [
	{ labelKey: "filter.all", value: undefined },
	{ labelKey: "filter.pending", value: "pending" },
	{ labelKey: "filter.translated", value: "translated" },
	{ labelKey: "filter.reviewed", value: "reviewed" },
	{ labelKey: "filter.approved", value: "approved" },
	{ labelKey: "filter.error", value: "error" },
];

const statusColors: Record<string, string> = {
	pending: "bg-border text-text",
	translated: "bg-blue-100 text-blue-700 dark:bg-blue-900 dark:text-blue-300",
	reviewed: "bg-warning-muted text-warning",
	approved: "bg-success-muted text-success",
	error: "bg-danger-muted text-danger",
};

interface FilterBarProps {
	total: number;
	showing: number;
}

export default function FilterBar({ total, showing }: FilterBarProps) {
	const t = useT();
	const projectPath = useProjectStore((s) => s.project?.path);
	const { filter, setFilter } = useEditorStore();
	const [searchInput, setSearchInput] = useState(filter.search || "");
	const [filePathDraft, setFilePathDraft] = useState(filter.file_path || "");
	const [tagDraft, setTagDraft] = useState(filter.tag || "");
	const { data: facets } = useQuery({
		queryKey: ["string-facets", projectPath],
		queryFn: getStringFacets,
		enabled: !!projectPath,
		staleTime: Infinity,
		retry: false,
	});
	const filePaths = facetOptions(facets?.file_paths);
	const tags = facetOptions(facets?.tags);

	useEffect(() => {
		const timer = setTimeout(() => {
			setFilter({ search: searchInput || undefined, offset: 0 });
		}, 300);
		return () => clearTimeout(timer);
	}, [searchInput, setFilter]);

	useEffect(() => setFilePathDraft(filter.file_path || ""), [filter.file_path]);
	useEffect(() => setTagDraft(filter.tag || ""), [filter.tag]);

	const commitFilePath = () => {
		const patch = filePathFilterPatch(filePathDraft);
		setFilePathDraft(patch.file_path || "");
		setFilter(patch);
	};
	const commitTag = () => {
		const patch = tagFilterPatch(tagDraft);
		setTagDraft(patch.tag || "");
		setFilter(patch);
	};
	const handleFacetKeyDown = (
		event: KeyboardEvent<HTMLInputElement>,
		committed: string,
		restore: (value: string) => void,
		commit: () => void,
	) => {
		if (event.key === "Enter") commit();
		else if (event.key === "Escape") {
			event.preventDefault();
			restore(committed);
		}
	};

	const hasFilters =
		filter.status || filter.search || filter.file_path || filter.tag;

	return (
		<div className="flex flex-wrap items-center gap-2 p-2 border-b border-border bg-surface">
			<div className="flex flex-wrap gap-0.5">
				{STATUSES.map(({ labelKey, value }) => (
					<button
						key={labelKey}
						onClick={() => setFilter({ status: value, offset: 0 })}
						className={clsx(
							"px-2.5 py-0.5 rounded-full text-caption font-medium transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg",
							filter.status === value
								? value
									? statusColors[value]
									: "bg-accent-muted text-accent-fg"
								: "text-text-muted hover:bg-surface-muted hover:text-text",
						)}
					>
						{t(labelKey)}
					</button>
				))}
			</div>

			<div className="flex-1 relative min-w-40 max-w-sm">
				<Search
					size={16}
					className="absolute left-3 top-1/2 -translate-y-1/2 text-text-muted"
				/>
				<input
					data-search-input
					type="text"
					value={searchInput}
					onChange={(e) => setSearchInput(e.target.value)}
					placeholder={t("filter.searchPlaceholder")}
					className="w-full pl-9 pr-3 py-1 text-body border border-border rounded-md bg-surface text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg"
				/>
			</div>

			<div className="flex items-center gap-1">
				<label
					htmlFor="file-path-filter"
					className="text-caption text-text-muted"
				>
					{t("filter.file")}
				</label>
				<input
					id="file-path-filter"
					list="file-path-filter-options"
					value={filePathDraft}
					onChange={(event) => {
						const value = event.target.value;
						setFilePathDraft(value);
						if (filePaths.includes(value))
							setFilter(filePathFilterPatch(value));
					}}
					onBlur={commitFilePath}
					onKeyDown={(event) =>
						handleFacetKeyDown(
							event,
							filter.file_path || "",
							setFilePathDraft,
							commitFilePath,
						)
					}
					placeholder={t("filter.anyFile")}
					className="w-32 rounded border border-border bg-surface px-2 py-0.5 text-body text-text"
				/>
				<datalist id="file-path-filter-options">
					{filePaths.map((path) => (
						<option key={path} value={path} label={filePathOptionLabel(path)} />
					))}
				</datalist>
			</div>

			<div className="flex items-center gap-1">
				<label
					htmlFor="tag-filter"
					className="text-caption text-text-muted"
				>
					{t("filter.tag")}
				</label>
				<input
					id="tag-filter"
					list="tag-filter-options"
					value={tagDraft}
					onChange={(event) => {
						const value = event.target.value;
						setTagDraft(value);
						if (tags.includes(value)) setFilter(tagFilterPatch(value));
					}}
					onBlur={commitTag}
					onKeyDown={(event) =>
						handleFacetKeyDown(event, filter.tag || "", setTagDraft, commitTag)
					}
					placeholder={t("filter.anyTag")}
					className="w-24 rounded border border-border bg-surface px-2 py-0.5 text-body text-text"
				/>
				<datalist id="tag-filter-options">
					{tags.map((tag) => (
						<option key={tag} value={tag} />
					))}
				</datalist>
			</div>

			{hasFilters && (
				<button
					onClick={() => {
						setFilter({
							status: undefined,
							search: undefined,
							file_path: undefined,
							tag: undefined,
							offset: 0,
						});
						setSearchInput("");
					}}
					className="flex items-center gap-1 px-2 py-0.5 text-caption text-text-muted hover:text-text"
				>
					<X size={14} /> {t("common.clear")}
				</button>
			)}

			<div className="flex items-center gap-2 ml-auto">
				<span className="text-caption text-text-muted">
					{total > 0
						? t("filter.results", {
								from: (filter.offset ?? 0) + 1,
								to: Math.min((filter.offset ?? 0) + showing, total),
								total,
							})
						: t("filter.zeroResults")}
				</span>
				{total > (filter.limit ?? 100) && (
					<div className="flex items-center gap-0.5">
						<button
							onClick={() =>
								setFilter({
									offset: Math.max(
										0,
										(filter.offset ?? 0) - (filter.limit ?? 100),
									),
								})
							}
							disabled={(filter.offset ?? 0) === 0}
							className="p-1 text-text-muted hover:text-text disabled:opacity-30 focus:outline-none focus:ring-2 focus:ring-accent-fg rounded"
						>
							<ChevronLeft size={14} />
						</button>
						<button
							onClick={() =>
								setFilter({
									offset: (filter.offset ?? 0) + (filter.limit ?? 100),
								})
							}
							disabled={(filter.offset ?? 0) + (filter.limit ?? 100) >= total}
							className="p-1 text-text-muted hover:text-text disabled:opacity-30 focus:outline-none focus:ring-2 focus:ring-accent-fg rounded"
						>
							<ChevronRight size={14} />
						</button>
					</div>
				)}
			</div>
		</div>
	);
}
