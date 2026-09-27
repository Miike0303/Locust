import { useQuery } from "@tanstack/react-query";
import {
	AlertCircle,
	CheckCircle2,
	HelpCircle,
	Loader2,
	Package,
} from "lucide-react";
import { patchStatus, type PatchStatusResult } from "../lib/api";
import { useT, type TranslateFn } from "../lib/i18n";

interface PatchStatusIndicatorProps {
	gamePath?: string;
	onOpenPatch: () => void;
	refreshKey?: number;
}

function statusPresentation(
	status: PatchStatusResult["status"],
	t: TranslateFn,
) {
	switch (status) {
		case "not_patched":
			return {
				label: t("patch.status.pristine"),
				className:
					"bg-gray-100 text-gray-700 dark:bg-gray-800 dark:text-gray-300 border-gray-200 dark:border-gray-700",
				Icon: CheckCircle2,
			};
		case "patched":
			return {
				label: t("patch.status.patched"),
				className:
					"bg-emerald-100 text-emerald-800 dark:bg-emerald-900/40 dark:text-emerald-200 border-emerald-200 dark:border-emerald-800",
				Icon: Package,
			};
		case "interrupted":
			return {
				label: t("patch.status.interrupted"),
				className:
					"bg-amber-100 text-amber-900 dark:bg-amber-900/40 dark:text-amber-100 border-amber-200 dark:border-amber-800",
				Icon: AlertCircle,
			};
		default:
			return {
				label: t("patch.status.unknown"),
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
	const t = useT();
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
				className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full border text-[11px] font-medium transition-colors hover:brightness-95 dark:hover:brightness-110 focus:outline-none focus-visible:ring-2 focus-visible:ring-emerald-500 bg-gray-50 text-gray-600 dark:bg-gray-800 dark:text-gray-300 border-gray-200 dark:border-gray-700"
				title={t("patch.status.checking")}
			>
				<Loader2 size={12} className="animate-spin" />
				{t("patch.status.label")}
			</button>
		);
	}

	if (isError || !data) {
		return (
			<button
				type="button"
				onClick={onOpenPatch}
				className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full border text-[11px] font-medium transition-colors hover:brightness-95 dark:hover:brightness-110 focus:outline-none focus-visible:ring-2 focus-visible:ring-emerald-500 bg-amber-50 text-amber-900 dark:bg-amber-950/40 dark:text-amber-100 border-amber-200 dark:border-amber-900"
				title={t("patch.status.unknownTitle")}
			>
				<HelpCircle size={12} />
				{t("patch.status.unknown")}
			</button>
		);
	}

	const { label, className, Icon } = statusPresentation(data.status, t);
	const detail =
		data.status === "patched" && data.patch_id
			? `${data.patch_id}@${data.patch_version ?? "?"}`
			: label;

	return (
		<button
			type="button"
			onClick={onOpenPatch}
			className={`inline-flex items-center gap-1 px-2 py-0.5 rounded-full border text-[11px] font-medium transition-colors hover:brightness-95 dark:hover:brightness-110 focus:outline-none focus-visible:ring-2 focus-visible:ring-emerald-500 ${className}`}
			title={t("patch.status.openTitle", { label })}
		>
			<Icon size={12} />
			{detail}
		</button>
	);
}
