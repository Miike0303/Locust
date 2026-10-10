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
      className="flex flex-wrap items-center gap-x-4 gap-y-1.5 border-b border-border bg-accent-muted/40 px-4 py-1.5"
    >
      <ol
        aria-label={t("workflow.stepsAria")}
        className="flex shrink-0 items-center gap-1.5 text-caption"
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
                      ? "bg-accent"
                      : "bg-border",
                  )}
                />
              )}
              <span
                aria-current={current ? "step" : undefined}
                className={clsx(
                  "flex items-center gap-1.5",
                  current
                    ? "font-semibold text-accent-fg"
                    : done
                      ? "text-text"
                      : "text-text-muted",
                )}
              >
                <span
                  aria-hidden="true"
                  className={clsx(
                    "flex size-4 items-center justify-center rounded-full text-caption font-semibold",
                    current
                      ? "bg-accent text-white"
                      : done
                        ? "bg-accent-muted text-accent-fg"
                        : "ring-1 ring-inset ring-border text-text-muted",
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

      <p className="min-w-0 flex-1 truncate text-caption text-text-muted" title={content.description}>
        {content.description}
      </p>

      <div className="ml-auto flex shrink-0 items-center gap-1">
        {step === "review" && onSkipReview && (
          <button
            type="button"
            onClick={onSkipReview}
            className="rounded px-2 py-1 text-caption font-medium text-text-muted hover:bg-surface-muted hover:text-text focus:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg"
          >
            {t("workflow.skipReview")}
          </button>
        )}
        <button
          type="button"
          onClick={onPrimaryAction}
          className="rounded bg-accent px-2.5 py-1 text-caption font-medium text-white transition-colors hover:bg-accent-hover focus:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-1 focus-visible:ring-offset-surface"
        >
          {content.action}
        </button>
        <button
          type="button"
          onClick={onDismiss}
          aria-label={t("workflow.dismiss")}
          title={t("workflow.dismiss")}
          className="rounded p-1 text-text-muted hover:bg-surface-muted hover:text-text focus:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg"
        >
          <X aria-hidden="true" size={14} />
        </button>
      </div>
    </section>
  );
}
