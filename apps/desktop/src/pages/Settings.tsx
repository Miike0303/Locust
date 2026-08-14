import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useLocation, useNavigate } from "react-router-dom";
import { CheckCircle, XCircle, Loader, Trash2, RotateCcw, Plus, Search, Copy, ExternalLink } from "lucide-react";
import clsx from "clsx";
import {
  getProviders, checkProviderHealth, getConfig, updateConfig,
  getBackups, restoreBackup, deleteBackup,
  getGlossary, addGlossaryEntry, deleteGlossaryEntry,
  getTranslationRuns,
  xaiAuthStart, xaiAuthPoll,
} from "../lib/api";
import type { GlossaryEntry, TranslationRun, ProviderInfo } from "../lib/api";
import { applyAppearance, clampTableRowHeight, TABLE_ROW_HEIGHT_MAX, TABLE_ROW_HEIGHT_MIN } from "../lib/appearance";
import { resolveProviderReadiness } from "../lib/providerReadiness";
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
import ConfirmDialog from "../components/ConfirmDialog";
import { addToast } from "../stores/toastStore";
import { useLocale, useT, type Locale } from "../lib/i18n";

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
    <div className="flex h-full">
      <nav className="w-48 border-r border-gray-200 dark:border-gray-700 p-4 space-y-1">
        {SETTINGS_SECTIONS.map(({ id }) => (
          <button
            key={id}
            onClick={() => selectSection(id)}
            aria-current={section === id ? "page" : undefined}
            className={clsx(
              "block w-full text-left px-3 py-2 rounded text-sm font-medium",
              section === id
                ? "bg-emerald-100 text-emerald-700 dark:bg-emerald-900 dark:text-emerald-300"
                : "text-gray-600 hover:bg-gray-100 dark:text-gray-400 dark:hover:bg-gray-800"
            )}
          >
            {t(`settings.nav.${id}`)}
          </button>
        ))}
      </nav>
      <div className="flex-1 p-6 overflow-y-auto">
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

function formatDuration(secs: number, t: (key: string, vars?: Record<string, string | number>) => string): string {
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

  const totals = useMemo(() => {
    const list = runs ?? [];
    return list.reduce(
      (acc, r) => {
        acc.strings += r.strings_translated;
        acc.tokens += r.tokens_used;
        acc.input += r.input_tokens;
        acc.output += r.output_tokens;
        acc.cost += r.cost_usd;
        acc.secs += r.duration_secs;
        return acc;
      },
      { strings: 0, tokens: 0, input: 0, output: 0, cost: 0, secs: 0 }
    );
  }, [runs]);

  return (
    <div className="space-y-4 max-w-5xl">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h2 className="text-xl font-bold">{t("settings.history.title")}</h2>
          <p className="text-sm text-gray-500 mt-1">
            {t("settings.history.description")}
          </p>
        </div>
        <button
          type="button"
          onClick={() => refetch()}
          className="text-sm text-emerald-700 hover:underline dark:text-emerald-400"
        >
          {t("common.refresh")}
        </button>
      </div>

      {isLoading && (
        <div className="flex items-center gap-2 text-sm text-gray-500">
          <Loader size={16} className="animate-spin" /> {t("settings.history.loading")}
        </div>
      )}

      {isError && (
        <div className="text-sm text-red-600 dark:text-red-400">
          {t("settings.history.loadFailed", {
            error: (error as Error)?.message ?? t("settings.history.unknownError"),
          })}
        </div>
      )}

      {!isLoading && !isError && (runs?.length ?? 0) === 0 && (
        <div className="border border-dashed border-gray-300 dark:border-gray-600 rounded-lg p-8 text-center text-sm text-gray-500">
          {t("settings.history.empty")}
        </div>
      )}

      {!isLoading && !isError && (runs?.length ?? 0) > 0 && (
        <div className="overflow-x-auto border border-gray-200 dark:border-gray-700 rounded-lg">
          <table className="w-full text-sm">
            <thead>
              <tr className="bg-gray-50 dark:bg-gray-800/80 text-left text-xs font-semibold text-gray-500 uppercase tracking-wide">
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
            <tbody className="divide-y divide-gray-100 dark:divide-gray-800">
              {runs!.map((r: TranslationRun) => (
                <tr key={r.id} className="hover:bg-gray-50/80 dark:hover:bg-gray-800/40">
                  <td className="px-3 py-2 whitespace-nowrap text-gray-700 dark:text-gray-300">
                    {formatRunDate(r.started_at)}
                  </td>
                  <td className="px-3 py-2 font-mono text-xs" title={r.provider}>
                    {r.provider}
                  </td>
                  <td className="px-3 py-2 whitespace-nowrap">
                    {r.source_lang}→{r.target_lang}
                  </td>
                  <td className="px-3 py-2 text-right tabular-nums">{r.strings_translated}</td>
                  <td className="px-3 py-2 text-right tabular-nums">{r.tokens_used}</td>
                  <td className="px-3 py-2 text-right tabular-nums text-gray-500">
                    {r.input_tokens}
                  </td>
                  <td className="px-3 py-2 text-right tabular-nums text-gray-500">
                    {r.output_tokens}
                  </td>
                  <td className="px-3 py-2 text-right tabular-nums">
                    {r.cost_usd.toFixed(4)}
                  </td>
                  <td className="px-3 py-2 text-right tabular-nums whitespace-nowrap">
                    {formatDuration(r.duration_secs, t)}
                  </td>
                </tr>
              ))}
            </tbody>
            <tfoot>
              <tr className="bg-gray-50 dark:bg-gray-800/80 font-semibold text-gray-800 dark:text-gray-200 border-t border-gray-200 dark:border-gray-700">
                <td className="px-3 py-2" colSpan={3}>
                  {t("settings.history.totalRuns", { count: runs!.length })}
                </td>
                <td className="px-3 py-2 text-right tabular-nums">{totals.strings}</td>
                <td className="px-3 py-2 text-right tabular-nums">{totals.tokens}</td>
                <td className="px-3 py-2 text-right tabular-nums text-gray-500">
                  {totals.input}
                </td>
                <td className="px-3 py-2 text-right tabular-nums text-gray-500">
                  {totals.output}
                </td>
                <td className="px-3 py-2 text-right tabular-nums">
                  {totals.cost.toFixed(4)}
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

const IS_TAURI = "__TAURI_INTERNALS__" in window;

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
  onTest,
  testing,
  testResult,
}: {
  provider?: ProviderInfo;
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
    <div className="border border-gray-200 dark:border-gray-700 rounded-lg p-4">
      <div className="flex items-center gap-2 mb-2">
        <h3 className="font-semibold">
          {provider?.name ?? t("settings.providers.grokSubName")}
        </h3>
        <span className="px-2 py-0.5 rounded-full text-xs bg-amber-100 text-amber-700">
          {t("settings.providers.paid")}
        </span>
        <span className={clsx(
          "px-2 py-0.5 rounded-full text-xs font-medium",
          ready
            ? "bg-emerald-100 text-emerald-700 dark:bg-emerald-900/40 dark:text-emerald-300"
            : "bg-amber-100 text-amber-700 dark:bg-amber-900/40 dark:text-amber-300"
        )}>
          {ready ? t("settings.providers.signedIn") : t("settings.providers.needsSignIn")}
        </span>
      </div>
      <p className="text-sm text-gray-500 mb-3">
        {t("settings.providers.grokSubHint")}
      </p>

      {phase === "pending" && session && (
        <div className="mb-3 p-3 rounded border border-emerald-200 bg-emerald-50 dark:border-emerald-800 dark:bg-emerald-950/30 space-y-2">
          <div className="text-xs font-medium text-gray-500 uppercase">
            {t("settings.providers.userCode")}
          </div>
          <div className="flex items-center gap-2">
            <code className="text-2xl font-mono tracking-widest font-semibold">
              {session.user_code}
            </code>
            <button
              type="button"
              onClick={() => void copyCode()}
              className="flex items-center gap-1 px-2 py-1 text-xs font-medium rounded bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-600"
            >
              <Copy size={12} />
              {copied ? t("common.copied") : t("common.copy")}
            </button>
          </div>
          <button
            type="button"
            onClick={() => void openVerification()}
            className="flex items-center gap-1.5 text-sm font-medium text-emerald-700 dark:text-emerald-400 hover:underline"
          >
            <ExternalLink size={14} />
            {t("settings.providers.openVerification")}
          </button>
          <p className="text-xs text-gray-500">
            {t("settings.providers.waitingApproval")}
          </p>
        </div>
      )}

      {phase === "denied" && (
        <p className="mb-3 text-sm text-red-600">{t("settings.providers.denied")}</p>
      )}
      {phase === "expired" && (
        <p className="mb-3 text-sm text-amber-700 dark:text-amber-300">
          {t("settings.providers.expired")}
        </p>
      )}

      <div className="flex items-center gap-3 flex-wrap">
        <button
          type="button"
          onClick={() => void startSignIn()}
          disabled={starting || phase === "pending"}
          className="px-3 py-1.5 bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50 text-white rounded text-sm font-medium"
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
            className="px-3 py-1.5 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded text-sm font-medium"
          >
            {testing ? <Loader size={14} className="animate-spin" /> : t("settings.providers.testConnection")}
          </button>
        )}
        {testResult && (
          <span className={clsx("flex items-center gap-1 text-sm", testResult.ok ? "text-green-600" : "text-red-600")}>
            {testResult.ok ? <CheckCircle size={14} /> : <XCircle size={14} />}
            {testResult.ok ? t("settings.providers.connected") : testResult.message.slice(0, 60)}
          </span>
        )}
      </div>
    </div>
  );
}

function ProvidersSection() {
  const t = useT();
  const { data: providers } = useQuery({ queryKey: ["providers"], queryFn: getProviders });
  const { data: config } = useQuery({ queryKey: ["config"], queryFn: getConfig });
  const qc = useQueryClient();
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
    const providers = { ...config?.providers, [providerId]: { ...config?.providers?.[providerId], [key]: value } };
    await updateConfig({ providers } as any);
    qc.invalidateQueries({ queryKey: ["config"] });
  };

  const grokSub = providers?.find((p) => p.id === GROK_SUB_PROVIDER_ID);

  return (
    <div className="space-y-4">
      <h2 className="text-xl font-bold">{t("settings.providers.title")}</h2>
      <GrokSubCard provider={grokSub} onTest={() => handleTest(GROK_SUB_PROVIDER_ID)} testing={!!testing[GROK_SUB_PROVIDER_ID]} testResult={results[GROK_SUB_PROVIDER_ID]} />
      {providers?.filter((p) => p.id !== GROK_SUB_PROVIDER_ID).map((p) => {
        const readiness = resolveProviderReadiness(p.id, providers, config);
        return (
        <div key={p.id} className="border border-gray-200 dark:border-gray-700 rounded-lg p-4">
          <div className="flex items-center gap-2 mb-3">
            <h3 className="font-semibold">{p.name}</h3>
            <span className={clsx("px-2 py-0.5 rounded-full text-xs", p.is_free ? "bg-green-100 text-green-700" : "bg-amber-100 text-amber-700")}>
              {p.is_free ? t("settings.providers.free") : t("settings.providers.paid")}
            </span>
            {p.requires_api_key && (
              <span className={clsx(
                "px-2 py-0.5 rounded-full text-xs font-medium",
                readiness.ready
                  ? "bg-emerald-100 text-emerald-700 dark:bg-emerald-900/40 dark:text-emerald-300"
                  : "bg-amber-100 text-amber-700 dark:bg-amber-900/40 dark:text-amber-300"
              )}>
                {readiness.ready ? t("settings.providers.configured") : t("settings.providers.needsApiKey")}
              </span>
            )}
          </div>

          {p.requires_api_key && (
            <div className="mb-3">
              <label className="text-sm text-gray-600">{t("settings.providers.apiKey")}</label>
              <input
                type="password"
                defaultValue={config?.providers?.[p.id]?.api_key === "***" ? "" : config?.providers?.[p.id]?.api_key || ""}
                onBlur={(e) => saveKey(p.id, "api_key", e.target.value)}
                placeholder={t("settings.providers.enterApiKey")}
                className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600"
              />
            </div>
          )}

          {(p.id === "argos" || p.id === "ollama") && (
            <div className="mb-3">
              <label className="text-sm text-gray-600">{t("settings.providers.baseUrl")}</label>
              <input
                defaultValue={config?.providers?.[p.id]?.base_url || (p.id === "argos" ? "http://localhost:5000" : "http://localhost:11434")}
                onBlur={(e) => saveKey(p.id, "base_url", e.target.value)}
                className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600"
              />
            </div>
          )}

          {p.id === "ollama" && (
            <div className="mb-3">
              <label className="text-sm text-gray-600">{t("settings.providers.model")}</label>
              <input
                defaultValue={config?.providers?.[p.id]?.model || "llama3.2"}
                onBlur={(e) => saveKey(p.id, "model", e.target.value)}
                className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600"
              />
            </div>
          )}

          {(p.id === "openai" || p.id === "claude") && (
            <div className="mb-3">
              <label className="text-sm text-gray-600">{t("settings.providers.model")}</label>
              <select
                defaultValue={config?.providers?.[p.id]?.model || ""}
                onChange={(e) => saveKey(p.id, "model", e.target.value)}
                className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600"
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

          <div className="flex items-center gap-3">
            <button onClick={() => handleTest(p.id)}
              className="px-3 py-1.5 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded text-sm font-medium">
              {testing[p.id] ? <Loader size={14} className="animate-spin" /> : t("settings.providers.testConnection")}
            </button>
            {results[p.id] && (
              <span className={clsx("flex items-center gap-1 text-sm", results[p.id].ok ? "text-green-600" : "text-red-600")}>
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
  const qc = useQueryClient();

  const save = async (key: string, value: any) => {
    await updateConfig({ [key]: value } as any);
    qc.invalidateQueries({ queryKey: ["config"] });
  };

  if (!config) return null;

  return (
    <div className="space-y-6 max-w-md">
      <h2 className="text-xl font-bold">{t("settings.defaults.title")}</h2>
      <div>
        <label className="text-sm font-medium">{t("settings.defaults.provider")}</label>
        <select value={config.default_provider || ""} onChange={(e) => save("default_provider", e.target.value || null)}
          className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600">
          <option value="">{t("common.none")}</option>
          {providers?.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
        </select>
      </div>
      <div className="grid grid-cols-2 gap-4">
        <div>
          <label className="text-sm font-medium">{t("settings.defaults.sourceLang")}</label>
          <input value={config.default_source_lang} onChange={(e) => save("default_source_lang", e.target.value)}
            className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600" />
        </div>
        <div>
          <label className="text-sm font-medium">{t("settings.defaults.targetLang")}</label>
          <input value={config.default_target_lang} onChange={(e) => save("default_target_lang", e.target.value)}
            className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600" />
        </div>
      </div>
      <p className="text-xs text-gray-500 -mt-2">{t("settings.defaults.langHint")}</p>
      <div>
        <label className="text-sm font-medium">{t("settings.defaults.batchSize", { size: config.default_batch_size })}</label>
        <input type="range" min={10} max={100} value={config.default_batch_size}
          onChange={(e) => save("default_batch_size", +e.target.value)}
          className="mt-1 w-full" />
      </div>
      <div>
        <label className="text-sm font-medium">{t("settings.defaults.costLimit")}</label>
        <input type="number" step="0.01" value={config.default_cost_limit ?? ""}
          onChange={(e) => save("default_cost_limit", e.target.value ? +e.target.value : null)}
          placeholder={t("settings.defaults.noLimit")}
          className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600" />
      </div>
    </div>
  );
}

function AppearanceSection() {
  const t = useT();
  const { locale, setLocale } = useLocale();
  const { data: config } = useQuery({ queryKey: ["config"], queryFn: getConfig });
  const qc = useQueryClient();

  const setTheme = async (theme: string) => {
    const ui = { ...config?.ui, theme };
    await updateConfig({ ui } as any);
    qc.invalidateQueries({ queryKey: ["config"] });
    applyAppearance(ui);
  };

  const setFontSize = async (size: number) => {
    const ui = { ...config?.ui, font_size: size };
    await updateConfig({ ui } as any);
    qc.invalidateQueries({ queryKey: ["config"] });
    applyAppearance(ui);
  };

  const setShowSourceColumn = async (show: boolean) => {
    const ui = { ...config?.ui, show_source_column: show };
    await updateConfig({ ui } as any);
    qc.invalidateQueries({ queryKey: ["config"] });
  };

  const setTableRowHeight = async (height: number) => {
    const ui = { ...config?.ui, table_row_height: height };
    await updateConfig({ ui } as any);
    qc.invalidateQueries({ queryKey: ["config"] });
  };

  if (!config) return null;

  return (
    <div className="space-y-6 max-w-md">
      <h2 className="text-xl font-bold">{t("settings.appearance.title")}</h2>
      <div>
        <label className="text-sm font-medium">{t("settings.appearance.theme")}</label>
        <div className="flex gap-3 mt-2">
          {(["system", "light", "dark"] as const).map((theme) => (
            <label key={theme} className="flex items-center gap-2 cursor-pointer">
              <input type="radio" name="theme" checked={config.ui.theme === theme} onChange={() => setTheme(theme)} />
              <span className="text-sm">{t(`settings.appearance.theme.${theme}`)}</span>
            </label>
          ))}
        </div>
      </div>
      <div>
        <label className="text-sm font-medium">{t("settings.appearance.fontSize", { size: config.ui.font_size })}</label>
        <input type="range" min={12} max={18} value={config.ui.font_size}
          onChange={(e) => setFontSize(+e.target.value)}
          className="mt-1 w-full" />
      </div>
      <label className="flex items-start gap-2 cursor-pointer">
        <input
          type="checkbox"
          className="mt-0.5"
          checked={config.ui.show_source_column !== false}
          onChange={(e) => setShowSourceColumn(e.target.checked)}
        />
        <span>
          <span className="text-sm font-medium">{t("settings.appearance.showSourceColumn")}</span>
          <span className="block text-xs text-gray-500 mt-0.5">
            {t("settings.appearance.showSourceColumnHint")}
          </span>
        </span>
      </label>
      <div>
        <label className="text-sm font-medium">
          {t("settings.appearance.tableRowHeight", {
            size: clampTableRowHeight(config.ui.table_row_height),
          })}
        </label>
        <input
          type="range"
          min={TABLE_ROW_HEIGHT_MIN}
          max={TABLE_ROW_HEIGHT_MAX}
          value={clampTableRowHeight(config.ui.table_row_height)}
          onChange={(e) => setTableRowHeight(+e.target.value)}
          className="mt-1 w-full"
        />
        <p className="text-xs text-gray-500 mt-0.5">
          {t("settings.appearance.tableRowHeightHint")}
        </p>
      </div>
      <div>
        <label className="text-sm font-medium" htmlFor="ui-language">
          {t("settings.appearance.interfaceLanguage")}
        </label>
        <select
          id="ui-language"
          value={locale}
          onChange={(e) => setLocale(e.target.value as Locale)}
          className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600"
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

  const { data: entries, refetch, isLoading } = useQuery({
    queryKey: ["glossary", activePair],
    queryFn: () => getGlossary(activePair),
    enabled: !!activePair,
  });

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
    <div className="space-y-6 max-w-3xl">
      <h2 className="text-xl font-bold">{t("settings.glossary.title")}</h2>
      <p className="text-sm text-gray-500">
        {t("settings.glossary.description")}
      </p>

      <div className="grid grid-cols-1 sm:grid-cols-3 gap-3 items-end">
        <div>
          <label className="text-sm font-medium">{t("settings.glossary.langPair")}</label>
          <input
            value={langPairOverride ?? configPair}
            onChange={(e) => setLangPairOverride(e.target.value)}
            placeholder="ja-en"
            className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600"
          />
        </div>
        <div className="sm:col-span-2 relative">
          <label className="text-sm font-medium">{t("settings.glossary.filter")}</label>
          <div className="relative mt-1">
            <Search size={14} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-gray-400" />
            <input
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder={t("settings.glossary.filterPlaceholder")}
              className="w-full pl-8 pr-3 py-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600"
            />
          </div>
        </div>
      </div>

      <div className="border border-gray-200 dark:border-gray-700 rounded-lg p-4 space-y-3">
        <h3 className="text-sm font-semibold text-gray-500 uppercase">{t("settings.glossary.addEntry")}</h3>
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div>
            <label className="text-sm text-gray-600">{t("settings.glossary.term")}</label>
            <input
              value={term}
              onChange={(e) => setTerm(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleAdd()}
              placeholder={t("settings.glossary.sourceTerm")}
              className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600"
            />
          </div>
          <div>
            <label className="text-sm text-gray-600">{t("settings.glossary.translation")}</label>
            <input
              value={translation}
              onChange={(e) => setTranslation(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleAdd()}
              placeholder={t("settings.glossary.preferredTranslation")}
              className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600"
            />
          </div>
        </div>
        <button
          onClick={handleAdd}
          disabled={saving}
          className="flex items-center gap-1.5 px-3 py-1.5 bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50 text-white rounded text-sm font-medium"
        >
          <Plus size={14} /> {saving ? t("settings.glossary.adding") : t("settings.glossary.addEntry")}
        </button>
      </div>

      <div>
        <h3 className="text-sm font-semibold text-gray-500 uppercase mb-2">
          {entries ? t("settings.glossary.entries", { count: filtered.length }) : t("settings.glossary.entriesBare")}
        </h3>
        {isLoading ? (
          <p className="text-sm text-gray-500 flex items-center gap-2">
            <Loader size={14} className="animate-spin" /> {t("common.loading")}
          </p>
        ) : filtered.length === 0 ? (
          <p className="text-sm text-gray-500">
            {filter
              ? t("settings.glossary.emptyFilter")
              : t("settings.glossary.empty")}
          </p>
        ) : (
          <table className="w-full text-sm">
            <thead>
              <tr className="text-left text-gray-500 text-xs uppercase">
                <th className="pb-2">{t("settings.glossary.col.term")}</th>
                <th className="pb-2">{t("settings.glossary.col.translation")}</th>
                <th className="pb-2 w-24">{t("settings.glossary.col.pair")}</th>
                <th className="pb-2 w-12"></th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((e) => (
                <tr
                  key={`${e.lang_pair}:${e.term}`}
                  className="border-t border-gray-100 dark:border-gray-800"
                >
                  <td className="py-2 font-medium">{e.term}</td>
                  <td className="py-2">{e.translation}</td>
                  <td className="py-2">
                    <span className="px-2 py-0.5 bg-blue-100 dark:bg-blue-900/40 text-blue-700 dark:text-blue-300 rounded text-xs">
                      {e.lang_pair}
                    </span>
                  </td>
                  <td className="py-2">
                    <button
                      onClick={() => setEntryToDelete(e)}
                      className="text-red-500 hover:text-red-700"
                      title={t("settings.glossary.deleteTitle")}
                    >
                      <Trash2 size={14} />
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
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
  const { data: backups, refetch } = useQuery({ queryKey: ["backups"], queryFn: getBackups });
  const [pendingAction, setPendingAction] = useState<
    { kind: "restore" | "delete"; id: string } | null
  >(null);

  const runPendingAction = async () => {
    if (!pendingAction) return;
    const { kind, id } = pendingAction;
    setPendingAction(null);
    if (kind === "restore") {
      try {
        await restoreBackup(id);
        addToast("success", t("settings.data.toast.restored", { id }));
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
      <h2 className="text-xl font-bold">{t("settings.data.title")}</h2>
      <div>
        <h3 className="text-sm font-semibold text-gray-500 uppercase mb-2">{t("settings.data.backups")}</h3>
        {(!backups || backups.length === 0) ? (
          <p className="text-sm text-gray-500">{t("settings.data.noBackups")}</p>
        ) : (
          <table className="w-full text-sm">
            <thead>
              <tr className="text-left text-gray-500">
                <th className="pb-2">{t("settings.data.col.id")}</th><th className="pb-2">{t("settings.data.col.created")}</th><th className="pb-2">{t("settings.data.col.files")}</th><th className="pb-2">{t("settings.data.col.actions")}</th>
              </tr>
            </thead>
            <tbody>
              {backups.map((b) => (
                <tr key={b.id} className="border-t border-gray-100 dark:border-gray-800">
                  <td className="py-2 font-mono text-xs">{b.id}</td>
                  <td className="py-2">{new Date(b.created_at).toLocaleString()}</td>
                  <td className="py-2">{b.file_count}</td>
                  <td className="py-2 flex gap-2">
                    <button onClick={() => setPendingAction({ kind: "restore", id: b.id })} className="text-emerald-600 hover:text-emerald-800" title={t("settings.data.restore")}><RotateCcw size={14} /></button>
                    <button onClick={() => setPendingAction({ kind: "delete", id: b.id })} className="text-red-500 hover:text-red-700" title={t("settings.data.delete")}><Trash2 size={14} /></button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <ConfirmDialog
        open={pendingAction !== null}
        title={pendingAction?.kind === "restore" ? t("settings.data.confirm.restoreTitle") : t("settings.data.confirm.deleteTitle")}
        message={
          pendingAction?.kind === "restore"
            ? t("settings.data.confirm.restoreMessage", { id: pendingAction.id })
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
