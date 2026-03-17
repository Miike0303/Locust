import { useNavigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { FolderOpen, Globe } from "lucide-react";
import { getFormats, getConfig, openProject } from "../lib/api";
import { useProjectStore } from "../stores/projectStore";

const IS_TAURI = "__TAURI_INTERNALS__" in window;

export default function Welcome() {
  const navigate = useNavigate();
  const setProject = useProjectStore((s) => s.setProject);
  const { data: formats } = useQuery({ queryKey: ["formats"], queryFn: getFormats });
  const { data: config } = useQuery({ queryKey: ["config"], queryFn: getConfig });

  const handleOpenGame = async () => {
    try {
      let path: string | null = null;

      if (IS_TAURI) {
        const { open } = await import("@tauri-apps/plugin-dialog");
        const selected = await open({
          title: "Select game executable or main file",
          filters: [
            {
              name: "Game files",
              extensions: ["exe", "html", "htm", "rpy", "rpa", "rpgproject", "rvproj2"],
            },
            { name: "All files", extensions: ["*"] },
          ],
        });
        if (typeof selected === "string") path = selected;
      } else {
        path = prompt("Enter game executable or folder path:");
      }

      if (!path) return;
      const result = await openProject(path);
      setProject({ path: result.project_path, format_id: result.format_id, name: result.project_name });
      navigate("/editor");
    } catch (err) {
      alert(`Failed to open project: ${err}`);
    }
  };

  return (
    <div className="flex flex-col items-center justify-center min-h-full p-8">
      <div className="text-center mb-12">
        <Globe size={64} className="mx-auto mb-4 text-emerald-500" />
        <h1 className="text-4xl font-bold mb-2">Project Locust</h1>
        <p className="text-lg text-gray-500">Universal game translation tool</p>
      </div>

      <button
        onClick={handleOpenGame}
        className="flex items-center gap-3 px-8 py-4 bg-emerald-600 hover:bg-emerald-700 text-white rounded-lg text-lg font-medium transition-colors mb-8"
      >
        <FolderOpen size={24} />
        Open game
      </button>

      {config?.recent_projects && config.recent_projects.length > 0 && (
        <div className="w-full max-w-lg mb-8">
          <h2 className="text-sm font-semibold text-gray-500 uppercase mb-3">Recent Projects</h2>
          <div className="space-y-2">
            {config.recent_projects.map((p: any, i: number) => (
              <button
                key={i}
                onClick={async () => {
                  try {
                    const result = await openProject(p.path);
                    setProject({ path: result.project_path, format_id: result.format_id, name: result.project_name });
                    navigate("/editor");
                  } catch (err) {
                    alert(`Failed to open: ${err}`);
                  }
                }}
                className="w-full text-left p-3 rounded-md border border-gray-200 dark:border-gray-700 hover:bg-gray-50 dark:hover:bg-gray-800 transition-colors"
              >
                <div className="font-medium">{p.name}</div>
                <div className="text-xs text-gray-500">{p.format_id} · {p.path}</div>
              </button>
            ))}
          </div>
        </div>
      )}

      {formats && formats.length > 0 && (
        <div className="w-full max-w-lg">
          <h2 className="text-sm font-semibold text-gray-500 uppercase mb-3">Supported Formats</h2>
          <div className="flex flex-wrap gap-2">
            {formats.map((f: any) => (
              <span
                key={f.id}
                className="px-3 py-1 bg-gray-100 dark:bg-gray-800 rounded-full text-sm text-gray-700 dark:text-gray-300"
              >
                {f.name}
              </span>
            ))}
          </div>
        </div>
      )}

      <footer className="mt-12 text-center text-xs text-gray-400">
        MIT License · <a href="https://github.com/Miike0303/Locust" className="hover:underline">GitHub</a>
      </footer>
    </div>
  );
}
