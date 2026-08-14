import { useQueueStore } from "../stores/queueStore";
import { Loader2 } from "lucide-react";
import { useT } from "../lib/i18n";

function formatEta(
	startedAt: number | null,
	completed: number,
	total: number,
	t: (key: string, vars?: Record<string, string | number>) => string,
): string {
	if (!startedAt || completed === 0 || total === 0) return "";
	const elapsed = (Date.now() - startedAt) / 1000;
	const rate = completed / elapsed;
	const remaining = Math.ceil((total - completed) / rate);
	if (remaining < 60) return t("bottom.etaSeconds", { count: remaining });
	return t("bottom.etaMinutes", { count: Math.ceil(remaining / 60) });
}

export default function BottomBar() {
	const t = useT();
	const progress = useQueueStore((s) => s.globalProgress);

	if (!progress || progress.total === 0) return null;

	const percent = Math.round((progress.completed / progress.total) * 100);
	const eta = formatEta(progress.startedAt, progress.completed, progress.total, t);

	return (
		<div className="h-9 flex items-center gap-3 px-4 bg-gray-50 dark:bg-gray-900 border-t border-gray-200 dark:border-gray-700 text-xs shrink-0">
			<Loader2
				size={14}
				className="animate-spin text-emerald-500 dark:text-emerald-400 shrink-0"
			/>
			<span className="font-medium truncate max-w-40 text-gray-800 dark:text-gray-200">
				{progress.projectName}
			</span>

			<div className="flex-1 max-w-64 h-2 bg-gray-200 dark:bg-gray-700 rounded-full overflow-hidden">
				<div
					className="h-full bg-emerald-500 dark:bg-emerald-400 rounded-full transition-all duration-300"
					style={{ width: `${percent}%` }}
				/>
			</div>

			<span className="text-gray-600 dark:text-gray-300 tabular-nums shrink-0">
				{progress.completed}/{progress.total} · {percent}%
			</span>

			{progress.costSoFar > 0 && (
				<span className="text-gray-500 dark:text-gray-400 tabular-nums">
					${progress.costSoFar.toFixed(4)}
				</span>
			)}

			{eta && <span className="text-gray-500 dark:text-gray-400">{eta}</span>}

			{progress.queuePosition != null &&
				progress.queueTotal != null &&
				progress.queueTotal > 1 && (
					<span className="text-gray-500 dark:text-gray-400 ml-auto">
						{t("bottom.project", {
							position: progress.queuePosition,
							total: progress.queueTotal,
						})}
					</span>
				)}
		</div>
	);
}
