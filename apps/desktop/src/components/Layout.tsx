import { Outlet, NavLink } from "react-router-dom";
import { Home, FileText, Settings, Github } from "lucide-react";
import clsx from "clsx";

const navItems = [
  { to: "/", icon: Home, label: "Home" },
  { to: "/editor", icon: FileText, label: "Editor" },
  { to: "/settings", icon: Settings, label: "Settings" },
];

export default function Layout() {
  return (
    <div className="flex h-screen">
      <aside className="w-60 flex flex-col bg-gray-50 dark:bg-gray-800 border-r border-gray-200 dark:border-gray-700">
        <div className="p-4">
          <h1 className="text-lg font-bold text-emerald-600">Project Locust</h1>
          <p className="text-xs text-gray-500">v0.1.0</p>
        </div>

        <nav className="flex-1 px-2 space-y-1">
          {navItems.map(({ to, icon: Icon, label }) => (
            <NavLink
              key={to}
              to={to}
              end={to === "/"}
              className={({ isActive }) =>
                clsx(
                  "flex items-center gap-3 px-3 py-2 rounded-md text-sm font-medium transition-colors",
                  isActive
                    ? "bg-emerald-100 text-emerald-700 dark:bg-emerald-900 dark:text-emerald-300"
                    : "text-gray-700 hover:bg-gray-100 dark:text-gray-300 dark:hover:bg-gray-700"
                )
              }
            >
              <Icon size={18} />
              {label}
            </NavLink>
          ))}
        </nav>

        <div className="p-4 border-t border-gray-200 dark:border-gray-700">
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

      <main className="flex-1 overflow-auto">
        <Outlet />
      </main>
    </div>
  );
}
