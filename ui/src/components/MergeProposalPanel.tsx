import type { MergeAiOutcome } from "../api/types";
import { newLines, proposalMessage, provenance } from "../core/mergeProposal";
import { tt } from "../i18n";

interface Props {
  /** `null` while the model works. */
  outcome: MergeAiOutcome | null;
  sideLabels: { left: string; right: string; base: string };
  left: string[];
  right: string[];
  base: string[];
  /** Another conflict has no choice yet: taking the proposal then leaves markers that are settled by hand. */
  othersOpen: boolean;
  busy: boolean;
  onAccept: () => void;
  onReject: () => void;
  onCancel: () => void;
}

/**
 * The proposal of the local model for one conflict, beside the cards: the lines it would put there with where each comes
 * from (Local, Remote, both, or new), its explanation, risk, uncertainty and the confidence it claims, and its evidence.
 * Nothing is in the result until Accept; Reject and Esc leave everything as it was.
 */
export function MergeProposalPanel({ outcome, sideLabels, left, right, base, othersOpen, busy, onAccept, onReject, onCancel }: Props) {
  const message = outcome ? proposalMessage(outcome, tt) : null;
  const done = outcome?.outcome === "proposed" ? outcome : null;
  const p = done?.proposal;
  const marks = p ? provenance(p.replacement, left, right, base) : [];
  const label = { both: tt("both"), left: sideLabels.left, right: sideLabels.right, base: sideLabels.base, new: tt("new") } as const;
  return (
    <section className="gm-pane gm-proposal" aria-label={tt("Proposal of the local model")}>
      <h3>{tt("Proposal of the local model")}<small>{done ? done.model || tt("(unnamed)") : ""}{done?.cached ? ` · ${tt("from the cache")}` : ""}</small></h3>
      {!outcome && <p role="status">{tt("Asking the local model…")} <button disabled={busy} onClick={onCancel}>{tt("Cancel")}</button></p>}
      {message && <p role="status">{message} <button onClick={onReject}>{tt("Close")}</button></p>}
      {p && (
        <>
          <p className="gm-proposal-by">{tt("The model's words, not a measurement: it can be wrong. Check every line before you take it.")}</p>
          <p>{p.explanation}</p>
          <p><b>{tt("Risk")}:</b> {p.risk}</p>
          <p><b>{tt("Uncertainty")}:</b> {p.uncertainty}</p>
          <p>{tt("The model says {pct} % sure.", { pct: Math.round(p.confidence * 100) })} {tt("{count} new lines.", { count: newLines(p.replacement, left, right, base) })}</p>
          <pre className="gm-proposal-lines" role="group" aria-label={tt("Proposed lines")}>
            {p.replacement.length === 0 ? <i className="gm-placeholder">{tt("(empty)")}{"\n"}</i> : p.replacement.map((line, i) => <span key={i} className={`gm-prov gm-prov-${marks[i]}`} title={label[marks[i]]}><span className="gm-sr">{label[marks[i]]}: </span>{line}{"\n"}</span>)}
          </pre>
          {p.evidence.length > 0 && <ul className="gm-proposal-evidence">{p.evidence.map((e, i) => <li key={i}>{tt("{source} lines {from}–{to}", { source: e.source === "base" ? sideLabels.base : e.source === "left" ? sideLabels.left : sideLabels.right, from: e.startLine, to: e.endLine })}</li>)}</ul>}
          {othersOpen && <p role="note" className="gm-proposal-warn">{tt("Other conflicts have no choice yet: they stay between markers in the result, and choosing a side later replaces this text. Decide them first.")}</p>}
          <div className="gm-proposal-actions">
            <button className="gm-confirm" disabled={busy} onClick={onAccept}>{tt("Take the proposal")}</button>
            <button disabled={busy} onClick={onReject}>{tt("Reject (Esc)")}</button>
          </div>
        </>
      )}
    </section>
  );
}
