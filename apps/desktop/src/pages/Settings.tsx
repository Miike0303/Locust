import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useLocation, useNavigate } from "react-router-dom";
import { CheckCircle, XCircle, Loader, Trash2, RotateCcw, Plus, Search } from "lucide-react";
import clsx from "clsx";
import {
  getProviders, checkProviderHealth, getConfig, updateConfig,
  getBackups, restoreBackup, deleteBackup,
  getGlossary, addGlossaryEntry, deleteGlossaryEntry,
  getTranslationRuns,
} from "../lib/api";
import type { GlossaryEntry, TranslationRun } from "../lib/api";
import { applyAppearance } from "../lib/appearance";
import {
  SETTINGS_SECTIONS,
  buildSettingsPath,
  parseSettingsSectionParam,
  type SettingsSectionId,
} from "../lib/settingsNav";
import ConfirmDialog from "../components/ConfirmDialog";
import { addToast } from "../stores/toastStore";

export default function Settings() {
  const location = useLocation();
  const navigate = useNavigate();
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
        {SETTINGS_SECTIONS.map(({ id, label }) => (
          <button
            key={id}
            onClick={() => selectSection(id)}
            className={clsx(
              "block w-full text-left px-3 py-2 rounded text-sm font-medium",
              section === id
                ? "bg-emerald-100 text-emerald-700 dark:bg-emerald-900 dark:text-emerald-300"
                : "text-gray-600 hover:bg-gray-100 dark:text-gray-400 dark:hover:bg-gray-800"
            )}
          >
            {label}
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

function formatDuration(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  if (s >= 3600) return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
  if (s >= 60) return `${Math.floor(s / 60)}m ${s % 60}s`;
  return `${s}s`;
}

function HistorySection() {
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
          <h2 className="text-xl font-bold">Translation history</h2>
          <p className="text-sm text-gray-500 mt-1">
            Every translation run recorded for this project — provider, cost, tokens and duration.
          </p>
        </div>
        <button
          type="button"
          onClick={() => refetch()}
          className="text-sm text-emerald-700 hover:underline dark:text-emerald-400"
        >
          Refresh
        </button>
      </div>

      {isLoading && (
        <div className="flex items-center gap-2 text-sm text-gray-500">
          <Loader size={16} className="animate-spin" /> Loading runs…
        </div>
      )}

      {isError && (
        <div className="text-sm text-red-600 dark:text-red-400">
          Failed to load runs: {(error as Error)?.message ?? "unknown error"}
        </div>
      )}

      {!isLoading && !isError && (runs?.length ?? 0) === 0 && (
        <div className="border border-dashed border-gray-300 dark:border-gray-600 rounded-lg p-8 text-center text-sm text-gray-500">
          No translation runs recorded yet. Run a translation from the Editor to populate this
          history.
        </div>
      )}

      {!isLoading && !isError && (runs?.length ?? 0) > 0 && (
        <div className="overflow-x-auto border border-gray-200 dark:border-gray-700 rounded-lg">
          <table className="w-full text-sm">
            <thead>
              <tr className="bg-gray-50 dark:bg-gray-800/80 text-left text-xs font-semibold text-gray-500 uppercase tracking-wide">
                <th className="px-3 py-2">Date</th>
                <th className="px-3 py-2">Provider</th>
                <th className="px-3 py-2">Langs</th>
                <th className="px-3 py-2 text-right">Strings</th>
                <th className="px-3 py-2 text-right">Tokens</th>
                <th className="px-3 py-2 text-right">In</th>
                <th className="px-3 py-2 text-right">Out</th>
                <th className="px-3 py-2 text-right">Cost (USD)</th>
                <th className="px-3 py-2 text-right">Duration</th>
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
                    {formatDuration(r.duration_secs)}
                  </td>
                </tr>
              ))}
            </tbody>
            <tfoot>
              <tr className="bg-gray-50 dark:bg-gray-800/80 font-semibold text-gray-800 dark:text-gray-200 border-t border-gray-200 dark:border-gray-700">
                <td className="px-3 py-2" colSpan={3}>
                  Total ({runs!.length} run{runs!.length === 1 ? "" : "s"})
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
                  {formatDuration(totals.secs)}
                </td>
              </tr>
            </tfoot>
          </table>
        </div>
      )}
    </div>
  );
}

function ProvidersSection() {
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

  return (
    <div className="space-y-4">
      <h2 className="text-xl font-bold">Providers</h2>
      {providers?.map((p) => (
        <div key={p.id} className="border border-gray-200 dark:border-gray-700 rounded-lg p-4">
          <div className="flex items-center gap-2 mb-3">
            <h3 className="font-semibold">{p.name}</h3>
            <span className={clsx("px-2 py-0.5 rounded-full text-xs", p.is_free ? "bg-green-100 text-green-700" : "bg-amber-100 text-amber-700")}>
              {p.is_free ? "Free" : "Paid"}
            </span>
          </div>

          {p.requires_api_key && (
            <div className="mb-3">
              <label className="text-sm text-gray-600">API Key</label>
              <input
                type="password"
                defaultValue={config?.providers?.[p.id]?.api_key === "***" ? "" : config?.providers?.[p.id]?.api_key || ""}
                onBlur={(e) => saveKey(p.id, "api_key", e.target.value)}
                placeholder="Enter API key..."
                className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600"
              />
            </div>
          )}

          {(p.id === "argos" || p.id === "ollama") && (
            <div className="mb-3">
              <label className="text-sm text-gray-600">Base URL</label>
              <input
                defaultValue={config?.providers?.[p.id]?.base_url || (p.id === "argos" ? "http://localhost:5000" : "http://localhost:11434")}
                onBlur={(e) => saveKey(p.id, "base_url", e.target.value)}
                className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600"
              />
            </div>
          )}

          {p.id === "ollama" && (
            <div className="mb-3">
              <label className="text-sm text-gray-600">Model</label>
              <input
                defaultValue={config?.providers?.[p.id]?.model || "llama3.2"}
                onBlur={(e) => saveKey(p.id, "model", e.target.value)}
                className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600"
              />
            </div>
          )}

          {(p.id === "openai" || p.id === "claude") && (
            <div className="mb-3">
              <label className="text-sm text-gray-600">Model</label>
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
              {testing[p.id] ? <Loader size={14} className="animate-spin" /> : "Test Connection"}
            </button>
            {results[p.id] && (
              <span className={clsx("flex items-center gap-1 text-sm", results[p.id].ok ? "text-green-600" : "text-red-600")}>
                {results[p.id].ok ? <CheckCircle size={14} /> : <XCircle size={14} />}
                {results[p.id].ok ? "Connected" : results[p.id].message.slice(0, 60)}
              </span>
            )}
          </div>
        </div>
      ))}
    </div>
  );
}

function DefaultsSection() {
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
      <h2 className="text-xl font-bold">Translation Defaults</h2>
      <div>
        <label className="text-sm font-medium">Default Provider</label>
        <select value={config.default_provider || ""} onChange={(e) => save("default_provider", e.target.value || null)}
          className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600">
          <option value="">None</option>
          {providers?.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
        </select>
      </div>
      <div className="grid grid-cols-2 gap-4">
        <div>
          <label className="text-sm font-medium">Source Language</label>
          <input value={config.default_source_lang} onChange={(e) => save("default_source_lang", e.target.value)}
            className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600" />
        </div>
        <div>
          <label className="text-sm font-medium">Target Language</label>
          <input value={config.default_target_lang} onChange={(e) => save("default_target_lang", e.target.value)}
            className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600" />
        </div>
      </div>
      <div>
        <label className="text-sm font-medium">Batch Size: {config.default_batch_size}</label>
        <input type="range" min={10} max={100} value={config.default_batch_size}
          onChange={(e) => save("default_batch_size", +e.target.value)}
          className="mt-1 w-full" />
      </div>
      <div>
        <label className="text-sm font-medium">Cost Limit ($)</label>
        <input type="number" step="0.01" value={config.default_cost_limit ?? ""}
          onChange={(e) => save("default_cost_limit", e.target.value ? +e.target.value : null)}
          placeholder="No limit"
          className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600" />
      </div>
    </div>
  );
}

function AppearanceSection() {
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

  if (!config) return null;

  return (
    <div className="space-y-6 max-w-md">
      <h2 className="text-xl font-bold">Appearance</h2>
      <div>
        <label className="text-sm font-medium">Theme</label>
        <div className="flex gap-3 mt-2">
          {(["system", "light", "dark"] as const).map((t) => (
            <label key={t} className="flex items-center gap-2 cursor-pointer">
              <input type="radio" name="theme" checked={config.ui.theme === t} onChange={() => setTheme(t)} />
              <span className="text-sm capitalize">{t}</span>
            </label>
          ))}
        </div>
      </div>
      <div>
        <label className="text-sm font-medium">Font Size: {config.ui.font_size}px</label>
        <input type="range" min={12} max={18} value={config.ui.font_size}
          onChange={(e) => setFontSize(+e.target.value)}
          className="mt-1 w-full" />
      </div>
    </div>
  );
}

function GlossarySection() {
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
    const t = term.trim();
    const tr = translation.trim();
    if (!t || !tr) {
      addToast("error", "Term and translation are required.");
      return;
    }
    if (!activePair.trim()) {
      addToast("error", "Language pair is required (e.g. ja-en).");
      return;
    }
    setSaving(true);
    try {
      const entry: GlossaryEntry = {
        term: t,
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
      addToast("error", `Failed to add entry: ${e.message}`);
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
      addToast("error", `Failed to delete: ${e.message}`);
    }
  };

  return (
    <div className="space-y-6 max-w-3xl">
      <h2 className="text-xl font-bold">Glossary</h2>
      <p className="text-sm text-gray-500">
        Preferred term translations used when “Use glossary” is enabled in the translation dialog.
      </p>

      <div className="grid grid-cols-1 sm:grid-cols-3 gap-3 items-end">
        <div>
          <label className="text-sm font-medium">Language pair</label>
          <input
            value={langPairOverride ?? configPair}
            onChange={(e) => setLangPairOverride(e.target.value)}
            placeholder="ja-en"
            className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600"
          />
        </div>
        <div className="sm:col-span-2 relative">
          <label className="text-sm font-medium">Filter</label>
          <div className="relative mt-1">
            <Search size={14} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-gray-400" />
            <input
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder="Search terms or translations..."
              className="w-full pl-8 pr-3 py-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600"
            />
          </div>
        </div>
      </div>

      <div className="border border-gray-200 dark:border-gray-700 rounded-lg p-4 space-y-3">
        <h3 className="text-sm font-semibold text-gray-500 uppercase">Add entry</h3>
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div>
            <label className="text-sm text-gray-600">Term</label>
            <input
              value={term}
              onChange={(e) => setTerm(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleAdd()}
              placeholder="Source term"
              className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600"
            />
          </div>
          <div>
            <label className="text-sm text-gray-600">Translation</label>
            <input
              value={translation}
              onChange={(e) => setTranslation(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleAdd()}
              placeholder="Preferred translation"
              className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600"
            />
          </div>
        </div>
        <button
          onClick={handleAdd}
          disabled={saving}
          className="flex items-center gap-1.5 px-3 py-1.5 bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50 text-white rounded text-sm font-medium"
        >
          <Plus size={14} /> {saving ? "Adding..." : "Add entry"}
        </button>
      </div>

      <div>
        <h3 className="text-sm font-semibold text-gray-500 uppercase mb-2">
          Entries {entries ? `(${filtered.length})` : ""}
        </h3>
        {isLoading ? (
          <p className="text-sm text-gray-500 flex items-center gap-2">
            <Loader size={14} className="animate-spin" /> Loading...
          </p>
        ) : filtered.length === 0 ? (
          <p className="text-sm text-gray-500">
            {filter
              ? "No glossary entries match your filter."
              : "No glossary entries for this language pair yet. Add a term above."}
          </p>
        ) : (
          <table className="w-full text-sm">
            <thead>
              <tr className="text-left text-gray-500 text-xs uppercase">
                <th className="pb-2">Term</th>
                <th className="pb-2">Translation</th>
                <th className="pb-2 w-24">Pair</th>
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
                      title="Delete entry"
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
        title="Delete glossary term"
        message={entryToDelete ? `Delete glossary term “${entryToDelete.term}”?` : ""}
        confirmLabel="Delete"
        destructive
        onConfirm={() => entryToDelete && handleDeleteConfirmed(entryToDelete)}
        onCancel={() => setEntryToDelete(null)}
      />
    </div>
  );
}

function DataSection() {
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
        addToast("success", `Backup ${id} restored`);
      } catch (e: any) {
        addToast("error", `Restore failed: ${e.message}`);
      }
    } else {
      try {
        await deleteBackup(id);
        refetch();
      } catch (e: any) {
        addToast("error", `Delete failed: ${e.message}`);
      }
    }
  };

  return (
    <div className="space-y-6">
      <h2 className="text-xl font-bold">Data</h2>
      <div>
        <h3 className="text-sm font-semibold text-gray-500 uppercase mb-2">Backups</h3>
        {(!backups || backups.length === 0) ? (
          <p className="text-sm text-gray-500">No backups found.</p>
        ) : (
          <table className="w-full text-sm">
            <thead>
              <tr className="text-left text-gray-500">
                <th className="pb-2">ID</th><th className="pb-2">Created</th><th className="pb-2">Files</th><th className="pb-2">Actions</th>
              </tr>
            </thead>
            <tbody>
              {backups.map((b) => (
                <tr key={b.id} className="border-t border-gray-100 dark:border-gray-800">
                  <td className="py-2 font-mono text-xs">{b.id}</td>
                  <td className="py-2">{new Date(b.created_at).toLocaleString()}</td>
                  <td className="py-2">{b.file_count}</td>
                  <td className="py-2 flex gap-2">
                    <button onClick={() => setPendingAction({ kind: "restore", id: b.id })} className="text-emerald-600 hover:text-emerald-800" title="Restore backup"><RotateCcw size={14} /></button>
                    <button onClick={() => setPendingAction({ kind: "delete", id: b.id })} className="text-red-500 hover:text-red-700" title="Delete backup"><Trash2 size={14} /></button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <ConfirmDialog
        open={pendingAction !== null}
        title={pendingAction?.kind === "restore" ? "Restore backup" : "Delete backup"}
        message={
          pendingAction?.kind === "restore"
            ? `Restore backup ${pendingAction.id}? This will overwrite current project files.`
            : `Delete backup ${pendingAction?.id ?? ""}?`
        }
        confirmLabel={pendingAction?.kind === "restore" ? "Restore" : "Delete"}
        destructive
        onConfirm={runPendingAction}
        onCancel={() => setPendingAction(null)}
      />
    </div>
  );
}
