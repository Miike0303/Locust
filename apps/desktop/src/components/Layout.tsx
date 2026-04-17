import { useState } from "react";
import { Outlet, NavLink } from "react-router-dom";
import { Home, FileText, Settings, Github, Database, BookCheck, Keyboard, ScrollText, ListOrdered } from "lucide-react";
import clsx from "clsx";
import { useGlobalHotkeys } from "../lib/hotkeys";
import HotkeyHelp from "./HotkeyHelp";
import ToastContainer from "./ToastContainer";
import ActivityLog from "./ActivityLog";
import BottomBar from "./BottomBar";
import QueuePanel from "./QueuePanel";
import { useLogStore } from "../stores/logStore";
import { useQueueStore } from "../stores/queueStore";

const navItems = [
  { to: "/", icon: Home, label: "Home", shortcut: "Alt+1" },
  { to: "/editor", icon: FileText, label: "Editor", shortcut: "Alt+2" },
  { to: "/review", icon: BookCheck, label: "Review", shortcut: "Alt+3" },
  { to: "/memory", icon: Database, label: "Memory", shortcut: "Alt+4" },
  { to: "/settings", icon: Settings, label: "Settings", shortcut: "Alt+5" },
];

export default function Layout() {
  const [showHelp, setShowHelp] = useState(false);
  useGlobalHotkeys(() => setShowHelp(true));

  const { isOpen: logOpen, setOpen: setLogOpen, unreadErrors } = useLogStore();
  const { isPanelOpen: queueOpen, setPanelOpen: setQueueOpen, items: queueItems, isRunning: queueRunning } = useQueueStore();

  const queueCount = queueItems.filter((i) => i.status === "pending" || i.status === "translating" || i.status === "extracting").length;

  return (
    <div className="flex h-screen">
      <aside className="w-60 flex flex-col bg-gray-50 dark:bg-gray-800 border-r border-gray-200 dark:border-gray-700">
        <div className="p-4">
          <h1 className="text-lg font-bold text-emerald-600">Project Locust</h1>
          <p className="text-xs text-gray-500">v0.1.0</p>
        </div>

        <nav className="flex-1 px-2 space-y-1">
          {navItems.map(({ to, icon: Icon, label, shortcut }) => (
            <NavLink
              key={to}
              to={to}
              end={to === "/"}
              className={({ isActive }) =>
                clsx(
                  "flex items-center gap-3 px-3 py-2 rounded-md text-sm font-medium transition-colors group",
                  isActive
                    ? "bg-emerald-100 text-emerald-700 dark:bg-emerald-900 dark:text-emerald-300"
                    : "text-gray-700 hover:bg-gray-100 dark:text-gray-300 dark:hover:bg-gray-700"
                )
              }
            >
              <Icon size={18} />
              <span className="flex-1">{label}</span>
              <kbd className="text-[10px] text-gray-400 opacity-0 group-hover:opacity-100 transition-opacity">
                {shortcut}
              </kbd>
            </NavLink>
          ))}
        </nav>

        <div className="p-4 border-t border-gray-200 dark:border-gray-700 space-y-2">
          <button
            onClick={() => setQueueOpen(true)}
            className={clsx(
              "flex items-center gap-2 text-xs w-full",
              queueRunning
                ? "text-emerald-500"
                : "text-gray-500 hover:text-gray-700 dark:hover:text-gray-300"
            )}
          >
            <ListOrdered size={14} />
            Queue
            {queueCount > 0 && (
              <span className="ml-auto px-1.5 py-0.5 rounded-full text-[10px] font-medium bg-emerald-100 text-emerald-700 dark:bg-emerald-900 dark:text-emerald-300">
                {queueCount}
              </span>
            )}
          </button>
          <button
            onClick={() => setLogOpen(!logOpen)}
            className="flex items-center gap-2 text-xs text-gray-500 hover:text-gray-700 dark:hover:text-gray-300 w-full"
          >
            <ScrollText size={14} />
            Activity Log
            {unreadErrors > 0 && (
              <span className="ml-auto px-1.5 py-0.5 rounded-full text-[10px] font-medium bg-red-100 text-red-700 dark:bg-red-900 dark:text-red-300">
                {unreadErrors}
              </span>
            )}
          </button>
          <button
            onClick={() => setShowHelp(true)}
            className="flex items-center gap-2 text-xs text-gray-500 hover:text-gray-700 dark:hover:text-gray-300 w-full"
          >
            <Keyboard size={14} />
            Shortcuts
            <kbd className="ml-auto text-[10px] text-gray-400">?</kbd>
          </button>
          <a
            href="https://github.com/Miike0303/Locust"
            target="_blank"
            rel="noopener noreferrer"
            className="flex items-center gap-2 text-xs text-gray-500 hover:text-gray-700"
          >
            <Github size={14} />
            GitHub
          </a>
        </div>
      </aside>

      <main className="flex-1 flex flex-col overflow-hidden">
        <div className="flex-1 overflow-auto">
          <Outlet />
        </div>
        <BottomBar />
      </main>

      <HotkeyHelp open={showHelp} onClose={() => setShowHelp(false)} />
      <ActivityLog />
      <QueuePanel />
      <ToastContainer />
    </div>
  );
}
