/**
 * Ordered unique entry ids from a validate response — one visit per string
 * even when it has multiple issue kinds.
 */
export function uniqueIssueEntryIds(
	issues: ReadonlyArray<{ entry_id: string }>,
): string[] {
	const seen = new Set<string>();
	const out: string[] = [];
	for (const issue of issues) {
		const id = issue.entry_id;
		if (!id || seen.has(id)) continue;
		seen.add(id);
		out.push(id);
	}
	return out;
}

/** Clamp step within `[0, len)`. Empty list → 0. */
export function stepWorklistIndex(
	current: number,
	len: number,
	delta: number,
): number {
	if (len <= 0) return 0;
	return Math.max(0, Math.min(len - 1, current + delta));
}

/** Whether the Review-in-Editor CTA should be enabled. */
export function canStartValidationWorklist(issueCount: number): boolean {
	return issueCount > 0;
}
