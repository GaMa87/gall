import { useEffect, useRef, useState } from "react";
import type { AiCategory, AiExplainScope, AiOutcome } from "../api/types";
import { tt } from "../i18n";
import { AI_CATEGORIES, categoryCounts, confidenceText, filterItems, outcomeMessage, scopeLabel } from "../core/explain";

interface Props {
  /** `null` while the model works. */
  outcome: AiOutcome | null;
  /** Which subset of changes this explanation covers. */
  scope: AiExplainScope;
  /** Number of changed blocks on screen and the currently active block index (0-based, -1 if none). */
  hunkCount: number;
  activeHunk: number;
  /** Current line selection in the diff body, if any. */
  selectionScope?: Extract<AiExplainScope, { kind: "selection" }> | null;
  /** The explanation was made for texts or diff options that have changed since: shown as out of date. */
  stale: boolean;
  /** Ask again for another scope (all changes, one hunk, or the current selection). */
  onScope: (scope: AiExplainScope) => void;
  /** An item in the list was chosen: jump to its hunk in the text (0-based block index). */
  onHunk: (hunkIndex: number) => void;
  /** A piece of evidence was chosen: go to that line of that side (1-based). */
  onEvidence: (side: "left" | "right", line: number) => void;
  onCancel: () => void;
  onClose: () => void;
}

/**
 * The explanation of the changes by the local model, beside the comparison. It says whose words these are (the model's
 * name), shows how sure the model claims to be, offers scope controls (all changes, one hunk, or selection) and
 * category filters, and links every item and piece of evidence to its hunk and lines. It is only opened on request;
 * the model is never asked while it is closed.
 */
export function CompareExplanation({ outcome, scope, hunkCount, activeHunk, selectionScope, stale, onScope, onHunk, onEvidence, onCancel, onClose }: Props) {
  const [chosen, setChosen] = useState<ReadonlySet<AiCategory>>(new Set());
  const panel = useRef<HTMLElement>(null);
  // Focus moves in, so that Esc closes the panel (and not the whole comparison) and the keys reach its buttons.
  useEffect(() => { panel.current?.focus(); }, []);
  const toggle = (c: AiCategory) => setChosen((s) => { const n = new Set(s); if (n.has(c)) n.delete(c); else n.add(c); return n; });
  const message = outcome ? outcomeMessage(outcome) : null;
  const done = outcome?.outcome === "explained" ? outcome : null;
  const counts = done ? categoryCounts(done.explanation.items) : null;
  const targetHunk = activeHunk >= 0 && activeHunk < hunkCount ? activeHunk : 0;
  return (
    <aside
      ref={panel}
      className="dv-outline dv-explain"
      aria-label={tt("Explanation by the local model")}
      tabIndex={-1}
      onKeyDown={(e) => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); if (!outcome) onCancel(); onClose(); } }}
    >
      <div className="dv-outline-head"><b>{tt("Explanation")}</b><button className="icon-btn" aria-label={tt("Close explanation (Esc)")} title={tt("Close explanation (Esc)")} onClick={() => { if (!outcome) onCancel(); onClose(); }}>×</button></div>
      <div className="dv-explain-scope" role="group" aria-label={tt("Scope")}>
        <button type="button" className={`tgl ${scope.kind === "all" ? "on" : ""}`} aria-pressed={scope.kind === "all"} onClick={() => onScope({ kind: "all" })}>{tt("All changes")}</button>
        {hunkCount > 0 && (
          <button
            type="button"
            className={`tgl ${scope.kind === "hunk" ? "on" : ""}`}
            aria-pressed={scope.kind === "hunk"}
            onClick={() => onScope({ kind: "hunk", index: scope.kind === "hunk" ? scope.index : targetHunk })}
          >
            {tt("Change {number}", { number: (scope.kind === "hunk" ? scope.index : targetHunk) + 1 })}
          </button>
        )}
        {selectionScope && (
          <button
            type="button"
            className={`tgl ${scope.kind === "selection" ? "on" : ""}`}
            aria-pressed={scope.kind === "selection"}
            onClick={() => onScope(selectionScope)}
          >
            {tt("Selection")}
          </button>
        )}
      </div>
      {!outcome && (
        <div role="status" className="dv-explain-body">
          <p>{tt("Asking the local model…")} ({scopeLabel(scope)})</p>
          <button className="tgl" onClick={onCancel}>{tt("Cancel")}</button>
        </div>
      )}
      {message && <p role="status" className="dv-explain-body">{message}</p>}
      {done && counts && (
        <div className="dv-explain-body">
          <p className="dv-explain-by">{tt("By the local model {model}", { model: done.model || tt("(unnamed)") })}{done.cached ? ` · ${tt("from the cache")}` : ""} · {scopeLabel(scope)}. {tt("It can be wrong: check the lines it points to.")}</p>
          {stale && <p role="alert" className="dv-explain-stale">{tt("The texts changed since this was written.")}</p>}
          <h4>{tt("What changed")}</h4><p>{done.explanation.summary}</p>
          <h4>{tt("Plausible why")}</h4><p>{done.explanation.why}</p>
          <h4>{tt("Risk")}</h4><p>{done.explanation.risk}</p>
          <h4>{tt("Uncertainty")}</h4><p>{done.explanation.uncertainty}</p>
          <p>{tt("Overall: {category}, the model says {confidence} sure.", { category: tt(AI_CATEGORIES.find((c) => c.id === done.explanation.category)?.label ?? ""), confidence: confidenceText(done.explanation.confidence) })}</p>
          {done.explanation.items.length > 0 && (
            <>
              <div role="group" aria-label={tt("Filter by kind of change")} className="dv-explain-filters">
                {AI_CATEGORIES.map((c) => (
                  <button key={c.id} className={`tgl ${chosen.has(c.id) ? "on" : ""}`} aria-pressed={chosen.has(c.id)} disabled={counts[c.id] === 0} onClick={() => toggle(c.id)}>{tt(c.label)} {counts[c.id]}</button>
                ))}
              </div>
              <ul>
                {filterItems(done.explanation.items, chosen).map((i) => (
                  <li key={i.hunk}>
                    <button
                      type="button"
                      className="dv-explain-link"
                      aria-label={tt("Jump to change {number}", { number: i.hunk + 1 })}
                      onClick={() => onHunk(i.hunk)}
                    >
                      {tt("Change {number}: {category}, {confidence}", { number: i.hunk + 1, category: tt(AI_CATEGORIES.find((c) => c.id === i.category)?.label ?? ""), confidence: confidenceText(i.confidence) })}
                    </button>
                    {i.note ? ` — ${i.note}` : ""}
                  </li>
                ))}
              </ul>
            </>
          )}
          {done.explanation.evidence.length > 0 && (
            <>
              <h4>{tt("Evidence")}</h4>
              <ul>
                {done.explanation.evidence.map((e, k) => (
                  <li key={k}><button type="button" className="dv-explain-link" onClick={() => onEvidence(e.side, e.startLine)}>{tt("Change {number}, {side} lines {from}–{to}", { number: e.hunk + 1, side: tt(e.side), from: e.startLine, to: e.endLine })}</button></li>
                ))}
              </ul>
            </>
          )}
        </div>
      )}
    </aside>
  );
}
