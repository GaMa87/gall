// What the merge editor needs to ask a local model for one conflict, to show what it proposes and to take it into the
// result only when the user says so. Pure, so it is unit-tested; the editor only draws it. The core checks a proposal
// (`gall-core::compare_ai`); this module builds the request, composes the result with the proposal in it, and tells
// where each proposed line comes from.
import type { MergeAiOutcome, MergeChoice, MergeConflictInput, MergeModel, MergeProposal } from "../api/types";
import { append, chunkLines, eolOf, type Card } from "./mergeCards";

/** Lines of context sent above and below the conflict. */
export const PROPOSAL_CONTEXT = 3;

/** A line without its terminator (`\n` or `\r\n`): the model gets lines, not bytes. */
export const bare = (line: string) => line.replace(/\r?\n$/, "");

/** The request for one conflict: its three versions and the lines around it, as text lines. */
export function conflictInput(card: Card): MergeConflictInput {
  return {
    id: card.id,
    baseLines: card.base.lines.map(bare),
    leftLines: card.left.lines.map(bare),
    rightLines: card.right.lines.map(bare),
    before: card.left.before.slice(-PROPOSAL_CONTEXT).map(bare),
    after: card.left.after.slice(0, PROPOSAL_CONTEXT).map(bare),
  };
}

/** The replacement as lines with the file's terminator, like the lines of the model. */
export const withTerminators = (lines: string[], eol: string) => lines.map((line) => line + eol);

/**
 * The text of the result with the conflict `id` replaced by `replacement` and every other conflict as chosen (one without
 * a choice stays between markers). `before` and `after` are the text on each side of the replacement, so that the
 * syntax of the whole file can be checked; they are given only when no other conflict is open (otherwise null).
 */
export function composeWithProposal(model: MergeModel, base: string[], choices: Record<string, MergeChoice | undefined>, id: string, replacement: string[]): { text: string; before: string; after: string; othersOpen: boolean } {
  const eol = eolOf(model);
  const beforeOut: string[] = [];
  const afterOut: string[] = [];
  let seen = false, othersOpen = false;
  for (const chunk of model.chunks) {
    if (chunk.id === id) {
      seen = true;
      continue;
    }
    const target = seen ? afterOut : beforeOut;
    const lines = chunkLines(chunk, choices[chunk.id], base, eol);
    if (lines) append(target, lines, eol);
    else {
      othersOpen = true;
      append(target, [`<<<<<<< ours${eol}`], eol);
      append(target, chunk.leftLines, eol);
      append(target, [`=======${eol}`], eol);
      append(target, chunk.rightLines, eol);
      append(target, [`>>>>>>> theirs${eol}`], eol);
    }
  }
  const out = [...beforeOut];
  const inserted = withTerminators(replacement, eol);
  if (seen) {
    append(out, inserted, eol);
    append(out, afterOut, eol);
  }
  // Keep a separator after `before` when the preceding chunk has no trailing newline so
  // syntax checks (`before + replacement + "\n" + after`) never merge the first proposed line
  // onto the preceding line, and `after` always matches the exact trailing text.
  const before = seen
    ? beforeOut.join("") + (beforeOut.length > 0 && !beforeOut[beforeOut.length - 1].endsWith("\n") ? eol : "")
    : "";
  const after = seen ? afterOut.join("") : "";
  return { text: out.join(""), before, after, othersOpen };
}

export type Provenance = "both" | "left" | "right" | "base" | "new";

/**
 * For each proposed line, whether the same line is in Local, in Remote, in both, only in the Base (a line that both sides
 * changed or dropped), or nowhere in the conflict (new). Equal text is the only evidence of origin there is: a common
 * line such as `}` says "both" whatever its real source.
 */
export function provenance(replacement: string[], left: string[], right: string[], base: string[] = []): Provenance[] {
  const l = new Set(left), r = new Set(right), b = new Set(base);
  return replacement.map((line) => (l.has(line) && r.has(line) ? "both" : l.has(line) ? "left" : r.has(line) ? "right" : b.has(line) ? "base" : "new"));
}

/** How many proposed lines are in none of the three versions, as a number the preview can show next to the model's own words. */
export function newLines(replacement: string[], left: string[], right: string[], base: string[] = []): number {
  return provenance(replacement, left, right, base).filter((p) => p === "new").length;
}

/** What to tell the user when a request did not end in a proposal; null for a proposal. `t` translates (with `{name}` parameters). */
export function proposalMessage(outcome: MergeAiOutcome, t: (key: string, params?: Record<string, string | number>) => string): string | null {
  switch (outcome.outcome) {
    case "proposed": return null;
    case "stale": return t("The conflict changed since you looked at it. Ask again.");
    case "disabled": return t("The local model is off. Set it up in compare-ai.json (see docs/compare-local-ai.md); nothing is sent anywhere before you do.");
    case "refused": return t("The local model is not usable: {reason}.", { reason: outcome.reason });
    case "tooLarge": return t("This conflict is too large to send to the model.");
    case "timeout": return t("The local model took too long and was stopped.");
    case "cancelled": return t("Cancelled.");
    case "failed": return t("The local model failed: {message}", { message: outcome.message });
    case "malformed": return t("The local model gave a proposal that could not be used ({reason}); it was ignored and the conflict is as it was.", { reason: outcome.reason });
  }
}

/** Whether a proposal can still be taken: it was made for the revision of the conflict that is on screen now. */
export function canAccept(proposal: MergeProposal, currentRevision: string | null): boolean {
  return currentRevision !== null && proposal.revision === currentRevision;
}
