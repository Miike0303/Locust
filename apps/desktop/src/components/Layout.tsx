import { Suspense, useEffect, useState } from "react";
import { Outlet, NavLink, useLocation } from "react-router-dom";
import {
	Home,
	LayoutDashboard,
	FileText,
	Settings,
	Github,
	Database,
	BookCheck,
	ShieldCheck,
	Users,
	Keyboard,
	ScrollText,
	ListOrdered,
	PanelLeftOpen,
	PanelLeftClose,
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
import { isCompactLayout, readSidebarCollapsed, saveSidebarCollapsed, subscribeCompactLayout } from "../lib/layoutPrefs";

const navItems = [
	{ to: "/", icon: Home, labelKey: "nav.home", shortcut: "Alt+1" },
	{ to: "/dashboard", icon: LayoutDashboard, labelKey: "nav.dashboard", shortcut: "Alt+6" },
	{ to: "/editor", icon: FileText, labelKey: "nav.editor", shortcut: "Alt+2" },
	{ to: "/review", icon: BookCheck, labelKey: "nav.review", shortcut: "Alt+3" },
	{ to: "/qa", icon: ShieldCheck, labelKey: "nav.qa", shortcut: null },
	{ to: "/speakers", icon: Users, labelKey: "nav.speakers", shortcut: null },
	{ to: "/memory", icon: Database, labelKey: "nav.memory", shortcut: "Alt+4" },
	{ to: "/settings", icon: Settings, labelKey: "nav.settings", shortcut: "Alt+5" },
] as const;

export default function Layout() {
	const t = useT();
	const location = useLocation();
	const { data: config } = useQuery({ queryKey: ["config"], queryFn: getConfig });
	const [showHelp, setShowHelp] = useState(false);
	const [compact, setCompact] = useState(isCompactLayout);
	const [sidebarPreference, setSidebarPreference] = useState(readSidebarCollapsed);
	const collapsed = sidebarPreference ?? compact;
	useEffect(() => subscribeCompactLayout(setCompact), []);
	const toggleSidebar = () => {
		setSidebarPreference(!collapsed);
		saveSidebarCollapsed(!collapsed);
	};
	const toggleLabel = t(collapsed ? "nav.expandSidebar" : "nav.collapseSidebar");
	const tooltip = (label: string) => collapsed && (
		<span role="tooltip" className="pointer-events-none invisible absolute left-full top-1/2 z-50 ml-2 -translate-y-1/2 whitespace-nowrap rounded border border-border bg-surface px-2 py-1 text-caption text-text shadow-lg group-hover:visible group-focus-visible:visible">
			{label}
		</span>
	);
	const utilityClass = clsx(
		"group relative flex items-center gap-2 text-caption w-full focus:outline-none focus:ring-2 focus:ring-accent-fg rounded",
		collapsed ? "justify-center h-9" : "px-1 py-0.5",
	);
	const mutedUtilityClass = clsx(utilityClass, "text-text-muted hover:text-text");
	const badgePosition = collapsed ? "absolute -right-1 -top-1" : "ml-auto";
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
			<aside className="w-60 data-[collapsed=true]:w-16 shrink-0 flex flex-col bg-surface-muted dark:bg-surface border-r border-border" data-collapsed={collapsed}>
				<div className={clsx("p-4", collapsed && "text-center")}>
					<p className="text-page font-bold text-accent-fg" title={t("nav.appName")} aria-label={t("nav.appName")}>{collapsed ? "L" : t("nav.appName")}</p>
					{!collapsed && <p className="text-caption text-text-muted">
						v{APP_VERSION}
					</p>}
				</div>

				<nav className="flex-1 px-2 space-y-1">
					{navItems.map(({ to, icon: Icon, labelKey, shortcut }) => (
						<NavLink
							key={to}
							to={to}
							end={to === "/"}
							title={t(labelKey)}
							aria-label={t(labelKey)}
							className={({ isActive }) =>
								clsx(
									"relative flex items-center gap-3 px-3 py-2 rounded-md text-body font-medium transition-colors group focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg",
									collapsed && "justify-center",
									isActive
										? "bg-accent-muted text-accent-fg"
										: "text-text-muted hover:bg-border/50 hover:text-text",
								)
							}
						>
							<Icon size={18} className="shrink-0" aria-hidden="true" />
							{!collapsed && <span className="flex-1">{t(labelKey)}</span>}
							{tooltip(t(labelKey))}
							{!collapsed && shortcut && (
								<kbd className="text-caption opacity-0 group-hover:opacity-100 transition-opacity">
									{shortcut}
								</kbd>
							)}
						</NavLink>
					))}
				</nav>

				<div className={clsx("border-t border-border space-y-2", collapsed ? "p-2" : "p-4")}>
					<button
						onClick={() => setQueueOpen(true)}
						title={t("nav.queue")}
						aria-label={t("nav.queue")}
						className={clsx(
							utilityClass,
							queueRunning
								? "text-accent-fg"
								: "text-text-muted hover:text-text",
						)}
					>
						<ListOrdered size={14} aria-hidden="true" />
						{!collapsed && t("nav.queue")}
						{tooltip(t("nav.queue"))}
						{queueCount > 0 && (
							<span className={clsx(badgePosition, "px-1.5 py-0.5 rounded-full text-caption font-medium bg-accent-muted text-accent-fg")}>
								{queueCount}
							</span>
						)}
					</button>
					<button
						onClick={() => setLogOpen(!logOpen)}
						title={t("nav.activityLog")}
						aria-label={t("nav.activityLog")}
						className={mutedUtilityClass}
					>
						<ScrollText size={14} aria-hidden="true" />
						{!collapsed && t("nav.activityLog")}
						{tooltip(t("nav.activityLog"))}
						{unreadErrors > 0 && (
							<span className={clsx(badgePosition, "px-1.5 py-0.5 rounded-full text-caption font-medium bg-danger-muted text-danger")}>
								{unreadErrors}
							</span>
						)}
					</button>
					<button
						onClick={() => setShowHelp(true)}
						title={t("nav.shortcuts")}
						aria-label={t("nav.shortcuts")}
						className={mutedUtilityClass}
					>
						<Keyboard size={14} aria-hidden="true" />
						{!collapsed && t("nav.shortcuts")}
						{tooltip(t("nav.shortcuts"))}
						{!collapsed && <kbd className="ml-auto text-caption text-text-muted">
							?
						</kbd>}
					</button>
					<a
						href="https://github.com/Miike0303/Locust"
						target="_blank"
						rel="noopener noreferrer"
						title={t("nav.github")}
						aria-label={t("nav.github")}
						className={mutedUtilityClass}
					>
						<Github size={14} aria-hidden="true" />
						{!collapsed && t("nav.github")}
						{tooltip(t("nav.github"))}
					</a>
					<button type="button" onClick={toggleSidebar} title={toggleLabel} aria-label={toggleLabel} aria-expanded={!collapsed} className={mutedUtilityClass}>
						{collapsed ? <PanelLeftOpen size={16} aria-hidden="true" /> : <PanelLeftClose size={16} aria-hidden="true" />}
						{!collapsed && toggleLabel}
						{tooltip(toggleLabel)}
					</button>
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
