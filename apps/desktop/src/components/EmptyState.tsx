import { Link } from "react-router-dom";

const ACTION = "rounded bg-accent px-4 py-2 text-body font-medium text-white hover:bg-accent-hover focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-2 focus-visible:ring-offset-surface";

interface EmptyStateProps {
	title: string;
	description?: string;
	actionLabel?: string;
	actionTo?: string;
	onAction?: () => void;
}

export default function EmptyState({
	title,
	description,
	actionLabel,
	actionTo,
	onAction,
}: EmptyStateProps) {
	return (
		<div className="flex h-full flex-1 flex-col items-center justify-center gap-3 px-6 text-center">
			<p className="text-section font-medium text-text">{title}</p>
			{description && <p className="text-body text-text-muted">{description}</p>}
			{actionLabel && actionTo ? (
				<Link to={actionTo} className={ACTION}>{actionLabel}</Link>
			) : actionLabel && onAction && (
				<button
					type="button"
					onClick={onAction}
					className={ACTION}
				>
					{actionLabel}
				</button>
			)}
		</div>
	);
}
