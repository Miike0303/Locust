import { useEffect, useId, useRef, type RefObject } from "react";

export interface FocusCandidate {
	isConnected?: boolean;
	focus?: () => void;
}

export function buildModalDialogProps({
	titleId,
	ariaLabel,
}: {
	titleId?: string;
	ariaLabel?: string;
}) {
	return {
		role: "dialog" as const,
		"aria-modal": true as const,
		...(ariaLabel
			? { "aria-label": ariaLabel }
			: { "aria-labelledby": titleId }),
		tabIndex: -1,
		"data-hotkey-overlay": "",
	};
}

export function buildModalTitleProps(titleId: string) {
	return { id: titleId };
}

export function chooseInitialFocus<T>({
	preferredInRoot,
	preferred,
	firstFocusable,
	root,
}: {
	preferredInRoot: boolean;
	preferred: T | null;
	firstFocusable: T | null;
	root: T | null;
}): T | null {
	if (preferredInRoot && preferred) return preferred;
	return firstFocusable ?? root;
}

export function canRestoreFocus(target: FocusCandidate | null): boolean {
	return target?.isConnected === true;
}

export function shouldOwnModalEscape({
	open,
	ownEscape,
}: {
	open: boolean;
	ownEscape: boolean;
}): boolean {
	return open && ownEscape;
}

export const MODAL_BACKDROP_CLASS =
	"fixed inset-0 bg-black/50 flex items-center justify-center z-50";

/** Shared modal panel chrome; pass size/overflow utilities as extraClass. */
export function modalPanelClass(extraClass = ""): string {
	const base = "bg-white dark:bg-gray-900 rounded-lg shadow-xl w-full";
	return extraClass ? `${base} ${extraClass}` : base;
}

export function isTabFocusTrapKey(key: string): boolean {
	return key === "Tab";
}

export function isTopmostOverlayDialog(el: HTMLElement | null): boolean {
	if (!el) return false;
	const dialogs = document.querySelectorAll<HTMLElement>(
		"[role='dialog'][data-hotkey-overlay]",
	);
	return dialogs.item(dialogs.length - 1) === el;
}

export function resolveFocusTrapTarget({
	focusable,
	active,
	shiftKey,
}: {
	focusable: readonly HTMLElement[];
	active: HTMLElement | null;
	shiftKey: boolean;
}): HTMLElement | null {
	if (focusable.length === 0) return null;
	const first = focusable[0];
	const last = focusable[focusable.length - 1];
	const activeInList = active !== null && focusable.includes(active);
	if (shiftKey) {
		if (!activeInList || active === first) return last;
		return null;
	}
	if (!activeInList || active === last) return first;
	return null;
}

export const FOCUSABLE_SELECTOR = [
	"button:not([disabled])",
	"[href]",
	"input:not([disabled])",
	"select:not([disabled])",
	"textarea:not([disabled])",
	"[tabindex]:not([tabindex='-1'])",
].join(",");

export function listFocusableElements(root: HTMLElement): HTMLElement[] {
	return Array.from(
		root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR),
	).filter((el) => !el.hasAttribute("disabled") && el.tabIndex !== -1);
}

export function useModalA11y({
	open,
	onClose,
	ownEscape = false,
	ariaLabel,
	initialFocusRef,
}: {
	open: boolean;
	onClose?: () => void;
	ownEscape?: boolean;
	ariaLabel?: string;
	initialFocusRef?: RefObject<HTMLElement | null>;
}) {
	const dialogRef = useRef<HTMLDivElement>(null);
	const titleId = useId();
	const closeRef = useRef(onClose);
	closeRef.current = onClose;

	useEffect(() => {
		if (!open) return;
		const previouslyFocused = document.activeElement as HTMLElement | null;
		const root = dialogRef.current;
		const preferred = initialFocusRef?.current ?? null;
		const firstFocusable =
			root?.querySelector<HTMLElement>(FOCUSABLE_SELECTOR) ?? null;
		const target = chooseInitialFocus({
			preferredInRoot: !!(root && preferred && root.contains(preferred)),
			preferred,
			firstFocusable,
			root,
		});
		target?.focus?.();

		return () => {
			if (previouslyFocused && canRestoreFocus(previouslyFocused)) {
				try {
					previouslyFocused.focus();
				} catch {
					// The prior surface may become unfocusable while the modal is open.
				}
			}
		};
	}, [open, initialFocusRef]);

	useEffect(() => {
		if (!shouldOwnModalEscape({ open, ownEscape })) return;
		const handleEscape = (event: KeyboardEvent) => {
			if (event.key !== "Escape") return;
			const dialogs = document.querySelectorAll<HTMLElement>(
				"[role='dialog'][data-hotkey-overlay]",
			);
			if (dialogRef.current !== dialogs.item(dialogs.length - 1)) return;
			event.preventDefault();
			event.stopImmediatePropagation();
			closeRef.current?.();
		};
		window.addEventListener("keydown", handleEscape, true);
		return () => window.removeEventListener("keydown", handleEscape, true);
	}, [open, ownEscape]);

	useEffect(() => {
		if (!open) return;
		const handleTabTrap = (event: KeyboardEvent) => {
			if (!isTabFocusTrapKey(event.key)) return;
			const dialog = dialogRef.current;
			if (!dialog || !isTopmostOverlayDialog(dialog)) return;
			const focusable = listFocusableElements(dialog);
			const trapTarget = resolveFocusTrapTarget({
				focusable,
				active: document.activeElement as HTMLElement | null,
				shiftKey: event.shiftKey,
			});
			if (!trapTarget) return;
			event.preventDefault();
			trapTarget.focus();
		};
		window.addEventListener("keydown", handleTabTrap, true);
		return () => window.removeEventListener("keydown", handleTabTrap, true);
	}, [open]);

	return {
		dialogRef,
		titleId,
		dialogProps: buildModalDialogProps(ariaLabel ? { ariaLabel } : { titleId }),
		titleProps: buildModalTitleProps(titleId),
	};
}
