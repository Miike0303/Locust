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
    <div className="flex flex-col min-[1200px]:flex-row h-full min-h-0 bg-surface-muted text-text text-body">
      <nav className="flex shrink-0 gap-2 overflow-x-auto border-b border-border bg-surface px-4 min-[1200px]:block min-[1200px]:w-56 min-[1200px]:overflow-y-auto min-[1200px]:border-b-0 min-[1200px]:border-r min-[1200px]:p-4 min-[1200px]:space-y-2">
        {SETTINGS_SECTIONS.map(({ id, labelKey }) => (
          <button
            key={id}
            onClick={() => selectSection(id)}
            aria-current={section === id ? "page" : undefined}
            className={clsx(
              "block shrink-0 whitespace-nowrap border-b-2 text-left px-3 py-2 text-body font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent-fg min-[1200px]:w-full min-[1200px]:rounded-lg min-[1200px]:border",
              section === id
                ? "border-accent text-accent-fg font-semibold min-[1200px]:border-accent-fg min-[1200px]:bg-accent-muted"
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
