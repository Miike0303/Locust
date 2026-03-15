import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { CheckCircle, XCircle, Loader, Trash2, RotateCcw } from "lucide-react";
import clsx from "clsx";
import {
  getProviders, checkProviderHealth, getConfig, updateConfig,
  getBackups, restoreBackup, deleteGlossaryEntry,
} from "../lib/api";
import type { AppConfig, ProviderInfo } from "../lib/api";

const SECTIONS = ["Providers", "Translation Defaults", "Appearance", "Data"] as const;
type Section = (typeof SECTIONS)[number];

export default function Settings() {
  const [section, setSection] = useState<Section>("Providers");

  return (
    <div className="flex h-full">
      <nav className="w-48 border-r border-gray-200 dark:border-gray-700 p-4 space-y-1">
        {SECTIONS.map((s) => (
          <button
            key={s}
            onClick={() => setSection(s)}
            className={clsx(
              "block w-full text-left px-3 py-2 rounded text-sm font-medium",
              section === s
                ? "bg-emerald-100 text-emerald-700 dark:bg-emerald-900 dark:text-emerald-300"
                : "text-gray-600 hover:bg-gray-100 dark:text-gray-400 dark:hover:bg-gray-800"
            )}
          >
            {s}
          </button>
        ))}
      </nav>
      <div className="flex-1 p-6 overflow-y-auto">
        {section === "Providers" && <ProvidersSection />}
        {section === "Translation Defaults" && <DefaultsSection />}
        {section === "Appearance" && <AppearanceSection />}
        {section === "Data" && <DataSection />}
      </div>
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
    await updateConfig({ ui: { ...config?.ui, theme } } as any);
    qc.invalidateQueries({ queryKey: ["config"] });
    const root = document.documentElement;
    root.classList.remove("dark", "light");
    if (theme === "dark") root.classList.add("dark");
    else if (theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches) root.classList.add("dark");
  };

  const setFontSize = async (size: number) => {
    await updateConfig({ ui: { ...config?.ui, font_size: size } } as any);
    qc.invalidateQueries({ queryKey: ["config"] });
    document.documentElement.style.fontSize = `${size}px`;
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

function DataSection() {
  const { data: backups, refetch } = useQuery({ queryKey: ["backups"], queryFn: getBackups });
  const qc = useQueryClient();

  const handleRestore = async (id: string) => {
    if (!confirm(`Restore backup ${id}? This will overwrite current project files.`)) return;
    try {
      await restoreBackup(id);
      alert("Restored successfully");
    } catch (e: any) {
      alert(`Restore failed: ${e.message}`);
    }
  };

  const handleDelete = async (id: string) => {
    if (!confirm(`Delete backup ${id}?`)) return;
    try {
      await fetch(`/api/backups/${id}`, { method: "DELETE" });
      refetch();
    } catch (e: any) {
      alert(`Delete failed: ${e.message}`);
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
                    <button onClick={() => handleRestore(b.id)} className="text-emerald-600 hover:text-emerald-800"><RotateCcw size={14} /></button>
                    <button onClick={() => handleDelete(b.id)} className="text-red-500 hover:text-red-700"><Trash2 size={14} /></button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
