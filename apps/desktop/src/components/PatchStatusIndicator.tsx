import { useQuery } from "@tanstack/react-query";
import {
	AlertCircle,
	CheckCircle2,
	HelpCircle,
	Loader2,
	Package,
} from "lucide-react";
import { patchStatus, type PatchStatusResult } from "../lib/api";

interface PatchStatusIndicatorProps {
	gamePath?: string;
	onOpenPatch: () => void;
	refreshKey?: number;
}

function statusPresentation(status: PatchStatusResult["status"]) {
	switch (status) {
		case "not_patched":
			return {
				label: "Pristine",
				className:
					"bg-gray-100 text-gray-700 dark:bg-gray-800 dark:text-gray-300 border-gray-200 dark:border-gray-700",
				Icon: CheckCircle2,
			};
		case "patched":
			return {
				label: "Patch applied",
				className:
					"bg-emerald-100 text-emerald-800 dark:bg-emerald-900/40 dark:text-emerald-200 border-emerald-200 dark:border-emerald-800",
				Icon: Package,
			};
		case "interrupted":
			return {
				label: "Patch interrupted",
				className:
					"bg-amber-100 text-amber-900 dark:bg-amber-900/40 dark:text-amber-100 border-amber-200 dark:border-amber-800",
				Icon: AlertCircle,
			};
		default:
			return {
				label: "Patch unknown",
				className:
					"bg-amber-50 text-amber-800 dark:bg-amber-950/40 dark:text-amber-100 border-amber-200 dark:border-amber-900",
				Icon: HelpCircle,
			};
	}
}

export default function PatchStatusIndicator({
	gamePath,
	onOpenPatch,
	refreshKey = 0,
}: PatchStatusIndicatorProps) {
	const trimmedPath = gamePath?.trim() ?? "";

	const { data, isLoading, isError } = useQuery({
		queryKey: ["patchStatus", trimmedPath, refreshKey],
		queryFn: () => patchStatus({ game_path: trimmedPath }),
		enabled: Boolean(trimmedPath),
		staleTime: 30_000,
		retry: false,
	});

	if (!trimmedPath) return null;

	if (isLoading) {
		return (
			<button
				type="button"
				onClick={onOpenPatch}
				className="flex items-center gap-1.5 px-2.5 py-1 rounded-full border text-xs font-medium bg-gray-50 text-gray-600 dark:bg-gray-800 dark:text-gray-300 border-gray-200 dark:border-gray-700"
				title="Checking patch status"
			>
				<Loader2 size={14} className="animate-spin" />
				Patch status
			</button>
		);
	}

	if (isError || !data) {
		return (
			<button
				type="button"
				onClick={onOpenPatch}
				className="flex items-center gap-1.5 px-2.5 py-1 rounded-full border text-xs font-medium bg-amber-50 text-amber-900 dark:bg-amber-950/40 dark:text-amber-100 border-amber-200 dark:border-amber-900"
				title="Could not read patch status — open Patch"
			>
				<HelpCircle size={14} />
				Patch unknown
			</button>
		);
	}

	const { label, className, Icon } = statusPresentation(data.status);
	const detail =
		data.status === "patched" && data.patch_id
			? `${data.patch_id}@${data.patch_version ?? "?"}`
			: label;

	return (
		<button
			type="button"
			onClick={onOpenPatch}
			className={`flex items-center gap-1.5 px-2.5 py-1 rounded-full border text-xs font-medium ${className}`}
			title={`${label}. Open Patch for details and rollback.`}
		>
			<Icon size={14} />
			{detail}
		</button>
	);
}
