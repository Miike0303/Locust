import { useRef } from "react";
import clsx from "clsx";
import {
	useModalA11y,
	MODAL_BACKDROP_CLASS,
	MODAL_FOOTER_CLASS,
	modalPanelClass,
} from "../lib/modalA11y";

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
	confirmLabel = "Confirm",
	cancelLabel = "Cancel",
	destructive = false,
	onConfirm,
	onCancel,
}: ConfirmDialogProps) {
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
				className={modalPanelClass("max-w-sm p-5")}
				onClick={(e) => e.stopPropagation()}
			>
				<h2 {...titleProps} className="text-base font-bold mb-2">
					{title}
				</h2>
				<p className="text-sm text-gray-600 dark:text-gray-300 mb-4 whitespace-pre-wrap">
					{message}
				</p>
				<div className={clsx(MODAL_FOOTER_CLASS, "-mx-5 -mb-5")}>
					<button
						type="button"
						onClick={onCancel}
						className="px-3 py-2 text-sm rounded bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700"
					>
						{cancelLabel}
					</button>
					<button
						ref={confirmRef}
						type="button"
						onClick={onConfirm}
						className={`px-4 py-2 text-sm font-medium rounded text-white ${
							destructive
								? "bg-red-600 hover:bg-red-700"
								: "bg-emerald-600 hover:bg-emerald-700"
						}`}
					>
						{confirmLabel}
					</button>
				</div>
			</div>
		</div>
	);
}
