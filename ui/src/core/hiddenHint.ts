import type { HiddenCount } from "../api/types";

/** Formats a hidden-match count (`5`, `100+`). */
export function formatHiddenCount(h: HiddenCount): string {
  return `${h.count.toLocaleString()}${h.capped ? "+" : ""}`;
}

/** Phrase for the empty-state message when only hidden files match. */
export function emptyHiddenText(h: HiddenCount): string {
  const n = formatHiddenCount(h);
  const singular = h.count === 1 && !h.capped;
  return singular ? `${n} hidden file matches` : `${n} hidden files match`;
}

/** Phrase for the result-list footer when both visible and hidden files match. */
export function footerHiddenText(h: HiddenCount): string {
  return `+${formatHiddenCount(h)} hidden by filters`;
}
