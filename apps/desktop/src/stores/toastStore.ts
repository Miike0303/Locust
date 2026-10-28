import { create } from "zustand";

export type ToastType = "success" | "error" | "info" | "warning";
type PauseReason = "pointer" | "focus";

export interface Toast {
  id: string;
  type: ToastType;
  message: string;
}

interface ToastStore {
  toasts: Toast[];
  add: (type: ToastType, message: string, duration?: number) => void;
  dismiss: (id: string) => void;
  pause: (id: string, reason: PauseReason) => void;
  resume: (id: string, reason: PauseReason) => void;
}

const priority = { success: 0, info: 0, warning: 1, error: 2 };
interface Clock {
  timer?: ReturnType<typeof setTimeout>;
  remaining: number;
  startedAt: number;
  reasons: Set<PauseReason>;
}

export const useToastStore = create<ToastStore>((set, get) => {
  // Only admitted, timed toasts own clocks; persistent messages allocate no timer.
  const clocks = new Map<string, Clock>();
  const clearClock = (id: string) => {
    const clock = clocks.get(id);
    if (clock) clearTimeout(clock.timer);
    clocks.delete(id);
  };
  const startClock = (id: string, clock: Clock) => {
    clock.startedAt = Date.now();
    clock.timer = setTimeout(() => get().dismiss(id), clock.remaining);
  };
  return {
    toasts: [],
    add: (type, message, duration) => {
      let toasts = get().toasts;
      if (toasts.length >= 5) {
        const lowest = Math.min(...toasts.map(toast => priority[toast.type]));
        // A success burst cannot silently discard an error. At equal priority,
        // prefer newer messages; an all-error queue still has a hard five cap.
        if (priority[type] < lowest) return;
        const victim = toasts.find(toast => priority[toast.type] === lowest)!;
        clearClock(victim.id);
        toasts = toasts.filter(toast => toast.id !== victim.id);
      }
      const id = crypto.randomUUID();
      const lifetime = duration ?? (type === "error" || type === "warning" ? undefined : 4000);
      if (lifetime !== undefined) {
        // Preserve the old asynchronous immediate expiry for zero/negative values.
        const clock: Clock = { remaining: Math.max(0, lifetime), startedAt: 0, reasons: new Set() };
        clocks.set(id, clock);
        startClock(id, clock);
      }
      set({ toasts: [...toasts, { id, type, message }] });
    },
    dismiss: (id) => {
      clearClock(id);
      set(state => ({ toasts: state.toasts.filter(toast => toast.id !== id) }));
    },
    pause: (id, reason) => {
      const clock = clocks.get(id);
      if (!clock || clock.reasons.has(reason)) return;
      if (clock.reasons.size === 0) {
        clearTimeout(clock.timer);
        clock.remaining = Math.max(0, clock.remaining - Math.max(0, Date.now() - clock.startedAt));
      }
      clock.reasons.add(reason);
    },
    resume: (id, reason) => {
      const clock = clocks.get(id);
      if (!clock || !clock.reasons.delete(reason) || clock.reasons.size > 0) return;
      startClock(id, clock);
    },
  };
});

export const addToast = (type: ToastType, message: string, duration?: number) =>
  useToastStore.getState().add(type, message, duration);
