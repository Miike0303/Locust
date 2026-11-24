import { useRef, useState } from "react";
import { X, Trash2 } from "lucide-react";
import clsx from "clsx";
import { useLogStore, type LogLevel } from "../stores/logStore";
import { useT, type TranslateFn } from "../lib/i18n";
import { useModalA11y } from "../lib/modalA11y";
import ConfirmDialog from "./ConfirmDialog";

const levelColors: Record<LogLevel, string> = {
	info: "bg-blue-500 dark:bg-blue-400",
	warning: "bg-warning",
	error: "bg-danger",
};

const levelBg: Record<LogLevel, string> = {
	info: "",
	warning: "bg-warning-muted/50",
	error: "bg-danger-muted",
};

const filters: Array<LogLevel | "all"> = ["all", "info", "warning", "error"];

function timeAgo(ts: number, t: TranslateFn): string {
	const sec = Math.floor((Date.now() - ts) / 1000);
	if (sec < 60) return t("log.timeSeconds", { count: sec });
	const min = Math.floor(sec / 60);
	if (min < 60) return t("log.timeMinutes", { count: min });
	const hr = Math.floor(min / 60);
	return t("log.timeHours", { count: hr });
}

export default function ActivityLog() {
	const t = useT();
	const { entries, filter, isOpen, setOpen, setFilter, clear } = useLogStore();
	const [expandedId, setExpandedId] = useState<string | null>(null);
	const [confirmClear, setConfirmClear] = useState(false);
	const closeRef = useRef<HTMLButtonElement>(null);
	// A dialog panel: Escape closes it (a nested confirmation takes Escape first),
	// focus starts on Close and returns to the opener.
	const { dialogRef, dialogProps, titleProps } = useModalA11y({
		open: isOpen,
		onClose: () => setOpen(false),
		ownEscape: true,
		initialFocusRef: closeRef,
	});

	if (!isOpen) return null;

	const filtered =
		filter === "all" ? entries : entries.filter((e) => e.level === filter);

	return (
		<div
			ref={dialogRef}
			{...dialogProps}
			className="fixed inset-y-0 right-0 w-96 bg-surface text-text border-l border-border z-50 flex flex-col shadow-xl focus:outline-none"
		>
			{/* Header */}
			<div className="flex items-center justify-between p-4 border-b border-border">
				<h2 {...titleProps} className="text-section font-bold text-text">
					{t("log.title")}
				</h2>
				<div className="flex items-center gap-2">
					<button
						type="button"
						onClick={() => setConfirmClear(true)}
						aria-label={t("log.clearAria")}
						className="text-text-muted hover:text-text focus:outline-none focus:ring-2 focus:ring-accent-fg rounded p-0.5"
					>
						<Trash2 size={16} />
					</button>
					<button
						ref={closeRef}
						type="button"
						onClick={() => setOpen(false)}
						aria-label={t("log.closeAria")}
						className="text-text-muted hover:text-text focus:outline-none focus:ring-2 focus:ring-accent-fg rounded p-0.5"
					>
						<X size={18} />
					</button>
				</div>
			</div>

			{/* Filter tabs */}
			<div className="flex gap-1 px-4 py-2 border-b border-border">
				{filters.map((f) => (
					<button
						key={f}
						type="button"
						onClick={() => setFilter(f)}
						className={clsx(
							"px-3 py-1 rounded-full text-caption font-medium transition-colors capitalize focus:outline-none focus:ring-2 focus:ring-accent-fg",
							filter === f
								? "bg-accent-muted text-accent-fg"
								: "text-text-muted hover:bg-surface-muted",
						)}
					>
						{t(`log.${f}`)}
						{f !== "all" && (
							<span className="ml-1 opacity-60">
								({entries.filter((e) => e.level === f).length})
							</span>
						)}
					</button>
				))}
			</div>

			{/* Entries */}
			<div className="flex-1 overflow-y-auto">
				{filtered.length === 0 ? (
					<div className="p-8 text-center text-body text-text-muted">
						{t("log.empty")}
					</div>
				) : (
					filtered.map((entry) => {
						// Only entries with details expand, so only they are buttons.
						const expandable = !!entry.detail;
						const expanded = expandable && expandedId === entry.id;
						const detailId = `activity-log-detail-${entry.id}`;
						const summary = (
							<div className="flex items-start gap-2">
								<div
									className={clsx(
										"w-2 h-2 rounded-full mt-1.5 shrink-0",
										levelColors[entry.level],
									)}
								/>
								<div className="flex-1 min-w-0">
									<div className="text-body leading-snug text-text">
										{entry.message}
									</div>
									<div className="flex gap-2 mt-0.5 text-caption text-text-muted">
										<span>{timeAgo(entry.timestamp, t)}</span>
										{entry.source && (
											<span className="text-text-muted">
												· {entry.source}
											</span>
										)}
									</div>
								</div>
							</div>
						);
						return (
							<div
								key={entry.id}
								className={clsx("border-b border-border", levelBg[entry.level])}
							>
								{expandable ? (
									<button
										type="button"
										aria-expanded={expanded}
										aria-controls={detailId}
										onClick={() => setExpandedId(expanded ? null : entry.id)}
										className="block w-full px-4 py-2.5 text-left hover:bg-surface-muted/60 focus:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent-fg"
									>
										{summary}
									</button>
								) : (
									<div className="px-4 py-2.5">{summary}</div>
								)}
								{expanded && (
									<pre
										id={detailId}
										className="mx-4 mb-2.5 ml-8 p-2 bg-surface-muted text-text rounded text-caption overflow-x-auto whitespace-pre-wrap border border-border"
									>
										{entry.detail}
									</pre>
								)}
							</div>
						);
					})
				)}
			</div>
			<ConfirmDialog
				open={confirmClear}
				title={t("log.clearConfirm.title")}
				message={t("log.clearConfirm.message", { count: entries.length })}
				confirmLabel={t("log.clearConfirm.confirm")}
				destructive
				onConfirm={() => {
					clear();
					setExpandedId(null);
					setConfirmClear(false);
				}}
				onCancel={() => setConfirmClear(false)}
			/>
		</div>
	);
}
