import { useToastStore } from "../stores/toastStore";
import { X, CheckCircle, AlertCircle, Info, AlertTriangle } from "lucide-react";
import { useT } from "../lib/i18n";

const icons = {
	success: CheckCircle,
	error: AlertCircle,
	info: Info,
	warning: AlertTriangle,
};

const colors = {
	success: "bg-success-muted text-success",
	error: "bg-danger-muted text-danger",
	info: "bg-accent-muted text-accent-fg",
	warning: "bg-warning-muted text-warning",
};

export default function ToastContainer() {
	const t = useT();
	const { toasts, dismiss, pause, resume } = useToastStore();

	if (toasts.length === 0) return null;

	return (
		<div
			className="fixed top-4 right-4 z-[100] flex flex-col gap-2 w-[calc(100%-2rem)] max-w-sm max-h-[calc(100dvh-2rem)] overflow-y-auto overflow-x-hidden"
			role="region"
			aria-label={t("toast.notifications")}
		>
			{toasts.map((toast) => {
				const Icon = icons[toast.type];
				return (
					<div
						key={toast.id}
						role={toast.type === "error" ? "alert" : "status"}
						aria-live={toast.type === "error" ? "assertive" : "polite"}
						aria-atomic={true}
						onPointerEnter={() => pause(toast.id, "pointer")}
						onPointerLeave={() => resume(toast.id, "pointer")}
						onFocus={() => pause(toast.id, "focus")}
						onBlur={(event) => {
							if (!event.currentTarget.contains(event.relatedTarget)) resume(toast.id, "focus");
						}}
						className={`flex shrink-0 min-w-0 items-start gap-2 px-4 py-3 rounded-lg shadow-lg text-body ${colors[toast.type]}`}
					>
						<Icon size={16} className="shrink-0 mt-0.5" aria-hidden="true" />
						<span className="flex-1 min-w-0 [overflow-wrap:anywhere]">{toast.message}</span>
						<button
							type="button"
							onClick={() => dismiss(toast.id)}
							aria-label={t("toast.dismiss")}
							className="shrink-0 focus:outline-none focus:ring-2 focus:ring-current rounded p-0.5"
						>
							<X size={14} />
						</button>
					</div>
				);
			})}
		</div>
	);
}
