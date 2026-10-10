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
			<p className="text-section font-medium text-text">{title}</p>
			{description && <p className="text-body text-text-muted">{description}</p>}
			{actionLabel && onAction && (
				<button
					type="button"
					onClick={onAction}
					className="rounded bg-accent px-4 py-2 text-body font-medium text-white hover:bg-accent-hover"
				>
					{actionLabel}
				</button>
			)}
		</div>
	);
}
