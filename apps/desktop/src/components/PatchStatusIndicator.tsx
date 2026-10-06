import { useQuery } from "@tanstack/react-query";
import {
	AlertCircle,
	CheckCircle2,
	HelpCircle,
	Loader2,
	Package,
} from "lucide-react";
import { patchStatus, type PatchStatusResult, type AppliedInjection } from "../lib/api";
import { useT, type TranslateFn } from "../lib/i18n";

interface PatchStatusIndicatorProps {
	gamePath?: string;
	onOpenPatch: () => void;
	refreshKey?: number;
}

function injectionLabel(injection: AppliedInjection, t: TranslateFn) {
	return t(injection.mode === "direct" ? "patch.status.direct"
		: injection.mode === "add" ? "patch.status.add" : "patch.status.modified");
}

function statusPresentation(
	data: PatchStatusResult,
	t: TranslateFn,
) {
	const injections = data.injections ?? [];
	if (data.status !== "interrupted" && data.status !== "unknown") {
		if (data.injection_pending) {
			return {
				label: t("patch.status.injectionPending"),
				className: "bg-amber-100 text-amber-900 dark:bg-amber-900/40 dark:text-amber-100 border-amber-200 dark:border-amber-800",
				Icon: AlertCircle,
			};
		}
		if (injections.length) {
			const labels = [...new Set(injections.map(injection =>
				injectionLabel(injection, t)))];
			if (data.status === "patched") labels.push(t("patch.status.patched"));
			return {
				label: labels.join(" · "),
				className: "bg-emerald-100 text-emerald-800 dark:bg-emerald-900/40 dark:text-emerald-200 border-emerald-200 dark:border-emerald-800",
				Icon: Package,
			};
		}
	}
	switch (data.status) {
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

	const { label, className, Icon } = statusPresentation(data, t);
	const details = (data.injections ?? []).map(injection => [
		injectionLabel(injection, t),
		injection.language ?? t("patch.status.languageUnavailable"),
		injection.applied_at,
	].filter(Boolean).join(" · "));
	if (data.patch_id) {
		details.push([`${data.patch_id}@${data.patch_version ?? "?"}`, data.language, data.applied_at].filter(Boolean).join(" · "));
	}
	if (data.injection_pending) details.push(t("patch.status.injectionPending"));
	const title = [t("patch.status.openTitle", { label }), ...details].join("\n");

	return (
		<button
			type="button"
			onClick={onOpenPatch}
			className={`inline-flex items-center gap-1 px-2 py-0.5 rounded-full border text-[11px] font-medium transition-colors hover:brightness-95 dark:hover:brightness-110 focus:outline-none focus-visible:ring-2 focus-visible:ring-emerald-500 ${className}`}
			title={title}
		>
			<Icon size={12} />
			{label}
		</button>
	);
}
