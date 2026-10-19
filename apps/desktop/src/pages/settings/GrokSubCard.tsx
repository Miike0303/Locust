import { useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { CheckCircle, Copy, ExternalLink, Loader, XCircle } from "lucide-react";
import { useEffect, useState } from "react";
import { SETTINGS_INPUT_CLASS } from "../../components/BufferedSetting";
import type { ProviderInfo } from "../../lib/api";
import { xaiAuthPoll, xaiAuthStart } from "../../lib/api";
import { useT } from "../../lib/i18n";
import { IS_TAURI } from "../../lib/runtime";
import {
  XAI_POLL_INTERVAL_MS,
  grokSubIsReady,
  nextXaiPollAction,
  type XaiAuthPollStatus,
} from "../../lib/xaiAuth";
import { addToast } from "../../stores/toastStore";

async function openExternalUrl(url: string): Promise<void> {
  if (IS_TAURI) {
    const { open } = await import("@tauri-apps/plugin-shell");
    await open(url);
    return;
  }
  window.open(url, "_blank", "noopener,noreferrer");
}

export default function GrokSubCard({
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
