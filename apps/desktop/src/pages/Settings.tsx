import { IS_TAURI } from "../lib/runtime";
import { formatObservedCost } from "../lib/translationCost";
import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useLocation, useNavigate } from "react-router-dom";
import { CheckCircle, XCircle, Loader, Trash2, RotateCcw, Plus, Search, Copy, ExternalLink } from "lucide-react";
import clsx from "clsx";
import {
  getProviders, checkProviderHealth, getConfig, updateConfig,
  getBackupsReport, restoreBackup, deleteBackup,
  getGlossary, addGlossaryEntry, deleteGlossaryEntry,
  getTranslationRuns,
  xaiAuthStart, xaiAuthPoll,
} from "../lib/api";
import type { GlossaryEntry, TranslationRun, ProviderInfo, ConfigUpdate, BackupRestoreReport } from "../lib/api";
import { applyAppearance, clampTableRowHeight, TABLE_ROW_HEIGHT_MAX, TABLE_ROW_HEIGHT_MIN } from "../lib/appearance";
import { resolveProviderReadiness } from "../lib/providerReadiness";
import { settingsQueryState } from "../lib/settingsQueryState";
import {
  GROK_SUB_PROVIDER_ID,
  XAI_POLL_INTERVAL_MS,
  grokSubIsReady,
  nextXaiPollAction,
  type XaiAuthPollStatus,
} from "../lib/xaiAuth";
import {
  SETTINGS_SECTIONS,
  buildSettingsPath,
  parseSettingsSectionParam,
  type SettingsSectionId,
} from "../lib/settingsNav";
import { useT, useLocale, type Locale, type TranslateFn } from "../lib/i18n";
import ConfirmDialog from "../components/ConfirmDialog";
import BufferedSetting, { SETTINGS_INPUT_CLASS } from "../components/BufferedSetting";
import { addToast } from "../stores/toastStore";

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

/** Last folder (or file) name from a backup source path. Handles Windows and POSIX. */
function gameFolderName(sourcePath: string): string {
  const trimmed = sourcePath.replace(/[\\/]+$/, "").trim();
  if (!trimmed) return "";
  const parts = trimmed.split(/[\\/]/);
  return parts[parts.length - 1] || trimmed;
}

function formatRunDate(iso: string): string {
  if (!iso) return "—";
  // Prefer local short form; fall back to first 16 chars of ISO.
  const d = new Date(iso);
  if (!Number.isNaN(d.getTime())) {
    return d.toLocaleString(undefined, {
      year: "numeric",
      month: "short",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
    });
  }
  return iso.length >= 16 ? iso.slice(0, 16) : iso;
}

function formatDuration(secs: number, t: TranslateFn): string {
  const s = Math.max(0, Math.floor(secs));
  if (s >= 3600) {
    return t("settings.history.durationHoursMinutes", {
      hours: Math.floor(s / 3600),
      minutes: Math.floor((s % 3600) / 60),
    });
  }
  if (s >= 60) {
    return t("settings.history.durationMinutesSeconds", {
      minutes: Math.floor(s / 60),
      seconds: s % 60,
    });
  }
  return t("settings.history.durationSeconds", { seconds: s });
}

function HistorySection() {
  const t = useT();
  const { data: runs, isLoading, isError, error, refetch } = useQuery({
    queryKey: ["translation-runs"],
    queryFn: getTranslationRuns,
  });
  const queryState = isError ? settingsQueryState(error) : null;

  const totals = useMemo(() => {
    const list = runs ?? [];
    return list.reduce(
      (acc, r) => {
        acc.strings += r.strings_translated;
        acc.tokens += r.tokens_used;
        acc.input += r.input_tokens;
        acc.output += r.output_tokens;
        acc.cost += r.cost_usd;
        acc.complete &&= r.cost_is_complete === true;
        acc.secs += r.duration_secs;
        return acc;
      },
      { strings: 0, tokens: 0, input: 0, output: 0, cost: 0, secs: 0, complete: true }
    );
  }, [runs]);

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="min-w-0">
          <h2 className="text-page font-bold">{t("settings.history.title")}</h2>
          <p className="text-body text-text-muted mt-1">
            {t("settings.history.description")}
          </p>
        </div>
        <button
          type="button"
          onClick={() => refetch()}
          className="text-body text-accent-fg hover:underline"
        >
          {t("common.refresh")}
        </button>
      </div>

      {isLoading && (
        <div className="flex items-center gap-2 text-body text-text-muted">
          <Loader size={16} className="animate-spin" /> {t("settings.history.loading")}
        </div>
      )}

      {queryState === "no_project" && (
        <div role="status" className="text-body text-text-muted">
          {t("settings.history.noProject")}
        </div>
      )}

      {queryState === "failed" && (
        <div className="text-body text-danger">
          {t("settings.history.loadFailed", {
            error: (error as Error)?.message ?? t("settings.history.unknownError"),
          })}
        </div>
      )}

      {!isLoading && !isError && (runs?.length ?? 0) === 0 && (
        <div className="border border-dashed border-border bg-surface rounded-xl p-8 text-center text-body text-text-muted">
          {t("settings.history.empty")}
        </div>
      )}

      {!isLoading && !isError && (runs?.length ?? 0) > 0 && (
        <div className="overflow-x-auto border border-border bg-surface rounded-xl">
          <table className="w-full min-w-[56rem] text-body">
            <thead>
              <tr className="bg-surface-muted text-left text-caption font-semibold text-text-muted uppercase tracking-wide">
                <th className="px-3 py-2">{t("settings.history.col.date")}</th>
                <th className="px-3 py-2">{t("settings.history.col.provider")}</th>
                <th className="px-3 py-2">{t("settings.history.col.langs")}</th>
                <th className="px-3 py-2 text-right">{t("settings.history.col.strings")}</th>
                <th className="px-3 py-2 text-right">{t("settings.history.col.tokens")}</th>
                <th className="px-3 py-2 text-right">{t("settings.history.col.in")}</th>
                <th className="px-3 py-2 text-right">{t("settings.history.col.out")}</th>
                <th className="px-3 py-2 text-right">{t("settings.history.col.cost")}</th>
                <th className="px-3 py-2 text-right">{t("settings.history.col.duration")}</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {runs!.map((r: TranslationRun) => (
                <tr key={r.id} className="hover:bg-surface-muted">
                  <td className="px-3 py-2 whitespace-nowrap text-text">
                    {formatRunDate(r.started_at)}
                  </td>
                  <td className="px-3 py-2 font-mono text-caption" title={r.provider}>
                    {r.provider}
                  </td>
                  <td className="px-3 py-2 whitespace-nowrap">
                    {r.source_lang}→{r.target_lang}
                  </td>
                  <td className="px-3 py-2 text-right tabular-nums">{r.strings_translated}</td>
                  <td className="px-3 py-2 text-right tabular-nums">{r.tokens_used}</td>
                  <td className="px-3 py-2 text-right tabular-nums text-text-muted">
                    {r.input_tokens}
                  </td>
                  <td className="px-3 py-2 text-right tabular-nums text-text-muted">
                    {r.output_tokens}
                  </td>
                  <td className="px-3 py-2 text-right tabular-nums">
                    {formatObservedCost(r.cost_usd, r.cost_is_complete, t)}
                  </td>
                  <td className="px-3 py-2 text-right tabular-nums whitespace-nowrap">
                    {formatDuration(r.duration_secs, t)}
                  </td>
                </tr>
              ))}
            </tbody>
            <tfoot>
              <tr className="bg-surface-muted font-semibold text-text border-t border-border">
                <td className="px-3 py-2" colSpan={3}>
                  {t("settings.history.totalRuns", { count: runs!.length })}
                </td>
                <td className="px-3 py-2 text-right tabular-nums">{totals.strings}</td>
                <td className="px-3 py-2 text-right tabular-nums">{totals.tokens}</td>
                <td className="px-3 py-2 text-right tabular-nums text-text-muted">
                  {totals.input}
                </td>
                <td className="px-3 py-2 text-right tabular-nums text-text-muted">
                  {totals.output}
                </td>
                <td className="px-3 py-2 text-right tabular-nums">
                  {formatObservedCost(totals.cost, totals.complete, t)}
                </td>
                <td className="px-3 py-2 text-right tabular-nums whitespace-nowrap">
                  {formatDuration(totals.secs, t)}
                </td>
              </tr>
            </tfoot>
          </table>
        </div>
      )}
    </div>
  );
}

async function openExternalUrl(url: string): Promise<void> {
  if (IS_TAURI) {
    const { open } = await import("@tauri-apps/plugin-shell");
    await open(url);
    return;
  }
  window.open(url, "_blank", "noopener,noreferrer");
}

function GrokSubCard({
  provider,
  model,
  onModelChange,
  onTest,
  testing,
  testResult,
}: {
  provider?: ProviderInfo;
  model?: string;
  onModelChange: (model: string) => void;
  onTest: () => void;
  testing: boolean;
  testResult?: { ok: boolean; message: string };
}) {
  const t = useT();
  const qc = useQueryClient();
  const ready = grokSubIsReady(provider ? [provider] : []);
  const [starting, setStarting] = useState(false);
  const [copied, setCopied] = useState(false);
  const [phase, setPhase] = useState<"idle" | XaiAuthPollStatus>("idle");
  const [session, setSession] = useState<{
    handle: string;
    user_code: string;
    verification_uri: string;
    expires_in_secs: number;
  } | null>(null);

  useEffect(() => {
    if (phase !== "pending" || !session) return;
    let stopped = false;
    const startedAt = Date.now();

    const tick = async () => {
      if (stopped) return;
      const expiry = nextXaiPollAction("pending", {
        startedAtMs: startedAt,
        expiresInSecs: session.expires_in_secs,
        nowMs: Date.now(),
      });
      if (expiry.action === "stop") {
        setPhase("expired");
        addToast("error", t("settings.providers.toast.expired"));
        return;
      }
      try {
        const r = await xaiAuthPoll(session.handle);
        if (stopped) return;
        const next = nextXaiPollAction(r.status, {
          startedAtMs: startedAt,
          expiresInSecs: session.expires_in_secs,
          nowMs: Date.now(),
        });
        if (next.action === "stop") {
          setPhase(next.outcome);
          if (next.outcome === "complete") {
            addToast("success", t("settings.providers.toast.signedIn"));
            void qc.invalidateQueries({ queryKey: ["providers"] });
            void qc.invalidateQueries({ queryKey: ["config"] });
          } else if (next.outcome === "denied") {
            addToast("error", t("settings.providers.toast.denied"));
          } else {
            addToast("error", t("settings.providers.toast.expired"));
          }
        }
      } catch {
        /* keep polling until expiry */
      }
    };

    void tick();
    const id = window.setInterval(tick, XAI_POLL_INTERVAL_MS);
    return () => {
      stopped = true;
      window.clearInterval(id);
    };
  }, [phase, session, qc, t]);

  const startSignIn = async () => {
    setStarting(true);
    setCopied(false);
    try {
      const started = await xaiAuthStart();
      setSession(started);
      setPhase("pending");
    } catch (e: unknown) {
      const message = e instanceof Error ? e.message : String(e);
      addToast("error", t("settings.providers.toast.startFailed", { error: message }));
      setPhase("idle");
    } finally {
      setStarting(false);
    }
  };

  const copyCode = async () => {
    if (!session) return;
    try {
      await navigator.clipboard.writeText(session.user_code);
      setCopied(true);
    } catch {
      addToast("error", t("settings.providers.toast.copyFailed"));
    }
  };

  const openVerification = async () => {
    if (!session) return;
    try {
      await openExternalUrl(session.verification_uri);
    } catch (e: unknown) {
      const message = e instanceof Error ? e.message : String(e);
      addToast("error", t("settings.providers.toast.openFailed", { error: message }));
    }
  };

  return (
    <div className="border border-border bg-surface rounded-xl p-5">
      <div className="flex flex-wrap items-center gap-2 mb-3">
        <h3 className="text-section font-semibold">
          {provider?.name ?? t("settings.providers.grokSubName")}
        </h3>
        <span className="px-2 py-0.5 rounded-full text-caption bg-warning-muted text-warning">
          {t("settings.providers.paid")}
        </span>
        <span className={clsx(
          "px-2 py-0.5 rounded-full text-caption font-medium",
          ready
            ? "bg-success-muted text-success"
            : "bg-warning-muted text-warning"
        )}>
          {ready ? t("settings.providers.signedIn") : t("settings.providers.needsSignIn")}
        </span>
      </div>
      <p className="text-body text-text-muted mb-3">
        {t("settings.providers.grokSubHint")}
      </p>
      <label className="block mb-3 text-body text-text-muted">
        {t("settings.providers.model")}
        <input key={model ?? "default"} defaultValue={model || "grok-4.6"}
          onBlur={(e) => { const value = e.target.value.trim(); if (value && value !== (model || "grok-4.6")) onModelChange(value); }}
          className={`mt-1 ${SETTINGS_INPUT_CLASS}`} />
      </label>

      {phase === "pending" && session && (
        <div className="mb-3 p-4 rounded-lg border border-accent-fg bg-accent-muted space-y-3">
          <div className="text-caption font-medium text-accent-fg uppercase">
            {t("settings.providers.userCode")}
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <code className="text-page font-mono tracking-widest font-semibold break-all">
              {session.user_code}
            </code>
            <button
              type="button"
              onClick={() => void copyCode()}
              className="flex items-center gap-1 px-2 py-1 text-caption text-text font-medium rounded-md bg-surface border border-border"
            >
              <Copy size={12} />
              {copied ? t("common.copied") : t("common.copy")}
            </button>
          </div>
          <button
            type="button"
            onClick={() => void openVerification()}
            className="flex items-center gap-1.5 text-body font-medium text-accent-fg hover:underline"
          >
            <ExternalLink size={14} />
            {t("settings.providers.openVerification")}
          </button>
          <p className="text-caption text-accent-fg">
            {t("settings.providers.waitingApproval")}
          </p>
        </div>
      )}

      {phase === "denied" && (
        <p className="mb-3 text-body text-danger">{t("settings.providers.denied")}</p>
      )}
      {phase === "expired" && (
        <p className="mb-3 text-body text-warning">
          {t("settings.providers.expired")}
        </p>
      )}

      <div className="flex items-center gap-3 flex-wrap">
        <button
          type="button"
          onClick={() => void startSignIn()}
          disabled={starting || phase === "pending"}
          className="px-3 py-2 bg-accent hover:bg-accent-hover disabled:opacity-50 text-white rounded-md text-body font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-2 focus-visible:ring-offset-surface"
        >
          {starting
            ? t("settings.providers.startingSignIn")
            : phase === "expired" || phase === "denied"
              ? t("settings.providers.retrySignIn")
              : ready
                ? t("settings.providers.signInAgain")
                : t("settings.providers.signIn")}
        </button>
        {ready && (
          <button
            type="button"
            onClick={onTest}
            className="px-3 py-2 border border-border bg-surface-muted hover:bg-surface text-text rounded-md text-body font-medium"
          >
            {testing ? <Loader size={14} className="animate-spin" /> : t("settings.providers.testConnection")}
          </button>
        )}
        {testResult && (
          <span className={clsx("flex items-center gap-1 text-body break-words", testResult.ok ? "text-success" : "text-danger")}>
            {testResult.ok ? <CheckCircle size={14} /> : <XCircle size={14} />}
            {testResult.ok ? t("settings.providers.connected") : testResult.message.slice(0, 60)}
          </span>
        )}
      </div>
    </div>
  );
}

function useSettingsSave() {
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

function ProvidersSection() {
  const t = useT();
  const { data: providers } = useQuery({ queryKey: ["providers"], queryFn: getProviders });
  const { data: config } = useQuery({ queryKey: ["config"], queryFn: getConfig });
  const saveSettings = useSettingsSave();
  const [testing, setTesting] = useState<Record<string, boolean>>({});
  const [results, setResults] = useState<Record<string, { ok: boolean; message: string }>>({});

  const handleTest = async (id: string) => {
    setTesting((p) => ({ ...p, [id]: true }));
    try {
      const r = await checkProviderHealth(id);
      setResults((p) => ({ ...p, [id]: r }));
    } catch (e: any) {
      setResults((p) => ({ ...p, [id]: { ok: false, message: e.message } }));
    }
    setTesting((p) => ({ ...p, [id]: false }));
  };

  const saveKey = async (providerId: string, key: string, value: string) => {
    await saveSettings({ providers: { [providerId]: { [key]: value } } });
  };

  const grokSub = providers?.find((p) => p.id === GROK_SUB_PROVIDER_ID);

  return (
    <div className="space-y-6">
      <h2 className="text-page font-bold">{t("settings.providers.title")}</h2>
      <GrokSubCard provider={grokSub} model={config?.providers?.[GROK_SUB_PROVIDER_ID]?.model} onModelChange={(model) => void saveKey(GROK_SUB_PROVIDER_ID, "model", model)} onTest={() => handleTest(GROK_SUB_PROVIDER_ID)} testing={!!testing[GROK_SUB_PROVIDER_ID]} testResult={results[GROK_SUB_PROVIDER_ID]} />
      {providers?.filter((p) => p.id !== GROK_SUB_PROVIDER_ID).map((p) => {
        const readiness = resolveProviderReadiness(p.id, providers, config);
        return (
        <div key={p.id} className="border border-border bg-surface rounded-xl p-5">
          <div className="flex flex-wrap items-center gap-2 mb-3">
            <h3 className="text-section font-semibold break-words">{p.name}</h3>
            <span className={clsx("px-2 py-0.5 rounded-full text-caption", p.is_free ? "bg-success-muted text-success" : "bg-warning-muted text-warning")}>
              {p.is_free ? t("settings.providers.free") : t("settings.providers.paid")}
            </span>
            {p.requires_api_key && (
              <span className={clsx(
                "px-2 py-0.5 rounded-full text-caption font-medium",
                readiness.ready
                  ? "bg-success-muted text-success"
                  : "bg-warning-muted text-warning"
              )}>
                {readiness.ready ? t("settings.providers.configured") : t("settings.providers.needsApiKey")}
              </span>
            )}
          </div>

          {p.requires_api_key && (
            <div className="mb-3">
              <label className="text-body text-text-muted">{t("settings.providers.apiKey")}</label>
              <input
                type="password"
                defaultValue={config?.providers?.[p.id]?.api_key === "***" ? "" : config?.providers?.[p.id]?.api_key || ""}
                onBlur={(e) => saveKey(p.id, "api_key", e.target.value)}
                placeholder={t("settings.providers.enterApiKey")}
                className={`mt-1 ${SETTINGS_INPUT_CLASS}`}
              />
            </div>
          )}

          {(p.id === "argos" || p.id === "ollama") && (
            <div className="mb-3">
              <label className="text-body text-text-muted">{t("settings.providers.baseUrl")}</label>
              <input
                defaultValue={config?.providers?.[p.id]?.base_url || (p.id === "argos" ? "http://localhost:5000" : "http://localhost:11434")}
                onBlur={(e) => saveKey(p.id, "base_url", e.target.value)}
                className={`mt-1 ${SETTINGS_INPUT_CLASS}`}
              />
            </div>
          )}

          {p.id === "ollama" && (
            <div className="mb-3">
              <label className="text-body text-text-muted">{t("settings.providers.model")}</label>
              <input
                defaultValue={config?.providers?.[p.id]?.model || "llama3.2"}
                onBlur={(e) => saveKey(p.id, "model", e.target.value)}
                className={`mt-1 ${SETTINGS_INPUT_CLASS}`}
              />
            </div>
          )}

          {p.id === "grok" && (
            <label className="block mb-3 text-body text-text-muted">
              {t("settings.providers.model")}
              <input key={config?.providers?.[p.id]?.model ?? "default"}
                defaultValue={config?.providers?.[p.id]?.model || "grok-4.6"}
                onBlur={(e) => { const value = e.target.value.trim(); if (value && value !== (config?.providers?.[p.id]?.model || "grok-4.6")) void saveKey(p.id, "model", value); }}
                className={`mt-1 ${SETTINGS_INPUT_CLASS}`} />
            </label>
          )}
          {(p.id === "openai" || p.id === "claude") && (
            <div className="mb-3">
              <label className="text-body text-text-muted">{t("settings.providers.model")}</label>
              <select
                defaultValue={config?.providers?.[p.id]?.model || ""}
                onChange={(e) => saveKey(p.id, "model", e.target.value)}
                className={`mt-1 ${SETTINGS_INPUT_CLASS}`}
              >
                {p.id === "openai" && <>
                  <option value="gpt-4o-mini">gpt-4o-mini</option>
                  <option value="gpt-4o">gpt-4o</option>
                  <option value="gpt-4-turbo">gpt-4-turbo</option>
                </>}
                {p.id === "claude" && <>
                  <option value="claude-haiku-4-5-20251001">Haiku</option>
                  <option value="claude-sonnet-4-6">Sonnet</option>
                  <option value="claude-opus-4-6">Opus</option>
                </>}
              </select>
            </div>
          )}

          <div className="flex flex-wrap items-center gap-3">
            <button onClick={() => handleTest(p.id)}
              className="px-3 py-2 border border-border bg-surface-muted hover:bg-surface text-text rounded-md text-body font-medium">
              {testing[p.id] ? <Loader size={14} className="animate-spin" /> : t("settings.providers.testConnection")}
            </button>
            {results[p.id] && (
              <span className={clsx("flex items-center gap-1 text-body break-words", results[p.id].ok ? "text-success" : "text-danger")}>
                {results[p.id].ok ? <CheckCircle size={14} /> : <XCircle size={14} />}
                {results[p.id].ok ? t("settings.providers.connected") : results[p.id].message.slice(0, 60)}
              </span>
            )}
          </div>
        </div>
        );
      })}
    </div>
  );
}

function DefaultsSection() {
  const t = useT();
  const { data: config } = useQuery({ queryKey: ["config"], queryFn: getConfig });
  const { data: providers } = useQuery({ queryKey: ["providers"], queryFn: getProviders });
  const saveSettings = useSettingsSave();

  const save = async (key: string, value: any) => {
    return Boolean(await saveSettings({ [key]: value }));
  };

  if (!config) return null;

  return (
    <div className="space-y-6 border border-border bg-surface rounded-xl p-5">
      <h2 className="text-page font-bold">{t("settings.defaults.title")}</h2>
      <div>
        <label className="text-body font-medium">{t("settings.defaults.provider")}</label>
        <select value={config.default_provider || ""} onChange={(e) => save("default_provider", e.target.value || null)}
          className={`mt-1 ${SETTINGS_INPUT_CLASS}`}>
          <option value="">{t("common.none")}</option>
          {providers?.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
        </select>
      </div>
      <div className="grid grid-cols-1 sm:grid-cols-2 gap-4 [&_label]:block [&_label]:min-h-10">
        <BufferedSetting label={t("settings.defaults.sourceLang")} value={config.default_source_lang} required normalize={value => value.trim()} onSave={value => save("default_source_lang", value)} />
        <BufferedSetting label={t("settings.defaults.targetLang")} value={config.default_target_lang} required normalize={value => value.trim()} onSave={value => save("default_target_lang", value)} />
      </div>
      <p className="text-caption text-text-muted -mt-2">{t("settings.defaults.langHint")}</p>
      <BufferedSetting label={value => t("settings.defaults.batchSize", { size: value })} type="range" min={10} max={100} value={config.default_batch_size} onSave={value => save("default_batch_size", Number(value))} />
      <BufferedSetting label={t("settings.defaults.costLimit")} type="number" min={0} step={0.01} value={config.default_cost_limit ?? ""} placeholder={t("settings.defaults.noLimit")} onSave={value => save("default_cost_limit", value ? Number(value) : null)} />
    </div>
  );
}

function AppearanceSection() {
  const t = useT();
  const { locale, setLocale } = useLocale();
  const { data: config } = useQuery({ queryKey: ["config"], queryFn: getConfig });
  const saveSettings = useSettingsSave();

  const setTheme = async (theme: string) => {
    await saveSettings({ ui: { theme } });
  };

  const setFontSize = async (size: number) => {
    return Boolean(await saveSettings({ ui: { font_size: size } }));
  };

  const setShowSourceColumn = async (show: boolean) => {
    await saveSettings({ ui: { show_source_column: show } });
  };

  const setTableRowHeight = async (height: number) => {
    return Boolean(await saveSettings({ ui: { table_row_height: height } }));
  };

  if (!config) return null;

  return (
    <div className="space-y-6 border border-border bg-surface rounded-xl p-5">
      <h2 className="text-page font-bold">{t("settings.appearance.title")}</h2>
      <div>
        <label className="text-body font-medium">{t("settings.appearance.theme")}</label>
        <div className="flex flex-wrap gap-4 mt-2">
          {(
            [
              ["system", "settings.appearance.theme.system"],
              ["light", "settings.appearance.theme.light"],
              ["dark", "settings.appearance.theme.dark"],
            ] as const
          ).map(([theme, labelKey]) => (
            <label key={theme} className="flex items-center gap-2 cursor-pointer">
              <input type="radio" name="theme" className="accent-accent" checked={config.ui.theme === theme} onChange={() => setTheme(theme)} />
              <span className="text-body">{t(labelKey)}</span>
            </label>
          ))}
        </div>
      </div>
      <BufferedSetting label={value => t("settings.appearance.fontSize", { size: value })} type="range" min={12} max={18} value={config.ui.font_size} onSave={value => setFontSize(Number(value))} />
      <label className="flex items-start gap-2 cursor-pointer">
        <input
          type="checkbox"
          className="mt-0.5 accent-accent"
          checked={config.ui.show_source_column !== false}
          onChange={(e) => setShowSourceColumn(e.target.checked)}
        />
        <span>
          <span className="text-body font-medium">{t("settings.appearance.showSourceColumn")}</span>
          <span className="block text-caption text-text-muted mt-0.5">
            {t("settings.appearance.showSourceColumnHint")}
          </span>
        </span>
      </label>
      <div>
        <BufferedSetting label={value => t("settings.appearance.tableRowHeight", { size: value })} type="range" min={TABLE_ROW_HEIGHT_MIN} max={TABLE_ROW_HEIGHT_MAX} value={clampTableRowHeight(config.ui.table_row_height)} onSave={value => setTableRowHeight(Number(value))} />
        <p className="text-caption text-text-muted mt-0.5">
          {t("settings.appearance.tableRowHeightHint")}
        </p>
      </div>
      <div>
        <label className="text-body font-medium" htmlFor="ui-language">
          {t("settings.appearance.interfaceLanguage")}
        </label>
        <select
          id="ui-language"
          value={locale}
          onChange={(e) => setLocale(e.target.value as Locale)}
          className={`mt-1 ${SETTINGS_INPUT_CLASS}`}
        >
          <option value="en">{t("settings.appearance.locale.en")}</option>
          <option value="es">{t("settings.appearance.locale.es")}</option>
        </select>
      </div>
    </div>
  );
}

function GlossarySection() {
  const t = useT();
  const { data: config } = useQuery({ queryKey: ["config"], queryFn: getConfig });
  const qc = useQueryClient();
  const configPair = config
    ? `${config.default_source_lang}-${config.default_target_lang}`
    : "ja-en";
  const [langPairOverride, setLangPairOverride] = useState<string | null>(null);
  const activePair = (langPairOverride ?? configPair).trim() || "ja-en";

  const { data: entries, refetch, isLoading, isError, error } = useQuery({
    queryKey: ["glossary", activePair],
    queryFn: () => getGlossary(activePair),
    enabled: !!activePair,
  });
  const queryState = isError ? settingsQueryState(error) : null;

  const [filter, setFilter] = useState("");
  const [term, setTerm] = useState("");
  const [translation, setTranslation] = useState("");
  const [saving, setSaving] = useState(false);
  const [entryToDelete, setEntryToDelete] = useState<GlossaryEntry | null>(null);

  const filtered = useMemo(() => {
    const list = entries ?? [];
    const q = filter.trim().toLowerCase();
    if (!q) return list;
    return list.filter(
      (e) =>
        e.term.toLowerCase().includes(q) ||
        e.translation.toLowerCase().includes(q) ||
        (e.context?.toLowerCase().includes(q) ?? false)
    );
  }, [entries, filter]);

  const handleAdd = async () => {
    const termText = term.trim();
    const tr = translation.trim();
    if (!termText || !tr) {
      addToast("error", t("settings.glossary.toast.termRequired"));
      return;
    }
    if (!activePair.trim()) {
      addToast("error", t("settings.glossary.toast.pairRequired"));
      return;
    }
    setSaving(true);
    try {
      const entry: GlossaryEntry = {
        term: termText,
        translation: tr,
        lang_pair: activePair.trim(),
        context: null,
        case_sensitive: false,
      };
      await addGlossaryEntry(entry);
      setTerm("");
      setTranslation("");
      refetch();
      qc.invalidateQueries({ queryKey: ["glossary"] });
    } catch (e: any) {
      addToast("error", t("settings.glossary.toast.addFailed", { error: e.message }));
    } finally {
      setSaving(false);
    }
  };

  const handleDeleteConfirmed = async (entry: GlossaryEntry) => {
    setEntryToDelete(null);
    try {
      await deleteGlossaryEntry(entry.term, entry.lang_pair);
      refetch();
      qc.invalidateQueries({ queryKey: ["glossary"] });
    } catch (e: any) {
      addToast("error", t("settings.glossary.toast.deleteFailed", { error: e.message }));
    }
  };

  return (
    <div className="space-y-6">
      <h2 className="text-page font-bold">{t("settings.glossary.title")}</h2>
      <p className="text-body text-text-muted -mt-4">
        {t("settings.glossary.description")}
      </p>

      <div className="grid grid-cols-1 sm:grid-cols-3 gap-4 items-end border border-border bg-surface rounded-xl p-5">
        <div>
          <label className="text-body font-medium">{t("settings.glossary.langPair")}</label>
          <input
            value={langPairOverride ?? configPair}
            onChange={(e) => setLangPairOverride(e.target.value)}
            placeholder="ja-en"
            className={`mt-1 ${SETTINGS_INPUT_CLASS}`}
          />
        </div>
        <div className="sm:col-span-2 relative">
          <label className="text-body font-medium">{t("settings.glossary.filter")}</label>
          <div className="relative mt-1">
            <Search size={14} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-text-muted" />
            <input
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder={t("settings.glossary.filterPlaceholder")}
              className={`${SETTINGS_INPUT_CLASS} pl-8 pr-3`}
            />
          </div>
        </div>
      </div>

      <div className="border border-border bg-surface rounded-xl p-5 space-y-4">
        <h3 className="text-section font-semibold">{t("settings.glossary.addEntry")}</h3>
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div>
            <label className="text-body text-text-muted">{t("settings.glossary.term")}</label>
            <input
              value={term}
              onChange={(e) => setTerm(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleAdd()}
              placeholder={t("settings.glossary.sourceTerm")}
              className={`mt-1 ${SETTINGS_INPUT_CLASS}`}
            />
          </div>
          <div>
            <label className="text-body text-text-muted">{t("settings.glossary.translation")}</label>
            <input
              value={translation}
              onChange={(e) => setTranslation(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleAdd()}
              placeholder={t("settings.glossary.preferredTranslation")}
              className={`mt-1 ${SETTINGS_INPUT_CLASS}`}
            />
          </div>
        </div>
        <button
          onClick={handleAdd}
          disabled={saving}
          className="flex items-center gap-1.5 px-3 py-2 bg-accent hover:bg-accent-hover disabled:opacity-50 text-white rounded-md text-body font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-2 focus-visible:ring-offset-surface"
        >
          <Plus size={14} /> {saving ? t("settings.glossary.adding") : t("settings.glossary.addEntry")}
        </button>
      </div>

      <div>
        <h3 className="text-section font-semibold mb-3">
          {entries ? t("settings.glossary.entries", { count: filtered.length }) : t("settings.glossary.entriesBare")}
        </h3>
        {isLoading ? (
          <p className="text-body text-text-muted flex items-center gap-2">
            <Loader size={14} className="animate-spin" /> {t("common.loading")}
          </p>
        ) : queryState === "no_project" ? (
          <p role="status" className="text-body text-text-muted">
            {t("settings.glossary.noProject")}
          </p>
        ) : queryState === "failed" ? (
          <p role="alert" className="text-body text-danger">
            {t("settings.glossary.loadFailed", { error: error?.message ?? "" })}
          </p>
        ) : filtered.length === 0 ? (
          <p className="text-body text-text-muted">
            {filter
              ? t("settings.glossary.emptyFilter")
              : t("settings.glossary.empty")}
          </p>
        ) : (
          <div className="overflow-x-auto border border-border bg-surface rounded-xl">
          <table className="w-full min-w-[36rem] text-body">
            <thead>
              <tr className="bg-surface-muted text-left text-text-muted text-caption uppercase tracking-wide">
                <th className="px-3 py-2">{t("settings.glossary.col.term")}</th>
                <th className="px-3 py-2">{t("settings.glossary.col.translation")}</th>
                <th className="px-3 py-2 w-24">{t("settings.glossary.col.pair")}</th>
                <th className="px-3 py-2 w-12"></th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((e) => (
                <tr
                  key={`${e.lang_pair}:${e.term}`}
                  className="border-t border-border hover:bg-surface-muted"
                >
                  <td className="px-3 py-2 font-medium break-words">{e.term}</td>
                  <td className="px-3 py-2 break-words">{e.translation}</td>
                  <td className="px-3 py-2">
                    <span className="px-2 py-0.5 bg-accent-muted text-accent-fg rounded text-caption whitespace-nowrap">
                      {e.lang_pair}
                    </span>
                  </td>
                  <td className="px-3 py-2">
                    <button
                      onClick={() => setEntryToDelete(e)}
                      className="inline-flex items-center justify-center rounded-md p-2 text-danger hover:bg-danger-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-danger"
                      title={t("settings.glossary.deleteTitle")}
                    >
                      <Trash2 size={14} />
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          </div>
        )}
      </div>

      <ConfirmDialog
        open={entryToDelete !== null}
        title={t("settings.glossary.confirm.title")}
        message={entryToDelete ? t("settings.glossary.confirm.message", { term: entryToDelete.term }) : ""}
        confirmLabel={t("common.delete")}
        destructive
        onConfirm={() => entryToDelete && handleDeleteConfirmed(entryToDelete)}
        onCancel={() => setEntryToDelete(null)}
      />
    </div>
  );
}

function DataSection() {
  const t = useT();
  const { data: listing, isPending, isError, error, refetch } = useQuery({
    queryKey: ["backups", "report"], queryFn: getBackupsReport,
  });
  const backups = listing?.entries ?? [];
  const unreadable = listing?.unreadable ?? [];
  const [pendingAction, setPendingAction] = useState<
    { kind: "restore" | "delete"; id: string; game: string; sourcePath: string } | null
  >(null);
  const [restoreResult, setRestoreResult] = useState<{
    id: string; restored: number; kept: BackupRestoreReport["kept"];
  } | null>(null);

  const gameLabel = (sourcePath: string) =>
    gameFolderName(sourcePath) || t("settings.data.unknownGame");

  const runPendingAction = async () => {
    if (!pendingAction) return;
    const { kind, id } = pendingAction;
    setPendingAction(null);
    if (kind === "restore") {
      setRestoreResult(null);
      try {
        const report = await restoreBackup(id);
        // Core writes every inventoried file, including those already identical.
        const restored = report.replaced.length + report.recreated.length + report.identical.length;
        if (report.kept.length > 0) {
          setRestoreResult({ id, restored, kept: report.kept });
        } else {
          addToast("success", t("settings.data.toast.restored", { id, count: restored }));
        }
      } catch (e: any) {
        addToast("error", t("settings.data.toast.restoreFailed", { error: e.message }));
      }
    } else {
      try {
        await deleteBackup(id);
        refetch();
      } catch (e: any) {
        addToast("error", t("settings.data.toast.deleteFailed", { error: e.message }));
      }
    }
  };

  return (
    <div className="space-y-6">
      <h2 className="text-page font-bold">{t("settings.data.title")}</h2>
      {restoreResult && (
        <div role="alert" className="p-4 border border-warning bg-warning-muted text-warning rounded-xl space-y-2">
          <p className="font-semibold">
            {t("settings.data.toast.restored", { id: restoreResult.id, count: restoreResult.restored })}
          </p>
          <p>{t("settings.data.restoreKept")}</p>
          <ul className="space-y-2 text-body">
            {restoreResult.kept.slice(0, 10).map(({ path, reason }) => (
              <li key={path}>
                <div className="font-mono text-caption break-all">{path}</div>
                <div className="text-caption break-words">{reason}</div>
              </li>
            ))}
          </ul>
          {restoreResult.kept.length > 10 && (
            <p className="text-caption">{t("settings.data.restoreKeptMore", { count: restoreResult.kept.length - 10 })}</p>
          )}
        </div>
      )}
      <div>
        <h3 className="text-section font-semibold mb-3">{t("settings.data.backups")}</h3>
        {isPending ? (
          <p className="text-body text-text-muted" role="status">{t("common.loading")}</p>
        ) : isError ? (
          <p className="text-body text-danger" role="alert">
            {t("settings.data.loadFailed", { error: error.message })}
          </p>
        ) : backups.length === 0 && unreadable.length === 0 ? (
          <p className="text-body text-text-muted">{t("settings.data.noBackups")}</p>
        ) : backups.length > 0 ? (
          <div className="overflow-x-auto border border-border bg-surface rounded-xl">
          <table className="w-full min-w-[48rem] text-body">
            <thead>
              <tr className="bg-surface-muted text-left text-text-muted text-caption uppercase tracking-wide">
                <th className="px-3 py-2">{t("settings.data.col.game")}</th>
                <th className="px-3 py-2">{t("settings.data.col.id")}</th>
                <th className="px-3 py-2">{t("settings.data.col.created")}</th>
                <th className="px-3 py-2">{t("settings.data.col.files")}</th>
                <th className="px-3 py-2">{t("settings.data.col.actions")}</th>
              </tr>
            </thead>
            <tbody>
              {backups.map((b) => {
                const game = gameLabel(b.source_path);
                const pending = { id: b.id, game, sourcePath: b.source_path };
                return (
                  <tr key={b.id} className="border-t border-border hover:bg-surface-muted">
                    <td className="px-3 py-2 max-w-[24rem]" title={b.source_path || undefined}>
                      <div className="font-medium break-words">{game}</div>
                      {b.source_path ? (
                        <div className="text-caption text-text-muted break-all">{b.source_path}</div>
                      ) : null}
                    </td>
                    <td className="px-3 py-2 font-mono text-caption">{b.id}</td>
                    <td className="px-3 py-2 whitespace-nowrap">{new Date(b.created_at).toLocaleString()}</td>
                    <td className="px-3 py-2 tabular-nums">{b.file_count}</td>
                    <td className="px-3 py-2 flex gap-2">
                      <button onClick={() => setPendingAction({ kind: "restore", ...pending })} className="inline-flex items-center justify-center rounded-md p-2 text-accent-fg hover:bg-accent-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg" title={t("settings.data.restore")}><RotateCcw size={14} /></button>
                      <button onClick={() => setPendingAction({ kind: "delete", ...pending })} className="inline-flex items-center justify-center rounded-md p-2 text-danger hover:bg-danger-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-danger" title={t("settings.data.delete")}><Trash2 size={14} /></button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
          </div>
        ) : null}
        {!isPending && !isError && unreadable.length > 0 && (
          <div className="mt-4 p-4 border border-warning bg-warning-muted text-warning rounded-xl space-y-2">
            <h4 className="text-section font-semibold">{t("settings.data.damagedBackups")}</h4>
            <p className="text-caption">{t("settings.data.damagedHint")}</p>
            <ul className="space-y-2 text-body">
              {unreadable.map((backup) => {
                const diagnostic = backup.error.replace(/\s+/g, " ").trim();
                const shortError = diagnostic.length > 200 ? `${diagnostic.slice(0, 200)}…` : diagnostic;
                return (
                  <li key={backup.id}>
                    <div className="font-mono text-caption break-all">{backup.id}</div>
                    <div className="text-caption break-words">{shortError}</div>
                  </li>
                );
              })}
            </ul>
          </div>
        )}
      </div>

      <ConfirmDialog
        open={pendingAction !== null}
        title={pendingAction?.kind === "restore" ? t("settings.data.confirm.restoreTitle") : t("settings.data.confirm.deleteTitle")}
        message={
          pendingAction?.kind === "restore"
            ? t("settings.data.confirm.restoreMessage", {
                id: pendingAction.id,
                game: pendingAction.game,
                path: pendingAction.sourcePath || pendingAction.game,
              })
            : t("settings.data.confirm.deleteMessage", { id: pendingAction?.id ?? "" })
        }
        confirmLabel={pendingAction?.kind === "restore" ? t("settings.data.restore") : t("common.delete")}
        destructive
        onConfirm={runPendingAction}
        onCancel={() => setPendingAction(null)}
      />
    </div>
  );
}
