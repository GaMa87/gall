// Hint labels (Vimium style): one or two letters per visible control; typing a label clicks it, with Shift it right-clicks.
// Pure rules only; `components/HintLabels.tsx` finds the controls and shows the labels, and only while hint mode is on.

/** Home row first, so the most common labels are the easiest to reach. */
export const HINT_ALPHABET = "asdfghjklqwertyuiopzxcvbnm";

/** The most labels two letters give: every label two letters long. */
export const maxHintLabels = (alphabet = HINT_ALPHABET) => alphabet.length * alphabet.length;

/**
 * `count` labels where none is the beginning of another, so typing one never waits for a second letter that may not
 * come: as many one-letter labels as the count allows, the rest two letters from the remaining first letters. Labels
 * beyond the maximum are not made (a window cannot show that many controls usefully).
 */
export function hintLabels(count: number, alphabet = HINT_ALPHABET): string[] {
  const letters = [...alphabet], n = letters.length;
  const wanted = Math.max(0, Math.min(count, maxHintLabels(alphabet)));
  if (wanted <= n) return letters.slice(0, wanted);
  // `single` one-letter labels leave n - single first letters, each starting n two-letter labels:
  // n * n - single * (n - 1) labels in all, so the most singles that still reach `wanted`:
  const single = Math.min(n, Math.floor((n * n - wanted) / (n - 1)));
  const labels = letters.slice(0, single);
  for (const first of letters.slice(single)) for (const second of letters) { if (labels.length === wanted) return labels; labels.push(first + second); }
  return labels;
}
export type HintMatch = { exact: number } | { partial: number[] } | { none: true };
/** What the typed letters name: one label, several that still start with them, or nothing. Case does not matter. */
export function matchHint(typed: string, labels: readonly string[]): HintMatch {
  const text = typed.toLowerCase();
  if (!text) return { partial: labels.map((_, index) => index) };
  const exact = labels.indexOf(text);
  if (exact >= 0) return { exact };
  const partial = labels.flatMap((label, index) => (label.startsWith(text) ? [index] : []));
  return partial.length ? { partial } : { none: true };
}

export type Box = { left: number; top: number; right: number; bottom: number };
/** Whether a control's box is on screen (inside `view`) and has a size. */
export function onScreen(box: Box, view: Box): boolean {
  return box.right > box.left && box.bottom > box.top && box.right > view.left && box.left < view.right && box.bottom > view.top && box.top < view.bottom;
}

/** What a click can be aimed at. */
export const HINT_TARGETS = [
  "button", "a[href]", "summary", "select", "textarea", "input:not([type=hidden])", "[role=tab]", "[role=button]", "[role=menuitem]", "[role=option]", "[role=checkbox]",
  ".gg-row", ".gg-side-row", ".gc-file", "[data-hint]",
].join(",");
