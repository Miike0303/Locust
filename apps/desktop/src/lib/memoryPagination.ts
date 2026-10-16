export const MEMORY_PAGE_SIZE = 50;

/** Keep the current page unless its last row was removed. */
export function memoryPageOffset(offset: number, total: number): number {
  const lastOffset = Math.max(0, Math.ceil(total / MEMORY_PAGE_SIZE) - 1) * MEMORY_PAGE_SIZE;
  return Math.min(offset, lastOffset);
}
