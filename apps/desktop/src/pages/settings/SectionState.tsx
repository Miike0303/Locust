import { useT } from "../../lib/i18n";

type SectionStateProps = { title: string } & (
  | { state: "loading" }
  | { state: "error"; error: Error | null; onRetry: () => void }
  | { state: "empty"; message: string }
);

export default function SectionState(props: SectionStateProps) {
  const t = useT();

  return (
    <div className="space-y-6 border border-border bg-surface rounded-xl p-5">
      <h2 className="text-page font-bold">{props.title}</h2>
      {props.state === "loading" ? (
        <p role="status" className="text-body text-text-muted">{t("common.loading")}</p>
      ) : props.state === "error" ? (
        <div role="alert" className="space-y-3 text-body">
          <p className="text-danger">{t("settings.loadFailed")}</p>
          <p className="text-text-muted break-words">{props.error?.message}</p>
          <button type="button" onClick={props.onRetry}
            className="px-3 py-2 border border-border bg-surface-muted hover:bg-surface text-text rounded-md text-body font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg">
            {t("common.retry")}
          </button>
        </div>
      ) : (
        <p className="text-body text-text-muted">{props.message}</p>
      )}
    </div>
  );
}
