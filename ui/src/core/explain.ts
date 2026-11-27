// Logic of the explanation panel of the comparison window (the local model is opt-in and asked only on request):
// the category filters, how sure the model says it is, and the words for every way a request can end.
import type { AiCategory, AiExplainScope, AiItem, AiOutcome } from "../api/types";
import { tt } from "../i18n";

/** The labels are English wordings that the view passes through `tt` where it shows them. */
export const AI_CATEGORIES: { id: AiCategory; label: string }[] = [
  { id: "bugfix", label: "Bugfix" },
  { id: "feature", label: "Feature" },
  { id: "refactor", label: "Refactor" },
  { id: "performance", label: "Performance" },
  { id: "formatting", label: "Formatting/Style" },
  { id: "unknown", label: "Unknown" },
];

/** The items to show: all of them when no filter is chosen, else those of the chosen categories, in the model's order. */
export function filterItems(items: AiItem[], chosen: ReadonlySet<AiCategory>): AiItem[] {
  return chosen.size === 0 ? items : items.filter((i) => chosen.has(i.category));
}

/** How many items each category has, for the filter buttons. */
export function categoryCounts(items: AiItem[]): Record<AiCategory, number> {
  const counts: Record<AiCategory, number> = { bugfix: 0, feature: 0, refactor: 0, performance: 0, formatting: 0, unknown: 0 };
  for (const i of items) counts[i.category]++;
  return counts;
}

/** The model's confidence as "70 %": a claim of the model, never shown as a measurement (hence the sign). */
export function confidenceText(confidence: number): string {
  const pct = Math.round(Math.min(1, Math.max(0, Number.isFinite(confidence) ? confidence : 0)) * 100);
  return `${pct} %`;
}

/** What to tell the user when a request did not end in an explanation. `null` for an explanation. */
export function outcomeMessage(outcome: AiOutcome): string | null {
  switch (outcome.outcome) {
    case "explained": return null;
    case "disabled": return tt("The local explanation model is off. Set it up in compare-ai.json (see docs/compare-local-ai.md); nothing is ever sent anywhere before you do.");
    case "refused": return tt("The local model is not usable: {reason}.", { reason: outcome.reason });
    case "tooLarge": return tt("There are too many changes to explain at once. Narrow the comparison and try again.");
    case "timeout": return tt("The local model took too long and was stopped.");
    case "cancelled": return tt("Cancelled.");
    case "failed": return tt("The local model failed: {message}", { message: outcome.message });
    case "malformed": return tt("The local model gave an answer that could not be used ({reason}); it was ignored.", { reason: outcome.reason });
  }
}

/** Whether an answer made for one pair of texts still belongs to what the window shows (the texts did not change). */
export function isCurrent(askedFor: { left: string; right: string } | null, now: { left: string; right: string }): boolean {
  return !!askedFor && askedFor.left === now.left && askedFor.right === now.right;
}

/** Lightweight staleness check using line-array references and serialized options (avoids joining full texts until E is pressed). */
export interface ExplainSnapshot {
  leftLines: readonly string[];
  rightLines: readonly string[];
  optsKey: string;
}

export function isSnapshotCurrent(askedFor: ExplainSnapshot | null, now: ExplainSnapshot | null): boolean {
  return !!askedFor && !!now
    && askedFor.leftLines === now.leftLines
    && askedFor.rightLines === now.rightLines
    && askedFor.optsKey === now.optsKey;
}

/** Human-readable description of what an explanation covers. */
export function scopeLabel(scope: AiExplainScope): string {
  if (scope.kind === "all") return tt("All changes");
  if (scope.kind === "hunk") return tt("Change {number}", { number: scope.index + 1 });
  const lo = Math.min(scope.startLine, scope.endLine);
  const hi = Math.max(scope.startLine, scope.endLine);
  return tt("Selected {side} lines {from}–{to}", { side: tt(scope.side), from: lo, to: hi });
}
