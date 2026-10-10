import { formatObservedCost } from "../lib/translationCost";
import { useQueueStore } from "../stores/queueStore";
import { useEditorStore } from "../stores/editorStore";
import { Loader2 } from "lucide-react";
import { useT, type TranslateFn } from "../lib/i18n";

function formatEta(
	startedAt: number | null,
	completed: number,
	total: number,
	t: TranslateFn,
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
	const queueRunning = useQueueStore((s) => s.isRunning);
	const isTranslating = useEditorStore((s) => s.isTranslating);

	if (!progress || progress.total === 0) return null;
	if (!isTranslating && !queueRunning) return null;

	const percent = Math.round((progress.completed / progress.total) * 100);
	const eta = formatEta(progress.startedAt, progress.completed, progress.total, t);

	return (
		<div className="h-9 flex items-center gap-3 px-4 bg-surface-muted dark:bg-surface border-t border-border text-caption shrink-0">
			<Loader2
				size={14}
				className="animate-spin text-accent-fg shrink-0"
			/>
			<span className="font-medium truncate max-w-40 text-text">
				{progress.projectName}
			</span>

			<div className="flex-1 max-w-64 h-2 bg-border rounded-full overflow-hidden">
				<div
					className="h-full bg-accent rounded-full transition-all duration-300"
					style={{ width: `${percent}%` }}
				/>
			</div>

			<span className="text-text-muted tabular-nums shrink-0">
				{progress.completed}/{progress.total} · {percent}%
			</span>

			{(
				<span className="text-text-muted tabular-nums">
					{formatObservedCost(progress.costSoFar, progress.costIsComplete, t)}
				</span>
			)}

			{eta && <span className="text-text-muted">{eta}</span>}

			{progress.queuePosition != null &&
				progress.queueTotal != null &&
				progress.queueTotal > 1 && (
					<span className="text-text-muted ml-auto">
						{t("bottom.project", {
							position: progress.queuePosition,
							total: progress.queueTotal,
						})}
					</span>
				)}
		</div>
	);
}
