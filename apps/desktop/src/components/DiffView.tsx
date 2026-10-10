import { useMemo } from "react";
import { diff_match_patch, DIFF_DELETE, DIFF_INSERT, DIFF_EQUAL } from "diff-match-patch";
import { useT } from "../lib/i18n";

interface DiffViewProps {
  originalText: string;
  translatedText: string;
  entryId: string;
}

export default function DiffView({ originalText, translatedText }: DiffViewProps) {
  const t = useT();
  const diffs = useMemo(() => {
    const dmp = new diff_match_patch();
    return dmp.diff_main(originalText, translatedText);
  }, [originalText, translatedText]);

  return (
    <div className="border border-border rounded p-3 space-y-3">
      <div className="grid grid-cols-2 gap-4">
        <div>
          <h4 className="text-caption font-semibold text-text-muted uppercase mb-1">{t("review.source")}</h4>
          <div className="font-mono text-body whitespace-pre-wrap bg-surface-muted p-2 rounded">
            {diffs.map(([op, text], i) => {
              if (op === DIFF_EQUAL) return <span key={i}>{text}</span>;
              if (op === DIFF_DELETE) return <span key={i} className="bg-danger-muted text-danger">{text}</span>;
              return null;
            })}
          </div>
        </div>
        <div>
          <h4 className="text-caption font-semibold text-text-muted uppercase mb-1">{t("review.translation")}</h4>
          <div className="font-mono text-body whitespace-pre-wrap bg-surface-muted p-2 rounded">
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
