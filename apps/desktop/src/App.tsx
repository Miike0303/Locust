import { lazy, useEffect } from "react";
import { Routes, Route } from "react-router-dom";
import Layout from "./components/Layout";
import NotFound from "./pages/NotFound";
import UpdateChecker from "./components/UpdateChecker";
import GameSourceDialog from "./components/GameSourceDialog";
import { ResumeProjectDialogHost } from "./components/ResumeProjectDialog";
import { getConfig, getCurrentProject } from "./lib/api";
import { restoreBootAppearance } from "./lib/appBoot";
import { useProjectStore } from "./stores/projectStore";

const Welcome = lazy(() => import("./pages/Welcome"));
const Dashboard = lazy(() => import("./pages/Dashboard"));
const Editor = lazy(() => import("./pages/Editor"));
const Settings = lazy(() => import("./pages/Settings"));
const Review = lazy(() => import("./pages/Review"));
const Qa = lazy(() => import("./pages/Qa"));
const Speakers = lazy(() => import("./pages/Speakers"));
const TranslationMemory = lazy(() => import("./pages/TranslationMemory"));

export default function App() {
  // Boot-time restore: apply persisted appearance and reattach the project
  // still open on the server. Both fail silently when the backend is down.
  useEffect(() => {
    void restoreBootAppearance(getConfig);
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
          <Route path="/dashboard" element={<Dashboard />} />
          <Route path="/editor" element={<Editor />} />
          <Route path="/review" element={<Review />} />
          <Route path="/qa" element={<Qa />} />
          <Route path="/speakers" element={<Speakers />} />
          <Route path="/memory" element={<TranslationMemory />} />
          <Route path="/settings" element={<Settings />} />
          <Route path="*" element={<NotFound />} />
        </Route>
      </Routes>
      <UpdateChecker />
      <ResumeProjectDialogHost />
      <GameSourceDialog />
    </>
  );
}
