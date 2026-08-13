import { useToastStore } from "../stores/toastStore";
import { X, CheckCircle, AlertCircle, Info, AlertTriangle } from "lucide-react";

const icons = {
	success: CheckCircle,
	error: AlertCircle,
	info: Info,
	warning: AlertTriangle,
};

const colors = {
	success: "bg-emerald-600 text-white dark:bg-emerald-700",
	error: "bg-red-600 text-white dark:bg-red-700",
	info: "bg-blue-600 text-white dark:bg-blue-700",
	warning: "bg-amber-500 text-white dark:bg-amber-600",
};

export default function ToastContainer() {
	const { toasts, dismiss } = useToastStore();

	if (toasts.length === 0) return null;

	return (
		<div
			className="fixed top-4 right-4 z-[100] flex flex-col gap-2 max-w-sm"
			role="region"
			aria-label="Notifications"
		>
			{toasts.map((toast) => {
				const Icon = icons[toast.type];
				return (
					<div
						key={toast.id}
						role="alert"
						className={`flex items-start gap-2 px-4 py-3 rounded-lg shadow-lg text-sm animate-[slideIn_0.2s_ease-out] ${colors[toast.type]}`}
					>
						<Icon size={16} className="shrink-0 mt-0.5" aria-hidden="true" />
						<span className="flex-1">{toast.message}</span>
						<button
							type="button"
							onClick={() => dismiss(toast.id)}
							aria-label="Dismiss notification"
							className="shrink-0 opacity-70 hover:opacity-100 focus:outline-none focus:ring-2 focus:ring-white/50 rounded p-0.5"
						>
							<X size={14} />
						</button>
					</div>
				);
			})}
		</div>
	);
}
