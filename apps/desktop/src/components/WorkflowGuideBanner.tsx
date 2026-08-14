import { ChevronRight, X } from "lucide-react";
import type { WorkflowGuideStep } from "../lib/workflowGuide";
import { useT } from "../lib/i18n";

interface WorkflowGuideBannerProps {
  step: WorkflowGuideStep;
  onPrimaryAction: () => void;
  onSkipReview?: () => void;
  onDismiss: () => void;
}

const STEPS: readonly WorkflowGuideStep[] = ["translate", "review", "inject"];

export default function WorkflowGuideBanner({
  step,
  onPrimaryAction,
  onSkipReview,
  onDismiss,
}: WorkflowGuideBannerProps) {
  const t = useT();
  const content = {
    description: t(`workflow.${step}Desc`),
    action: t(`workflow.${step}Action`),
  };

  return (
    <section
      aria-label={t("workflow.guideAria")}
      className="flex items-center gap-4 border-b border-emerald-200 bg-emerald-50 px-4 py-2 dark:border-emerald-900 dark:bg-emerald-950/30"
    >
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-3">
          <span className="text-xs font-semibold uppercase tracking-wide text-emerald-700 dark:text-emerald-300">
            {t("workflow.title")}
          </span>
          <ol aria-label={t("workflow.stepsAria")} className="flex items-center gap-1 text-xs">
            {STEPS.map((item, index) => (
              <li key={item} className="flex items-center gap-1">
                {index > 0 && <ChevronRight aria-hidden="true" size={13} className="text-gray-400" />}
                <span
                  aria-current={item === step ? "step" : undefined}
                  className={
                    item === step
                      ? "rounded-full bg-emerald-600 px-2 py-0.5 font-semibold text-white"
                      : "px-1 text-gray-500 dark:text-gray-400"
                  }
                >
                  {t(`workflow.${item}`)}
                </span>
              </li>
            ))}
          </ol>
        </div>
        <p className="mt-0.5 truncate text-sm text-gray-700 dark:text-gray-300">
          {content.description}
        </p>
      </div>

      {step === "review" && onSkipReview && (
        <button
          type="button"
          onClick={onSkipReview}
          className="shrink-0 px-2 py-1.5 text-sm font-medium text-gray-600 hover:text-gray-900 dark:text-gray-300 dark:hover:text-white"
        >
          {t("workflow.skipReview")}
        </button>
      )}
      <button
        type="button"
        onClick={onPrimaryAction}
        className="shrink-0 rounded bg-emerald-600 px-3 py-1.5 text-sm font-medium text-white transition-colors hover:bg-emerald-700"
      >
        {content.action}
      </button>
      <button
        type="button"
        onClick={onDismiss}
        aria-label={t("workflow.dismiss")}
        title={t("workflow.dismiss")}
        className="shrink-0 rounded p-1 text-gray-500 hover:bg-emerald-100 hover:text-gray-800 dark:hover:bg-emerald-900 dark:hover:text-white"
      >
        <X aria-hidden="true" size={18} />
      </button>
    </section>
  );
}
