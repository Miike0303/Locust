interface EmptyStateProps {
	title: string;
	description?: string;
	actionLabel?: string;
	onAction?: () => void;
}

export default function EmptyState({
	title,
	description,
	actionLabel,
	onAction,
}: EmptyStateProps) {
	return (
		<div className="flex h-full flex-1 flex-col items-center justify-center gap-3 px-6 text-center">
			<p className="font-medium text-gray-700 dark:text-gray-200">{title}</p>
			{description && <p className="text-sm text-gray-500">{description}</p>}
			{actionLabel && onAction && (
				<button
					type="button"
					onClick={onAction}
					className="rounded bg-emerald-600 px-4 py-2 text-sm font-medium text-white hover:bg-emerald-700"
				>
					{actionLabel}
				</button>
			)}
		</div>
	);
}
