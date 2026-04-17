import { create } from "zustand";

export type LogLevel = "info" | "warning" | "error";

export interface LogEntry {
  id: string;
  timestamp: number;
  level: LogLevel;
  message: string;
  detail?: string;
  source?: string;
}

interface LogStore {
  entries: LogEntry[];
  filter: LogLevel | "all";
  isOpen: boolean;
  unreadErrors: number;
  add: (level: LogLevel, message: string, detail?: string, source?: string) => void;
  clear: () => void;
  setFilter: (f: LogLevel | "all") => void;
  setOpen: (v: boolean) => void;
}

export const useLogStore = create<LogStore>((set) => ({
  entries: [],
  filter: "all",
  isOpen: false,
  unreadErrors: 0,
  add: (level, message, detail, source) => {
    const entry: LogEntry = {
      id: crypto.randomUUID(),
      timestamp: Date.now(),
      level,
      message,
      detail,
      source,
    };
    set((s) => ({
      entries: [entry, ...s.entries].slice(0, 500),
      unreadErrors: s.isOpen ? s.unreadErrors : s.unreadErrors + (level === "error" ? 1 : 0),
    }));
  },
  clear: () => set({ entries: [], unreadErrors: 0 }),
  setFilter: (filter) => set({ filter }),
  setOpen: (isOpen) => set((s) => ({
    isOpen,
    unreadErrors: isOpen ? 0 : s.unreadErrors,
  })),
}));

export const addLog = (level: LogLevel, message: string, detail?: string, source?: string) =>
  useLogStore.getState().add(level, message, detail, source);
