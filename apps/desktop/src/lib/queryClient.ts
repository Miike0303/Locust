import { QueryClient } from "@tanstack/react-query";

export const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: 1, staleTime: 30_000 } },
});

/** A failed mutation may still have written files or left recovery evidence. */
export function refreshGameState(): void {
  void queryClient.invalidateQueries({ queryKey: ["patchStatus"] });
}
