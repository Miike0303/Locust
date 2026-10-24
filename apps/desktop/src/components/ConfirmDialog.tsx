import { useRef } from "react";
import {
	useModalA11y,
	MODAL_BACKDROP_CLASS,
	MODAL_HEADER_CLASS,
	MODAL_BODY_CLASS,
	MODAL_FOOTER_CLASS,
	modalPanelClass,
} from "../lib/modalA11y";
import { useT } from "../lib/i18n";

interface ConfirmDialogProps {
	open: boolean;
	title: string;
	message: string;
	confirmLabel?: string;
	cancelLabel?: string;
	/** Red confirm button for delete/clear/overwrite actions. */
	destructive?: boolean;
	onConfirm: () => void;
	onCancel: () => void;
}

/**
 * Small in-app replacement for window.confirm(): Esc or backdrop click cancels,
 * confirm/cancel controlled by the caller. Same hand-rolled modal style as the rest.
 */
export default function ConfirmDialog({
	open,
	title,
	message,
	confirmLabel,
	cancelLabel,
	destructive = false,
	onConfirm,
	onCancel,
}: ConfirmDialogProps) {
	const t = useT();
	const resolvedConfirm = confirmLabel ?? t("common.confirm");
	const resolvedCancel = cancelLabel ?? t("common.cancel");
	const confirmRef = useRef<HTMLButtonElement>(null);
	const { dialogRef, dialogProps, titleProps } = useModalA11y({
		open,
		onClose: onCancel,
		ownEscape: true,
		initialFocusRef: destructive ? confirmRef : undefined,
	});

	if (!open) return null;

	return (
		<div className={MODAL_BACKDROP_CLASS} onClick={onCancel}>
			<div
				ref={dialogRef}
				{...dialogProps}
				className={modalPanelClass("max-w-sm")}
				onClick={(e) => e.stopPropagation()}
			>
				<header className={`${MODAL_HEADER_CLASS} px-5 pt-5 pb-2`}>
					<h2 {...titleProps} className="text-section font-bold">
						{title}
					</h2>
				</header>
				<div className={`${MODAL_BODY_CLASS} px-5 pb-4`}>
					<p className="text-body text-text-muted whitespace-pre-wrap">
						{message}
					</p>
				</div>
				<footer className={MODAL_FOOTER_CLASS}>
					<button
						type="button"
						onClick={onCancel}
						className="px-3 py-2 text-body rounded border border-border bg-surface text-text hover:bg-surface-muted"
					>
						{resolvedCancel}
					</button>
					<button
						ref={confirmRef}
						type="button"
						onClick={onConfirm}
						className={`px-4 py-2 text-body font-medium rounded ${destructive
							? "bg-danger text-danger-contrast hover:bg-danger/90"
							: "bg-accent text-white hover:bg-accent-hover"
							}`}
					>
						{resolvedConfirm}
					</button>
				</footer>
			</div>
		</div>
	);
}
