import { X } from "lucide-react";
import { getGroupedHotkeys, formatKey, useHotkey, type HotkeyActionId } from "../lib/hotkeys";
import { useT, type MessageKey } from "../lib/i18n";
import {
	useModalA11y,
	MODAL_BACKDROP_CLASS,
	modalPanelClass,
} from "../lib/modalA11y";

interface Props {
	open: boolean;
	onClose: () => void;
}

const GROUP_ORDER = ["Navigation", "Editor", "Review", "General"] as const;
type HotkeyGroup = (typeof GROUP_ORDER)[number];

const GROUP_LABEL: Record<HotkeyGroup, MessageKey> = {
	Navigation: "hotkeys.group.Navigation",
	Editor: "hotkeys.group.Editor",
	Review: "hotkeys.group.Review",
	General: "hotkeys.group.General",
};

const ACTION_LABEL: Record<HotkeyActionId, MessageKey> = {
	openProject: "hotkeys.openProject",
	translate: "hotkeys.translate",
	inject: "hotkeys.inject",
	applyPatch: "hotkeys.applyPatch",
	exportFile: "hotkeys.exportFile",
	validate: "hotkeys.validate",
	search: "hotkeys.search",
	searchReplace: "hotkeys.searchReplace",
	reviewMode: "hotkeys.reviewMode",
	settings: "hotkeys.settings",
	memory: "hotkeys.memory",
	closePanel: "hotkeys.closePanel",
	showHelp: "hotkeys.showHelp",
	showHelpF1: "hotkeys.showHelp",
	navHome: "hotkeys.navHome",
	navEditor: "hotkeys.navEditor",
	navReview: "hotkeys.navReview",
	navMemory: "hotkeys.navMemory",
	navSettings: "hotkeys.navSettings",
};

export default function HotkeyHelp({ open, onClose }: Props) {
	const t = useT();
	useHotkey("closePanel", onClose, open, true, true);
	const { dialogRef, dialogProps, titleProps } = useModalA11y({
		open,
		ownEscape: false,
	});

	if (!open) return null;
	const groups = getGroupedHotkeys();

	return (
		<div className={MODAL_BACKDROP_CLASS} onClick={onClose}>
			<div
				ref={dialogRef}
				{...dialogProps}
				className={modalPanelClass("max-w-xl max-h-[80vh] overflow-y-auto")}
				onClick={(e) => e.stopPropagation()}
			>
				<div className="flex items-center justify-between p-4 border-b border-border">
					<h2 {...titleProps} className="text-section font-bold">
						{t("hotkeys.title")}
					</h2>
					<button
						onClick={onClose}
						className="text-text-muted hover:text-text"
					>
						<X size={18} />
					</button>
				</div>

				<div className="p-4 space-y-6">
					{GROUP_ORDER.map((group) => {
						const items = groups[group];
						if (!items || items.length === 0) return null;
						return (
							<div key={group}>
								<h3 className="text-caption font-semibold text-text-muted uppercase mb-2">
									{t(GROUP_LABEL[group])}
								</h3>
								<div className="space-y-1">
									{items.map(({ action, binding }) => (
										<div
											key={action}
											className="flex justify-between items-center py-1"
										>
											<span className="text-body text-text">
												{t(ACTION_LABEL[action as HotkeyActionId])}
											</span>
											<kbd className="px-2 py-0.5 bg-surface-muted rounded text-caption font-mono text-text-muted border border-border">
												{formatKey(binding)}
											</kbd>
										</div>
									))}
								</div>
							</div>
						);
					})}
				</div>

				<div className="p-4 border-t border-border text-center">
					<span className="text-caption text-text-muted">
						{t("hotkeys.closeHint")}
					</span>
				</div>
			</div>
		</div>
	);
}
