import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { estimateTranslation, type TranslationEstimate, type TranslationStartParams } from "./api";
import { useProjectStore } from "../stores/projectStore";
import { useEditorStore } from "../stores/editorStore";
import { useQueueStore } from "../stores/queueStore";

type Status = "noProject" | "notRequested" | "loading" | "unavailable" | "ready";
interface Options {
  enabled: boolean;
  config: unknown;
  buildParams: () => TranslationStartParams;
}

/** Explicit, source-only requests. Client identity guards cannot attest server project binding. */
export function useTranslationEstimate({ enabled, config, buildParams }: Options) {
  const project = useSyncExternalStore(useProjectStore.subscribe,
    () => useProjectStore.getState().project, () => useProjectStore.getState().project);
  const translating = useSyncExternalStore(useEditorStore.subscribe,
    () => useEditorStore.getState().isTranslating, () => useEditorStore.getState().isTranslating);
  const running = useSyncExternalStore(useQueueStore.subscribe,
    () => useQueueStore.getState().isRunning, () => useQueueStore.getState().isRunning);
  const active = enabled && !translating && !running;
  let params: Pick<TranslationStartParams, "provider_id" | "options"> | undefined;
  try {
    const start = buildParams();
    params = { provider_id: start.provider_id, options: start.options };
  } catch { /* Validation belongs to the existing builder; never substitute defaults or zero. */ }
  const allowed = active && !!project && !!params?.provider_id;
  const key = JSON.stringify([project?.database_path, project?.path, project?.format_id, params, config, active]);
  const current = useRef({ key, generation: 0, pending: false, alive: true });
  // Derive visible state during render, not a later effect: no frame of old cost
  // and no obsolete pending flag when a project/form/config/visibility changes.
  if (current.current.key !== key) {
    current.current = { key, generation: current.current.generation + 1, pending: false, alive: current.current.alive };
  }
  const owner = current.current;
  const [result, setResult] = useState<{
    key: string; generation: number; status: Status; data?: TranslationEstimate;
  } | null>(null);
  useEffect(() => {
    current.current.alive = true;
    return () => { current.current.alive = false; current.current.generation++; };
  }, []);
  const visible = active && result?.key === key && result.generation === owner.generation ? result : null;
  const status: Status = !project ? "noProject" : !params?.provider_id ? "unavailable" : visible?.status ?? "notRequested";
  const pending = allowed && visible?.status === "loading";
  const refresh = async () => {
    if (!allowed || !params || current.current !== owner || !owner.alive || owner.pending) return;
    owner.pending = true;
    const generation = ++owner.generation;
    const owns = () => current.current === owner && owner.key === key && owner.generation === generation && owner.alive;
    setResult({ key, generation, status: "loading" });
    try {
      const data = await estimateTranslation(params);
      if (owns()) setResult({ key, generation, status: "ready", data });
    } catch {
      if (owns()) setResult({ key, generation, status: "unavailable" });
    } finally {
      if (owns()) owner.pending = false;
    }
  };
  return { status, data: visible?.data, pending, disabled: !allowed || pending, refresh };
}

export type TranslationEstimateState = ReturnType<typeof useTranslationEstimate>;
