import { useMemo } from "react";
import { diff_match_patch, DIFF_DELETE, DIFF_INSERT, DIFF_EQUAL } from "diff-match-patch";
import { useT } from "../lib/i18n";

interface DiffViewProps {
  originalText: string;
  translatedText: string;
  entryId: string;
  compact?: boolean;
  originalLabel?: string;
  translatedLabel?: string;
}

export default function DiffView({ originalText, translatedText, compact = false, originalLabel, translatedLabel }: DiffViewProps) {
  const t = useT();
  const diffs = useMemo(() => {
    const dmp = new diff_match_patch();
    return dmp.diff_main(originalText, translatedText);
  }, [originalText, translatedText]);

  return (
    <div className={`border border-border rounded ${compact ? "p-2" : "p-3 space-y-3"}`}>
      <div className={`grid grid-cols-2 ${compact ? "gap-2" : "gap-4"}`}>
        <div>
          <h4 className="text-caption font-semibold text-text-muted uppercase mb-1">{originalLabel ?? t("review.source")}</h4>
          <div className={`font-mono whitespace-pre-wrap break-words min-w-0 bg-surface-muted rounded ${compact ? "text-caption p-1" : "text-body p-2"}`}>
            {diffs.map(([op, text], i) => {
              if (op === DIFF_EQUAL) return <span key={i}>{text}</span>;
              if (op === DIFF_DELETE) return <span key={i} className="bg-danger-muted text-danger">{text}</span>;
              return null;
            })}
          </div>
        </div>
        <div>
          <h4 className="text-caption font-semibold text-text-muted uppercase mb-1">{translatedLabel ?? t("review.translation")}</h4>
          <div className={`font-mono whitespace-pre-wrap break-words min-w-0 bg-surface-muted rounded ${compact ? "text-caption p-1" : "text-body p-2"}`}>
            {diffs.map(([op, text], i) => {
              if (op === DIFF_EQUAL) return <span key={i}>{text}</span>;
              if (op === DIFF_INSERT) return <span key={i} className="bg-success-muted text-success">{text}</span>;
              return null;
            })}
          </div>
        </div>
      </div>
    </div>
  );
}
