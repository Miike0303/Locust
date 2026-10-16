import { Link } from "react-router-dom";
import { t } from "../lib/i18n";

/** Unknown URLs retain the shell and offer recovery without reloading the app. */
export default function NotFound() {
  return (
    <section className="flex min-h-full items-center justify-center p-6">
      <div className="w-full max-w-md rounded-xl border border-border bg-surface p-6">
        <h1 className="text-page font-semibold text-text">{t("route.notFound.title")}</h1>
        <p className="mt-3 text-body text-text-muted">{t("route.notFound.body")}</p>
        <Link
          to="/"
          className="mt-5 inline-flex rounded-md bg-accent px-4 py-2 text-body font-medium text-white hover:bg-accent-hover focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-2 focus-visible:ring-offset-surface"
        >
          {t("route.notFound.home")}
        </Link>
      </div>
    </section>
  );
}
