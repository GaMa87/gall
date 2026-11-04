// Small fuzzy scorer for short in-memory lists (command palette). File-name
// search uses the Rust index; this mirrors its rules: in-order letters,
// bonuses for word starts and consecutive runs, smart case.

export interface FuzzyMatch {
  score: number;
  positions: number[];
}

/** Letters that have no decomposition but a plain-letter look (đ, ł, ø …). */
const BASE: Record<string, string> = { "đ": "d", "Đ": "D", "ł": "l", "Ł": "L", "ø": "o", "Ø": "O", "ħ": "h", "Ħ": "H", "ı": "i", "ŧ": "t", "Ŧ": "T" };

/** The base letter of an accented Latin letter (č → c), one character for one, as the
 *  Rust index does (fuzzy::fold_char). */
export function foldChar(c: string): string {
  if (c < "À") return c;
  if (BASE[c]) return BASE[c];
  const d = c.normalize("NFD");
  return d.length > 1 && /^\p{L}\p{M}+$/u.test(d) ? d[0] : c;
}

/** `s` with accents removed, the same length (per character). */
export function foldText(s: string): string {
  let out = "";
  for (const c of s) out += foldChar(c);
  return out;
}

/** Typed without accented letters, a pattern matches them as their base letter. */
export const wantsFold = (pattern: string) => foldText(pattern) === pattern;

export function fuzzyMatch(pattern: string, text: string): FuzzyMatch | null {
  const p = pattern.trim();
  if (!p) return { score: 0, positions: [] };
  const cs = /[A-Z]/.test(p);
  const folded = wantsFold(p) ? foldText(text) : text;
  const t = cs ? folded : folded.toLowerCase();
  const q = cs ? p : p.toLowerCase();
  let score = 0;
  let ti = 0;
  let prev = -2;
  const positions: number[] = [];
  for (const ch of q) {
    if (ch === " ") continue;
    const at = t.indexOf(ch, ti);
    if (at < 0) return null;
    const boundary = at === 0 || /[\s_\-./:()]/.test(text[at - 1]) || (/[a-z]/.test(text[at - 1]) && /[A-Z]/.test(text[at]));
    score += 16 + (boundary ? 10 : 0) + (at === prev + 1 ? 8 : 0) - Math.min(at - ti, 6);
    positions.push(at);
    prev = at;
    ti = at + 1;
  }
  return { score: score - text.length * 0.1, positions };
}
