/** Last path segment, falling back to the original path for an empty segment. */
export function pathBasename(path: string): string {
  return path.split(/[/\\]/).pop() || path;
}
