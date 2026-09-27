import { Check, X } from "lucide-react";
import clsx from "clsx";
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
  const currentIndex = STEPS.indexOf(step);
  const content = {
    description: t(`workflow.${step}Desc`),
    action: t(`workflow.${step}Action`),
  };

  return (
    <section
      aria-label={t("workflow.guideAria")}
      className="flex flex-wrap items-center gap-x-4 gap-y-1.5 border-b border-gray-200 bg-emerald-50/40 px-4 py-1.5 dark:border-gray-800 dark:bg-emerald-950/15"
    >
      <ol
        aria-label={t("workflow.stepsAria")}
        className="flex shrink-0 items-center gap-1.5 text-xs"
      >
        {STEPS.map((item, index) => {
          const done = index < currentIndex;
          const current = index === currentIndex;
          return (
            <li key={item} className="flex items-center gap-1.5">
              {index > 0 && (
                <span
                  aria-hidden="true"
                  className={clsx(
                    "h-px w-4",
                    done || current
                      ? "bg-emerald-400 dark:bg-emerald-700"
                      : "bg-gray-300 dark:bg-gray-700",
                  )}
                />
              )}
              <span
                aria-current={current ? "step" : undefined}
                className={clsx(
                  "flex items-center gap-1.5",
                  current
                    ? "font-semibold text-emerald-800 dark:text-emerald-200"
                    : done
                      ? "text-gray-700 dark:text-gray-300"
                      : "text-gray-500 dark:text-gray-400",
                )}
              >
                <span
                  aria-hidden="true"
                  className={clsx(
                    "flex size-4 items-center justify-center rounded-full text-[10px] font-semibold",
                    current
                      ? "bg-emerald-600 text-white"
                      : done
                        ? "bg-emerald-100 text-emerald-700 dark:bg-emerald-900/60 dark:text-emerald-300"
                        : "ring-1 ring-inset ring-gray-300 text-gray-500 dark:ring-gray-600 dark:text-gray-400",
                  )}
                >
                  {done ? <Check size={10} strokeWidth={3} /> : index + 1}
                </span>
                {t(`workflow.${item}`)}
                {done && <span className="sr-only">{t("workflow.stepDone")}</span>}
              </span>
            </li>
          );
        })}
      </ol>

      <p className="min-w-0 flex-1 truncate text-xs text-gray-600 dark:text-gray-400" title={content.description}>
        {content.description}
      </p>

      <div className="ml-auto flex shrink-0 items-center gap-1">
        {step === "review" && onSkipReview && (
          <button
            type="button"
            onClick={onSkipReview}
            className="rounded px-2 py-1 text-xs font-medium text-gray-600 hover:bg-gray-100 hover:text-gray-900 focus:outline-none focus-visible:ring-2 focus-visible:ring-emerald-500 dark:text-gray-300 dark:hover:bg-gray-800 dark:hover:text-white"
          >
            {t("workflow.skipReview")}
          </button>
        )}
        <button
          type="button"
          onClick={onPrimaryAction}
          className="rounded bg-emerald-600 px-2.5 py-1 text-xs font-medium text-white transition-colors hover:bg-emerald-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-emerald-500 focus-visible:ring-offset-1 dark:focus-visible:ring-offset-gray-900"
        >
          {content.action}
        </button>
        <button
          type="button"
          onClick={onDismiss}
          aria-label={t("workflow.dismiss")}
          title={t("workflow.dismiss")}
          className="rounded p-1 text-gray-400 hover:bg-gray-100 hover:text-gray-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-emerald-500 dark:text-gray-500 dark:hover:bg-gray-800 dark:hover:text-gray-200"
        >
          <X aria-hidden="true" size={14} />
        </button>
      </div>
    </section>
  );
}
