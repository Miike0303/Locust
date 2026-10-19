import { useQueryClient } from "@tanstack/react-query";
import type { ConfigUpdate } from "../../lib/api";
import { updateConfig } from "../../lib/api";
import { applyAppearance } from "../../lib/appearance";
import { useT } from "../../lib/i18n";
import { addToast } from "../../stores/toastStore";

export default function useSettingsSave() {
  const t = useT();
  const qc = useQueryClient();
  return async (partial: ConfigUpdate) => {
    try {
      const saved = await updateConfig(partial);
      qc.setQueryData(["config"], saved);
      applyAppearance(saved.ui);
      if (partial.providers) void qc.invalidateQueries({ queryKey: ["providers"] });
      return saved;
    } catch (error) {
      addToast("error", t("settings.saveFailed", { error: error instanceof Error ? error.message : String(error) }));
      return null;
    }
  };
}
