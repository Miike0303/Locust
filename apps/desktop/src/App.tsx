import { useEffect } from "react";
import { Routes, Route } from "react-router-dom";
import Layout from "./components/Layout";
import Welcome from "./pages/Welcome";
import Editor from "./pages/Editor";
import Settings from "./pages/Settings";
import Review from "./pages/Review";
import TranslationMemory from "./pages/TranslationMemory";
import UpdateChecker from "./components/UpdateChecker";
import { getConfig, getCurrentProject } from "./lib/api";
import { applyAppearance } from "./lib/appearance";
import { useProjectStore } from "./stores/projectStore";

export default function App() {
  // Boot-time restore: apply persisted appearance and reattach the project
  // still open on the server. Both fail silently when the backend is down.
  useEffect(() => {
    getConfig()
      .then((cfg) => applyAppearance(cfg.ui))
      .catch(() => { /* backend unreachable — keep defaults */ });
    if (!useProjectStore.getState().project) {
      getCurrentProject()
        .then((p) => {
          if (p && !useProjectStore.getState().project) {
            useProjectStore.getState().setProject(p);
          }
        })
        .catch(() => { /* no server or no open project — stay on Welcome */ });
    }
  }, []);

  return (
    <>
      <Routes>
        <Route element={<Layout />}>
          <Route path="/" element={<Welcome />} />
          <Route path="/editor" element={<Editor />} />
          <Route path="/review" element={<Review />} />
          <Route path="/memory" element={<TranslationMemory />} />
          <Route path="/settings" element={<Settings />} />
        </Route>
      </Routes>
      <UpdateChecker />
    </>
  );
}
