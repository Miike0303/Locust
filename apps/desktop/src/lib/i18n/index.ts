import {
  createContext,
  createElement,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { en } from "./en";
import { es } from "./es";

export type Locale = "en" | "es";
export type MessageKey = keyof typeof en;
export type Vars = Record<string, string | number>;
export type TranslateFn = (key: string, vars?: Vars) => string;

export const UI_LANGUAGE_KEY = "locust.ui.language";
export const LOCALES: readonly Locale[] = ["en", "es"];

const catalogs: Record<Locale, Record<string, string>> = { en, es };

export type StringStorage = {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
};

function browserStorage(): StringStorage | null {
  try {
    if (typeof localStorage === "undefined") return null;
    return localStorage;
  } catch {
    return null;
  }
}

export function isLocale(value: unknown): value is Locale {
  return value === "en" || value === "es";
}

export function detectLocale(
  storage?: StringStorage | null,
  navigatorLanguage?: string,
): Locale {
  const s = storage === undefined ? browserStorage() : storage;
  if (s) {
    try {
      const stored = s.getItem(UI_LANGUAGE_KEY);
      if (isLocale(stored)) return stored;
    } catch {
      /* ignore quota / private mode */
    }
  }
  const nav =
    navigatorLanguage ??
    (typeof navigator !== "undefined" ? navigator.language : "");
  if (nav.toLowerCase().startsWith("es")) return "es";
  return "en";
}

export function persistLocale(
  locale: Locale,
  storage?: StringStorage | null,
): void {
  const s = storage === undefined ? browserStorage() : storage;
  if (!s) return;
  try {
    s.setItem(UI_LANGUAGE_KEY, locale);
  } catch {
    /* ignore quota / private mode */
  }
}

function interpolate(template: string, vars?: Vars): string {
  if (!vars) return template;
  let out = template;
  for (const [name, value] of Object.entries(vars)) {
    out = out.replace(new RegExp(`\\{${name}\\}`, "g"), String(value));
  }
  return out;
}

function isDev(): boolean {
  try {
    return Boolean(import.meta.env?.DEV);
  } catch {
    return false;
  }
}

/** Pure translator — used by tests and the React hook. */
export function translate(
  catalog: Record<string, string>,
  locale: string,
  key: string,
  vars?: Vars,
): string {
  let resolved: string | undefined;
  if (vars && typeof vars.count === "number") {
    let rule = "other";
    try {
      rule = new Intl.PluralRules(locale).select(vars.count);
    } catch {
      rule = vars.count === 1 ? "one" : "other";
    }
    resolved = catalog[`${key}.${rule}`] ?? catalog[`${key}.other`];
  }
  if (resolved == null) resolved = catalog[key];
  if (resolved == null) {
    if (isDev()) console.warn(`[i18n] missing key: ${key}`);
    return key;
  }
  return interpolate(resolved, vars);
}

let currentLocale: Locale = detectLocale();
const listeners = new Set<() => void>();

export function getLocale(): Locale {
  return currentLocale;
}

export function applyDocumentLang(locale: Locale): void {
  if (typeof document !== "undefined") {
    document.documentElement.lang = locale;
  }
}

export function setLocale(next: Locale): void {
  currentLocale = next;
  persistLocale(next);
  applyDocumentLang(next);
  listeners.forEach((fn) => fn());
}

/** Standalone t() for non-React callers (api.ts). */
export function t(key: string, vars?: Vars): string {
  return translate(catalogs[currentLocale], currentLocale, key, vars);
}

type I18nContextValue = {
  locale: Locale;
  setLocale: (locale: Locale) => void;
  t: TranslateFn;
};

const I18nContext = createContext<I18nContextValue | null>(null);

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocaleState] = useState<Locale>(() => {
    const initial = detectLocale();
    currentLocale = initial;
    return initial;
  });

  useEffect(() => {
    applyDocumentLang(locale);
  }, [locale]);

  useEffect(() => {
    const onChange = () => setLocaleState(currentLocale);
    listeners.add(onChange);
    return () => {
      listeners.delete(onChange);
    };
  }, []);

  const changeLocale = useCallback((next: Locale) => {
    setLocale(next);
    setLocaleState(next);
  }, []);

  const tFn = useCallback<TranslateFn>(
    (key, vars) => translate(catalogs[locale], locale, key, vars),
    [locale],
  );

  const value = useMemo(
    () => ({ locale, setLocale: changeLocale, t: tFn }),
    [locale, changeLocale, tFn],
  );

  return createElement(I18nContext.Provider, { value }, children);
}

export function useT(): TranslateFn {
  const ctx = useContext(I18nContext);
  return ctx?.t ?? t;
}

export function useLocale(): {
  locale: Locale;
  setLocale: (locale: Locale) => void;
} {
  const ctx = useContext(I18nContext);
  if (!ctx) return { locale: currentLocale, setLocale };
  return { locale: ctx.locale, setLocale: ctx.setLocale };
}
