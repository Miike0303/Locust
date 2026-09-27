import { Component, type ErrorInfo, type ReactNode } from "react";
import { AlertTriangle, RotateCw } from "lucide-react";
import { t } from "../lib/i18n";

type Props = {
	children?: ReactNode;
	/** Injected for tests; defaults to a full window reload. */
	reload?: () => void;
};

type State = { error: Error | null };

const defaultReload = () => window.location.reload();

/**
 * Catches render errors and failed lazy-chunk imports for one routed page so
 * the sidebar, queue and toasts stay usable. Layout keys it by pathname, so
 * navigating to another page clears the error without a full reload.
 */
export default class RouteErrorBoundary extends Component<Props, State> {
	state: State = { error: null };

	static getDerivedStateFromError(error: unknown): State {
		return { error: error instanceof Error ? error : new Error(String(error)) };
	}

	componentDidCatch(error: Error, info: ErrorInfo): void {
		console.error("[route] screen crashed", error, info.componentStack);
	}

	handleReload = (): void => {
		(this.props.reload ?? defaultReload)();
	};

	render(): ReactNode {
		const { error } = this.state;
		if (!error) return this.props.children;
		return (
			<div className="flex min-h-full items-center justify-center p-6">
				<div
					role="alert"
					className="w-full max-w-md rounded-xl border border-gray-200 bg-white p-6 shadow-sm dark:border-gray-700 dark:bg-gray-900"
				>
					<div className="flex items-start gap-3">
						<span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-amber-50 text-amber-600 dark:bg-amber-950/60 dark:text-amber-400">
							<AlertTriangle size={20} aria-hidden="true" />
						</span>
						<div className="min-w-0">
							<h2 className="text-base font-semibold text-gray-900 dark:text-gray-100">
								{t("route.crash.title")}
							</h2>
							<p className="mt-1 text-sm text-gray-600 dark:text-gray-400">
								{t("route.crash.body")}
							</p>
						</div>
					</div>
					<div className="mt-5 flex flex-wrap items-center gap-3">
						<button
							type="button"
							onClick={this.handleReload}
							className="inline-flex items-center gap-2 rounded-md bg-emerald-600 px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-emerald-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-emerald-500 focus-visible:ring-offset-2 dark:focus-visible:ring-offset-gray-900"
						>
							<RotateCw size={16} aria-hidden="true" />
							{t("route.crash.reload")}
						</button>
						<span className="text-xs text-gray-500 dark:text-gray-400">
							{t("route.crash.sidebarHint")}
						</span>
					</div>
					<details className="mt-5 text-xs text-gray-500 dark:text-gray-400">
						<summary className="cursor-pointer select-none rounded focus:outline-none focus-visible:ring-2 focus-visible:ring-emerald-500">
							{t("route.crash.details")}
						</summary>
						<pre className="mt-2 max-h-40 overflow-auto whitespace-pre-wrap break-all rounded-md bg-gray-50 p-3 font-mono text-[11px] text-gray-700 dark:bg-gray-800 dark:text-gray-300">
							{error.message || error.name}
						</pre>
					</details>
				</div>
			</div>
		);
	}
}
