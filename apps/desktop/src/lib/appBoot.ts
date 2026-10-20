import type { AppConfig } from "./api";
import { applyAppearance } from "./appearance";
import { queryClient } from "./queryClient";

/** Share boot config with Layout and Home, including their in-flight request. */
export async function restoreBootAppearance(
  getConfig: () => Promise<AppConfig>,
  apply: typeof applyAppearance = applyAppearance,
): Promise<void> {
  try {
    const config = await queryClient.fetchQuery({ queryKey: ["config"], queryFn: getConfig });
    apply(config.ui);
  } catch { /* backend unreachable — keep defaults */ }
}
