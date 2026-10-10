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
					className="w-full max-w-md rounded-xl border border-border bg-surface p-6 shadow-sm"
				>
					<div className="flex items-start gap-3">
						<span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-warning-muted text-warning">
							<AlertTriangle size={20} aria-hidden="true" />
						</span>
						<div className="min-w-0">
							<h2 className="text-section font-semibold text-text">
								{t("route.crash.title")}
							</h2>
							<p className="mt-1 text-body text-text-muted">
								{t("route.crash.body")}
							</p>
						</div>
					</div>
					<div className="mt-5 flex flex-wrap items-center gap-3">
						<button
							type="button"
							onClick={this.handleReload}
							className="inline-flex items-center gap-2 rounded-md bg-accent px-4 py-2 text-body font-medium text-white transition-colors hover:bg-accent-hover focus:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-2 focus-visible:ring-offset-surface"
						>
							<RotateCw size={16} aria-hidden="true" />
							{t("route.crash.reload")}
						</button>
						<span className="text-caption text-text-muted">
							{t("route.crash.sidebarHint")}
						</span>
					</div>
					<details className="mt-5 text-caption text-text-muted">
						<summary className="cursor-pointer select-none rounded focus:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg">
							{t("route.crash.details")}
						</summary>
						<pre className="mt-2 max-h-40 overflow-auto whitespace-pre-wrap break-all rounded-md bg-surface-muted p-3 font-mono text-caption text-text">
							{error.message || error.name}
						</pre>
					</details>
				</div>
			</div>
		);
	}
}
