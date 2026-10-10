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
				className: "bg-warning-muted text-warning border-warning/40",
				Icon: AlertCircle,
			};
		}
		if (injections.length) {
			const labels = [...new Set(injections.map(injection =>
				injectionLabel(injection, t)))];
			if (data.status === "patched") labels.push(t("patch.status.patched"));
			return {
				label: labels.join(" · "),
				className: "bg-success-muted text-success border-success/40",
				Icon: Package,
			};
		}
	}
	switch (data.status) {
		case "not_patched":
			return {
				label: t("patch.status.pristine"),
				className:
					"bg-surface-muted text-text-muted border-border",
				Icon: CheckCircle2,
			};
		case "patched":
			return {
				label: t("patch.status.patched"),
				className:
					"bg-success-muted text-success border-success/40",
				Icon: Package,
			};
		case "interrupted":
			return {
				label: t("patch.status.interrupted"),
				className:
					"bg-warning-muted text-warning border-warning/40",
				Icon: AlertCircle,
			};
		default:
			return {
				label: t("patch.status.unknown"),
				className:
					"bg-warning-muted text-warning border-warning/40",
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
				className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full border text-caption font-medium transition-colors hover:brightness-95 dark:hover:brightness-110 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg bg-surface-muted text-text-muted border-border"
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
				className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full border text-caption font-medium transition-colors hover:brightness-95 dark:hover:brightness-110 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg bg-warning-muted text-warning border-warning/40"
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
			className={`inline-flex items-center gap-1 px-2 py-0.5 rounded-full border text-caption font-medium transition-colors hover:brightness-95 dark:hover:brightness-110 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg ${className}`}
			title={title}
		>
			<Icon size={12} />
			{label}
		</button>
	);
}
