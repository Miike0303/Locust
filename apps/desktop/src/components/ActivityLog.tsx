import { useState } from "react";
import { X, Trash2 } from "lucide-react";
import clsx from "clsx";
import { useLogStore, type LogLevel } from "../stores/logStore";
import { useT, type TranslateFn } from "../lib/i18n";

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

	if (!isOpen) return null;

	const filtered =
		filter === "all" ? entries : entries.filter((e) => e.level === filter);

	return (
		<div className="fixed inset-y-0 right-0 w-96 bg-surface text-text border-l border-border z-50 flex flex-col shadow-xl">
			{/* Header */}
			<div className="flex items-center justify-between p-4 border-b border-border">
				<h2 className="text-section font-bold text-text">
					{t("log.title")}
				</h2>
				<div className="flex items-center gap-2">
					<button
						type="button"
						onClick={clear}
						aria-label={t("log.clearAria")}
						className="text-text-muted hover:text-text focus:outline-none focus:ring-2 focus:ring-accent-fg rounded p-0.5"
					>
						<Trash2 size={16} />
					</button>
					<button
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
					filtered.map((entry) => (
						<div
							key={entry.id}
							className={clsx(
								"px-4 py-2.5 border-b border-border cursor-pointer hover:bg-surface-muted/60",
								levelBg[entry.level],
							)}
							onClick={() =>
								setExpandedId(expandedId === entry.id ? null : entry.id)
							}
						>
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
							{expandedId === entry.id && entry.detail && (
								<pre className="mt-2 ml-4 p-2 bg-surface-muted text-text rounded text-caption overflow-x-auto whitespace-pre-wrap border border-border">
									{entry.detail}
								</pre>
							)}
						</div>
					))
				)}
			</div>
		</div>
	);
}
