/**
 * Format a stored timestamp in the interface language (`useLocale().locale`),
 * not the operating system's locale. Missing values show a dash; unparsable
 * values keep their first 16 characters so the raw timestamp stays readable.
 */
export function formatDateTime(value: string, locale: string, options?: Intl.DateTimeFormatOptions): string {
  if (!value) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value.length >= 16 ? value.slice(0, 16) : value;
  return date.toLocaleString(locale, options);
}
