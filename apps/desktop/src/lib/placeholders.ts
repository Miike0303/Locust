// Token extraction ported from crates/core/src/placeholder.rs. Keep its six
// scanners, profile mapping, precedence and bounded interpolation grammar aligned.
type TokenSpan = { start: number; end: number };
type PlaceholderPart = { text: string; token: boolean };

function nameboxLength(text: string): number {
  return /^\\n</i.test(text) ? 2 : /^\\n[cr]</i.test(text) ? 3 : 0;
}

function bracketEnd(text: string, start: number): number | null {
  let depth = 0;
  let quote: string | null = null;
  for (let i = start; i < text.length; i++) {
    const char = text[i];
    if (quote) {
      if (char === "\\") i++;
      else if (char === quote) quote = null;
    } else if (char === "'" || char === '"') quote = char;
    else if (char === "[") {
      if (++depth > 16) return null;
    } else if (char === "]") {
      if (--depth < 0) return null;
      if (!depth) return i + 1;
    } else if (char === "\n" || char === "\r") return null;
  }
  return null;
}

function quotedKey(key: string): boolean {
  const quote = key[0];
  if (quote !== "'" && quote !== '"') return false;
  for (let i = 1; i < key.length; i++) {
    if (key[i] === "\\") i++;
    else if (key[i] === quote) return i + 1 === key.length;
  }
  return false;
}

function renpyInterpolation(inner: string): boolean {
  const first = /^[A-Za-z_][A-Za-z_0-9]*/.exec(inner);
  if (!first) return false;
  let i = first[0].length;
  while (i < inner.length) {
    if (inner[i] === ".") {
      const attribute = /^[A-Za-z_][A-Za-z_0-9]*/.exec(inner.slice(++i));
      if (!attribute) return false;
      i += attribute[0].length;
    } else if (inner[i] === "[") {
      const end = bracketEnd(inner, i);
      if (end === null) return false;
      const key = inner.slice(i + 1, end - 1);
      if (!/^-?[0-9]+$/.test(key) && !quotedKey(key)) return false;
      i = end;
    } else if (inner[i] === "!") return /^[sraqtulc]+$/.test(inner.slice(i + 1));
    else return false;
  }
  return true;
}

function formatName(name: string): boolean {
  return /^[A-Za-z_0-9]{2,}$/.test(name) || /^[a-z]$/.test(name);
}

function formatInner(inner: string): boolean {
  const equals = inner.indexOf("=");
  if (equals >= 0) {
    return ["a", "alpha", "color", "cps", "font", "size", "outlinecolor", "plain", "k", "image", "space", "vspace", "w", "p"].includes(inner.slice(0, equals))
      && /^[^{}\n\r]+$/.test(inner.slice(equals + 1));
  }
  if (inner.startsWith("/")) return formatName(inner.slice(1));
  return inner === "" || /^[0-9]+$/.test(inner) || formatName(inner);
}

function sentinelLookalike(text: string): boolean {
  return /^\{\s*(?:PL|G)[_\-:\s]/.test(text);
}

export function placeholderParts(text: string, format: string): PlaceholderPart[] {
  const matches: TokenSpan[] = [];
  const add = (start: number, end: number) => matches.push({ start, end });

  // RPG Maker codes, including complete name-box escapes (arguments stay text).
  for (let i = 0; i < text.length; i++) {
    if (text[i] !== "\\" || i + 1 >= text.length) continue;
    const next = text[i + 1].toLowerCase();
    if (next === "\\") { i++; continue; }
    const namebox = nameboxLength(text.slice(i));
    if (namebox) { add(i, i + namebox); i += namebox - 1; continue; }
    if ("cvnpi".includes(next) && text[i + 2] === "[") {
      const close = text.indexOf("]", i + 3);
      if (close >= 0) { add(i, close + 1); i = close; continue; }
    }
    if ("g$.!><^|".includes(next)) { add(i, i + 2); i++; }
  }

  // HTML; only immediate, unescaped known name-box arguments are exempt.
  for (let i = 0; i < text.length; i++) {
    if (text[i] === "\\") {
      if (text[i + 1] === "\\") { i++; continue; }
      const namebox = nameboxLength(text.slice(i));
      if (namebox) {
        const close = text.indexOf(">", i + namebox);
        if (close >= 0) { i = close; continue; }
      }
    }
    if (text[i] !== "<") continue;
    const close = text.indexOf(">", i);
    if (close >= 0 && close + 1 - i >= 3 && (text[i + 1] === "/" || /[A-Za-z]/.test(text[i + 1]))) {
      add(i, close + 1); i = close;
    }
  }

  // Python/printf and RPG Maker single-digit substitution codes.
  for (let i = 0; i < text.length; i++) {
    if (text[i] !== "%") continue;
    if (text[i + 1] === "%") { i++; continue; }
    const match = /^%(?:\([^)]*\)[sdifg]|[sdif]|[1-9](?![0-9]))/.exec(text.slice(i));
    if (match) { add(i, i + match[0].length); i += match[0].length - 1; }
  }

  // Rust format fields, Ren'Py text tags and opaque source sentinel lookalikes.
  for (let i = 0; i < text.length; i++) {
    if (text[i] !== "{") continue;
    if (text[i + 1] === "{") {
      if (!sentinelLookalike(text.slice(i + 1))) i++;
      continue;
    }
    const close = text.indexOf("}", i);
    if (sentinelLookalike(text.slice(i))) {
      const newline = text.slice(i).search(/[\r\n]/);
      const end = close >= 0 ? close + 1 : newline >= 0 ? i + newline : text.length;
      add(i, end); i = end - 1; continue;
    }
    if (close >= 0 && formatInner(text.slice(i + 1, close))) { add(i, close + 1); i = close; }
  }

  // C-style newline/tab escapes, avoiding escaped backslashes and RPG \n[n].
  for (let i = 0; i < text.length; i++) {
    if (text[i] !== "\\") continue;
    const next = text[i + 1];
    if (next === "\\") { i++; continue; }
    if ((next === "n" || next === "t") && !(next === "n" && text[i + 2] === "[")) {
      add(i, i + 2); i++;
    }
  }

  if (!["rpgmaker-mv", "rpgmaker-mz", "rpgmaker-vxa", "wolf-rpg"].includes(format)) {
    for (let i = 0; i < text.length; i++) {
      if (text[i] !== "[") continue;
      if (text[i + 1] === "[") { i++; continue; }
      const end = bracketEnd(text, i);
      if (end !== null) {
        if (renpyInterpolation(text.slice(i + 1, end - 1))) add(i, end);
        i = end - 1;
      }
    }
  }

  // Stable sorting preserves Rust's scanner precedence for equal starts.
  matches.sort((a, b) => a.start - b.start);
  const parts: PlaceholderPart[] = [];
  let offset = 0;
  for (const match of matches) {
    if (match.start < offset) continue;
    if (match.start > offset) parts.push({ text: text.slice(offset, match.start), token: false });
    parts.push({ text: text.slice(match.start, match.end), token: true });
    offset = match.end;
  }
  if (offset < text.length) parts.push({ text: text.slice(offset), token: false });
  return parts;
}

// Ordered multisets: consume one matching occurrence at a time, retaining
// source order for missing tokens and translation order for extra tokens.
export function placeholderCheck(source: string[], translated: string[]) {
  const remaining = [...translated];
  const missing = source.filter(token => {
    const index = remaining.indexOf(token);
    if (index < 0) return true;
    remaining.splice(index, 1);
    return false;
  });
  return { missing, extra: remaining, present: source.length - missing.length };
}
