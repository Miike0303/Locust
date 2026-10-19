import clsx from "clsx";
import { useEffect } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { useT } from "../lib/i18n";
import {
  SETTINGS_SECTIONS,
  buildSettingsPath,
  parseSettingsSectionParam,
  type SettingsSectionId,
} from "../lib/settingsNav";
import AppearanceSection from "./settings/AppearanceSection";
import DataSection from "./settings/DataSection";
import DefaultsSection from "./settings/DefaultsSection";
import GlossarySection from "./settings/GlossarySection";
import HistorySection from "./settings/HistorySection";
import ProvidersSection from "./settings/ProvidersSection";

export default function Settings() {
  const location = useLocation();
  const navigate = useNavigate();
  const t = useT();
  const section = parseSettingsSectionParam(location.search);

  useEffect(() => {
    const requested = new URLSearchParams(location.search).get("section");
    if (requested !== section) navigate(buildSettingsPath(section), { replace: true });
  }, [location.search, navigate, section]);

  const selectSection = (next: SettingsSectionId) => {
    navigate(buildSettingsPath(next), { replace: true });
  };

  return (
    <div className="flex h-full min-h-0 bg-surface-muted text-text text-body">
      <nav className="w-56 shrink-0 overflow-y-auto border-r border-border bg-surface p-4 space-y-2">
        {SETTINGS_SECTIONS.map(({ id, labelKey }) => (
          <button
            key={id}
            onClick={() => selectSection(id)}
            aria-current={section === id ? "page" : undefined}
            className={clsx(
              "block w-full rounded-lg border text-left px-3 py-2 text-body font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg",
              section === id
                ? "border-accent-fg bg-accent-muted text-accent-fg font-semibold"
                : "border-transparent text-text-muted hover:bg-surface-muted hover:text-text"
            )}
          >
            {t(labelKey)}
          </button>
        ))}
      </nav>
      <div className="flex-1 min-w-0 p-6 lg:p-8 overflow-y-auto [&>div]:mx-auto [&>div]:w-full [&>div]:max-w-5xl">
        {section === "providers" && <ProvidersSection />}
        {section === "defaults" && <DefaultsSection />}
        {section === "appearance" && <AppearanceSection />}
        {section === "glossary" && <GlossarySection />}
        {section === "history" && <HistorySection />}
        {section === "data" && <DataSection />}
      </div>
    </div>
  );
}
