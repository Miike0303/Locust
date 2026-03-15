import { useMemo } from "react";
import { diff_match_patch, DIFF_DELETE, DIFF_INSERT, DIFF_EQUAL } from "diff-match-patch";

interface DiffViewProps {
  originalText: string;
  translatedText: string;
  entryId: string;
}

export default function DiffView({ originalText, translatedText, entryId }: DiffViewProps) {
  const diffs = useMemo(() => {
    const dmp = new diff_match_patch();
    return dmp.diff_main(originalText, translatedText);
  }, [originalText, translatedText]);

  return (
    <div className="border border-gray-200 dark:border-gray-700 rounded p-3 space-y-3">
      <div className="grid grid-cols-2 gap-4">
        <div>
          <h4 className="text-xs font-semibold text-gray-500 uppercase mb-1">Source</h4>
          <div className="font-mono text-sm whitespace-pre-wrap bg-gray-50 dark:bg-gray-800 p-2 rounded">
            {diffs.map(([op, text], i) => {
              if (op === DIFF_EQUAL) return <span key={i}>{text}</span>;
              if (op === DIFF_DELETE) return <span key={i} className="bg-red-200 dark:bg-red-900/50 text-red-800 dark:text-red-300">{text}</span>;
              return null;
            })}
          </div>
        </div>
        <div>
          <h4 className="text-xs font-semibold text-gray-500 uppercase mb-1">Translation</h4>
          <div className="font-mono text-sm whitespace-pre-wrap bg-gray-50 dark:bg-gray-800 p-2 rounded">
            {diffs.map(([op, text], i) => {
              if (op === DIFF_EQUAL) return <span key={i}>{text}</span>;
              if (op === DIFF_INSERT) return <span key={i} className="bg-blue-200 dark:bg-blue-900/50 text-blue-800 dark:text-blue-300">{text}</span>;
              return null;
            })}
          </div>
        </div>
      </div>
    </div>
  );
}
