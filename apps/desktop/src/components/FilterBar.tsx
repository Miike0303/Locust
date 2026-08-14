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

const STATUSES: { labelKey: string; value: StringStatus | undefined }[] = [
	{ labelKey: "filter.all", value: undefined },
	{ labelKey: "filter.pending", value: "pending" },
	{ labelKey: "filter.translated", value: "translated" },
	{ labelKey: "filter.reviewed", value: "reviewed" },
	{ labelKey: "filter.approved", value: "approved" },
	{ labelKey: "filter.error", value: "error" },
];

const statusColors: Record<string, string> = {
	pending: "bg-gray-200 text-gray-700 dark:bg-gray-700 dark:text-gray-300",
	translated: "bg-blue-100 text-blue-700 dark:bg-blue-900 dark:text-blue-300",
	reviewed: "bg-amber-100 text-amber-700 dark:bg-amber-900 dark:text-amber-300",
	approved: "bg-green-100 text-green-700 dark:bg-green-900 dark:text-green-300",
	error: "bg-red-100 text-red-700 dark:bg-red-900 dark:text-red-300",
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
		<div className="flex items-center gap-2 p-2 border-b border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900">
			<div className="flex gap-0.5">
				{STATUSES.map(({ labelKey, value }) => (
					<button
						key={labelKey}
						onClick={() => setFilter({ status: value, offset: 0 })}
						className={clsx(
							"px-2.5 py-0.5 rounded-full text-[11px] font-medium transition-colors",
							filter.status === value
								? value
									? statusColors[value]
									: "bg-emerald-100 text-emerald-700 dark:bg-emerald-900 dark:text-emerald-300"
								: "bg-gray-100 text-gray-600 hover:bg-gray-200 dark:bg-gray-800 dark:text-gray-300 dark:hover:bg-gray-700",
						)}
					>
						{t(labelKey)}
					</button>
				))}
			</div>

			<div className="flex-1 relative max-w-md">
				<Search
					size={16}
					className="absolute left-3 top-1/2 -translate-y-1/2 text-gray-400"
				/>
				<input
					data-search-input
					type="text"
					value={searchInput}
					onChange={(e) => setSearchInput(e.target.value)}
					placeholder={t("filter.searchPlaceholder")}
					className="w-full pl-9 pr-3 py-1 text-xs border border-gray-300 dark:border-gray-600 rounded-md bg-white dark:bg-gray-800 text-gray-900 dark:text-gray-100 placeholder:text-gray-400 dark:placeholder:text-gray-500 focus:outline-none focus:ring-2 focus:ring-emerald-500"
				/>
			</div>

			<div className="flex items-center gap-1">
				<label
					htmlFor="file-path-filter"
					className="text-[11px] text-gray-500 dark:text-gray-400"
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
					className="w-32 rounded border border-gray-300 bg-white px-2 py-0.5 text-[11px] text-gray-900 dark:border-gray-600 dark:bg-gray-800 dark:text-gray-100"
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
					className="text-[11px] text-gray-500 dark:text-gray-400"
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
					className="w-24 rounded border border-gray-300 bg-white px-2 py-0.5 text-[11px] text-gray-900 dark:border-gray-600 dark:bg-gray-800 dark:text-gray-100"
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
					className="flex items-center gap-1 px-2 py-0.5 text-[11px] text-gray-500 hover:text-gray-700 dark:text-gray-400 dark:hover:text-gray-200"
				>
					<X size={14} /> {t("common.clear")}
				</button>
			)}

			<div className="flex items-center gap-2 ml-auto">
				<span className="text-[11px] text-gray-500 dark:text-gray-400">
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
							className="p-1 text-gray-400 hover:text-gray-600 dark:text-gray-500 dark:hover:text-gray-300 disabled:opacity-30 focus:outline-none focus:ring-2 focus:ring-emerald-500 rounded"
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
							className="p-1 text-gray-400 hover:text-gray-600 dark:text-gray-500 dark:hover:text-gray-300 disabled:opacity-30 focus:outline-none focus:ring-2 focus:ring-emerald-500 rounded"
						>
							<ChevronRight size={14} />
						</button>
					</div>
				)}
			</div>
		</div>
	);
}
