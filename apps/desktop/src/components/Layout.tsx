import { Suspense, useState } from "react";
import { Outlet, NavLink, useLocation } from "react-router-dom";
import {
	Home,
	FileText,
	Settings,
	Github,
	Database,
	BookCheck,
	Keyboard,
	ScrollText,
	ListOrdered,
} from "lucide-react";
import clsx from "clsx";
import { useGlobalHotkeys } from "../lib/hotkeys";
import HotkeyHelp from "./HotkeyHelp";
import RouteErrorBoundary from "./RouteErrorBoundary";
import ToastContainer from "./ToastContainer";
import ActivityLog from "./ActivityLog";
import BottomBar from "./BottomBar";
import QueuePanel from "./QueuePanel";
import { useLogStore } from "../stores/logStore";
import { useQueueStore } from "../stores/queueStore";
import { APP_VERSION } from "../lib/appVersion";
import { useT } from "../lib/i18n";
import { useQuery } from "@tanstack/react-query";
import { getConfig } from "../lib/api";

const navItems = [
	{ to: "/", icon: Home, labelKey: "nav.home", shortcut: "Alt+1" },
	{ to: "/editor", icon: FileText, labelKey: "nav.editor", shortcut: "Alt+2" },
	{ to: "/review", icon: BookCheck, labelKey: "nav.review", shortcut: "Alt+3" },
	{ to: "/memory", icon: Database, labelKey: "nav.memory", shortcut: "Alt+4" },
	{ to: "/settings", icon: Settings, labelKey: "nav.settings", shortcut: "Alt+5" },
] as const;

export default function Layout() {
	const t = useT();
	const location = useLocation();
	const { data: config } = useQuery({ queryKey: ["config"], queryFn: getConfig });
	const [showHelp, setShowHelp] = useState(false);
	useGlobalHotkeys(() => setShowHelp(true));

	const { isOpen: logOpen, setOpen: setLogOpen, unreadErrors } = useLogStore();
	const {
		setPanelOpen: setQueueOpen,
		items: queueItems,
		isRunning: queueRunning,
	} = useQueueStore();

	const queueCount = queueItems.filter(
		(i) =>
			i.status === "pending" ||
			i.status === "translating" ||
			i.status === "extracting",
	).length;

	return (
		<div className="flex h-screen">
			<aside className="w-60 shrink-0 flex flex-col bg-surface-muted dark:bg-surface border-r border-border">
				<div className="p-4">
					<h1 className="text-page font-bold text-accent-fg">{t("nav.appName")}</h1>
					<p className="text-caption text-text-muted">
						v{APP_VERSION}
					</p>
				</div>

				<nav className="flex-1 px-2 space-y-1">
					{navItems.map(({ to, icon: Icon, labelKey, shortcut }) => (
						<NavLink
							key={to}
							to={to}
							end={to === "/"}
							className={({ isActive }) =>
								clsx(
									"flex items-center gap-3 px-3 py-2 rounded-md text-body font-medium transition-colors group",
									isActive
										? "bg-accent-muted text-accent-fg"
										: "text-text-muted hover:bg-border/50 hover:text-text",
								)
							}
						>
							<Icon size={18} />
							<span className="flex-1">{t(labelKey)}</span>
							<kbd className="text-caption text-text-muted opacity-0 group-hover:opacity-100 transition-opacity">
								{shortcut}
							</kbd>
						</NavLink>
					))}
				</nav>

				<div className="p-4 border-t border-border space-y-2">
					<button
						onClick={() => setQueueOpen(true)}
						className={clsx(
							"flex items-center gap-2 text-caption w-full focus:outline-none focus:ring-2 focus:ring-accent-fg rounded px-1 py-0.5",
							queueRunning
								? "text-accent-fg"
								: "text-text-muted hover:text-text",
						)}
					>
						<ListOrdered size={14} />
						{t("nav.queue")}
						{queueCount > 0 && (
							<span className="ml-auto px-1.5 py-0.5 rounded-full text-caption font-medium bg-accent-muted text-accent-fg">
								{queueCount}
							</span>
						)}
					</button>
					<button
						onClick={() => setLogOpen(!logOpen)}
						className="flex items-center gap-2 text-caption text-text-muted hover:text-text w-full focus:outline-none focus:ring-2 focus:ring-accent-fg rounded px-1 py-0.5"
					>
						<ScrollText size={14} />
						{t("nav.activityLog")}
						{unreadErrors > 0 && (
							<span className="ml-auto px-1.5 py-0.5 rounded-full text-caption font-medium bg-danger-muted text-danger">
								{unreadErrors}
							</span>
						)}
					</button>
					<button
						onClick={() => setShowHelp(true)}
						className="flex items-center gap-2 text-caption text-text-muted hover:text-text w-full focus:outline-none focus:ring-2 focus:ring-accent-fg rounded px-1 py-0.5"
					>
						<Keyboard size={14} />
						{t("nav.shortcuts")}
						<kbd className="ml-auto text-caption text-text-muted">
							?
						</kbd>
					</button>
					<a
						href="https://github.com/Miike0303/Locust"
						target="_blank"
						rel="noopener noreferrer"
						className="flex items-center gap-2 text-caption text-text-muted hover:text-text focus:outline-none focus:ring-2 focus:ring-accent-fg rounded px-1 py-0.5"
					>
						<Github size={14} />
						{t("nav.github")}
					</a>
				</div>
			</aside>

			<main className="min-w-0 flex-1 flex flex-col overflow-hidden">
				{config?.load_warning && <div role="alert" className="shrink-0 border-b border-warning bg-warning-muted px-5 py-3 text-body text-warning">
					<p>{t("settings.configLoadFailed")}</p>
					<p className="mt-1 break-all font-mono text-caption">{config.load_warning}</p>
				</div>}
				<div className="flex-1 overflow-auto">
				<RouteErrorBoundary key={location.pathname}>
					<Suspense fallback={<div role="status" className="p-6 text-body text-text-muted">{t("common.loading")}</div>}>
						<Outlet />
					</Suspense>
				</RouteErrorBoundary>
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
