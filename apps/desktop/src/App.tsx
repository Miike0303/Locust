import { Routes, Route } from "react-router-dom";
import Layout from "./components/Layout";
import Welcome from "./pages/Welcome";
import Editor from "./pages/Editor";
import Settings from "./pages/Settings";

export default function App() {
  return (
    <Routes>
      <Route element={<Layout />}>
        <Route path="/" element={<Welcome />} />
        <Route path="/editor" element={<Editor />} />
        <Route path="/settings" element={<Settings />} />
      </Route>
    </Routes>
  );
}
