import { X } from "lucide-react";
import { getGroupedHotkeys, formatKey } from "../lib/hotkeys";

interface Props {
  open: boolean;
  onClose: () => void;
}

const GROUP_ORDER = ["Navigation", "Editor", "Review", "General"];

export default function HotkeyHelp({ open, onClose }: Props) {
  if (!open) return null;
  const groups = getGroupedHotkeys();

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50" onClick={onClose}>
      <div
        className="bg-white dark:bg-gray-900 rounded-lg shadow-xl w-full max-w-xl max-h-[80vh] overflow-y-auto"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between p-4 border-b border-gray-200 dark:border-gray-700">
          <h2 className="text-lg font-bold">Keyboard Shortcuts</h2>
          <button onClick={onClose} className="text-gray-400 hover:text-gray-600">
            <X size={18} />
          </button>
        </div>

        <div className="p-4 space-y-6">
          {GROUP_ORDER.map((group) => {
            const items = groups[group];
            if (!items || items.length === 0) return null;
            return (
              <div key={group}>
                <h3 className="text-xs font-semibold text-gray-500 uppercase mb-2">{group}</h3>
                <div className="space-y-1">
                  {items.map(({ action, binding }) => (
                    <div key={action} className="flex justify-between items-center py-1">
                      <span className="text-sm text-gray-700 dark:text-gray-300">
                        {binding.description}
                      </span>
                      <kbd className="px-2 py-0.5 bg-gray-100 dark:bg-gray-800 rounded text-xs font-mono text-gray-600 dark:text-gray-400 border border-gray-200 dark:border-gray-700">
                        {formatKey(binding)}
                      </kbd>
                    </div>
                  ))}
                </div>
              </div>
            );
          })}
        </div>

        <div className="p-4 border-t border-gray-200 dark:border-gray-700 text-center">
          <span className="text-xs text-gray-400">
            Press <kbd className="px-1 bg-gray-100 dark:bg-gray-800 rounded text-xs">Esc</kbd> to close
          </span>
        </div>
      </div>
    </div>
  );
}
