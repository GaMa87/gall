import { Fragment } from "react";
import { colouredLine, useMergeHighlight } from "./MergeHighlight";
import { nextRequestId } from "../core/requestId";
import { useEffect, useMemo, useRef, useState } from "react";
import * as A from "../state/actions";
import { api } from "../api/ipc";
import type { MergeAiOutcome, MergeChoice, MergeOutcome, MergeRequest } from "../api/types";
import { splitLines } from "../core/gitMerge";
import { bothRepeatsLines, commonLineIndices, compose, gapHasLines, lineRange, overview, stepCard, type Card, type CardSide, type Gap } from "../core/mergeCards";
import { tt } from "../i18n";
import "./GitMerge.css";
import { platformLabel } from "../core/keymap";
import { gitKeyOf } from "../core/gitCommands";
import { useGitKeys } from "../state/gitKeys";
import { GitKeyCap } from "./GitKeyCap";
import { MergeProposalPanel } from "./MergeProposalPanel";
import { canAccept, composeWithProposal, conflictInput } from "../core/mergeProposal";
import { MergeWindow } from "./MergeWindow";
import { textRows, WINDOW_FROM } from "../core/windowRows";

type Side = { file: string; base: string; ours: string; theirs: string; result: string };
const choices: { id: MergeChoice; label: string; allLabel: string; key: string }[] = [
  { id: "left", label: "Local", allLabel: "All local", key: "o" },
  { id: "right", label: "Remote", allLabel: "All remote", key: "t" },
  { id: "combine", label: "Combine changes", allLabel: "Combine all", key: "b" },
  { id: "both", label: "Both full blocks", allLabel: "All both", key: "" },
  { id: "base", label: "Base", allLabel: "All base", key: "B" },
];
const hasMarkers = (text: string) => /^(<{7}|={7}|>{7})( |$)/m.test(text);
let tickets = Date.now();

/** What an empty side says: nothing is shown as a blank, never as a missing pane. */
function placeholder(name: "base" | "left" | "right", side: CardSide): string | null {
  if (side.lines.length) return null;
  return name === "base" ? tt("(not in the base)") : side.kind === "deleted" ? tt("(deleted on this side)") : tt("(empty)");
}

export function GitMergeEditor({ conflict, vim, busy, onConfirm, onOpenExternal, onDirty, onFailure, gitOperation }: { gitOperation?: string | null; conflict: Side; vim: boolean; busy: boolean; onConfirm: (content: string) => Promise<void>; onOpenExternal: () => void; onDirty?: (dirty: boolean) => void; onFailure?: (message: string) => void }) {
  const sources = useMemo(() => ({ base: splitLines(conflict.base), left: splitLines(conflict.ours), right: splitLines(conflict.theirs) }), [conflict.base, conflict.ours, conflict.theirs]);
  const baseSyntax = useMergeHighlight(conflict.file, sources.base);
  const leftSyntax = useMergeHighlight(conflict.file, sources.left);
  const rightSyntax = useMergeHighlight(conflict.file, sources.right);
  const syntax = { base: baseSyntax, left: leftSyntax, right: rightSyntax };
  const request = useMemo<MergeRequest>(() => ({ base: { kind: "text", text: conflict.base }, left: { kind: "text", text: conflict.ours }, right: { kind: "text", text: conflict.theirs } }), [conflict.base, conflict.ours, conflict.theirs]);
  // The merge comes from the core. Choices are kept by conflict ID, which is the same for the same files.
  const [outcome, setOutcome] = useState<MergeOutcome | null>(null), [problem, setProblem] = useState("");
  useEffect(() => {
    let alive = true;
    const ticket = ++tickets;
    setOutcome(null); setProblem("");
    api.mergeThreeWay(request, ticket)
      .then((found) => { if (alive) setOutcome(found); })
      .catch((e) => { if (alive) setProblem(String(e instanceof Error ? e.message : e)); });
    return () => { alive = false; void api.mergeCancel(ticket).catch(() => {}); };
  }, [request]);
  const model = outcome?.kind === "text" ? outcome.merge : null;
  const view = useMemo(() => (model ? overview(model, sources) : null), [model, sources]);
  const chunksById = useMemo(() => new Map(model?.chunks.map((chunk) => [chunk.id, chunk]) ?? []), [model]);
  const cards = view?.cards ?? [];
  const commonLines = useMemo(() => cards.map((card) => commonLineIndices(card.base.lines, card.left.lines, card.right.lines)), [view]); // eslint-disable-line react-hooks/exhaustive-deps
  const [picked, setPicked] = useState<Record<string, MergeChoice | undefined>>({});
  const [active, setActive] = useState(-1);
  const [manual, setManual] = useState<string | null>(null), [editing, setEditing] = useState(false);
  // The proposal of the local model for one conflict: asked for on request, shown, and taken only on Accept.
  const [proposal, setProposal] = useState<{ id: string; outcome: MergeAiOutcome | null } | null>(null);
  const proposalSeq = useRef(0), proposalRequest = useRef(0), proposalPending = useRef(false);
  // The gaps the user opened (by position) and the full-file view, which opens them all.
  const [opened, setOpened] = useState<Set<number>>(new Set()), [full, setFull] = useState(false);
  const root = useRef<HTMLDivElement>(null), pointer = useRef<{ x: number; y: number } | null>(null), gitKeys = useGitKeys();
  const stopProposal = () => { if (proposalPending.current) { proposalPending.current = false; void api.compareAiCancel(proposalRequest.current).catch(() => {}); } proposalSeq.current++; };
  const live = useMemo(() => (model ? compose(model, sources.base, picked) : null), [model, sources.base, picked]);
  const remaining = live?.unresolved.length ?? 0;
  useEffect(() => { setPicked((prev) => (Object.keys(prev).length ? {} : prev)); setManual(null); setEditing(false); setOpened(new Set()); setFull(false); stopProposal(); setProposal(null); root.current?.focus(); }, [conflict.file, conflict.base, conflict.ours, conflict.theirs]);
  // The proposal was made, and its syntax checked, for the choices of that moment: another choice ends it.
  useEffect(() => { if (proposal) { stopProposal(); setProposal(null); } }, [picked]); // eslint-disable-line react-hooks/exhaustive-deps
  const pickedNow = useRef(picked);
  pickedNow.current = picked;
  // Closing the editor ends a request that is still running: the model is never left working in the background.
  useEffect(() => () => { if (proposalPending.current) void api.compareAiCancel(proposalRequest.current).catch(() => {}); }, []);
  useEffect(() => { setActive(cards.length ? 0 : -1); }, [model]);
  useEffect(() => {
    const list = root.current?.querySelector<HTMLElement>(".gm-cards");
    const card = list?.querySelector<HTMLElement>(`[data-card="${active}"]`);
    if (!list || !card) return;
    const box = card.getBoundingClientRect(), viewport = list.getBoundingClientRect();
    if (box.top < viewport.top || box.top >= viewport.bottom) list.scrollTop += box.top - viewport.top;
  }, [active]);
  // Unsaved work: any chosen side or hand edit that has not been staged yet.
  const unsaved = manual !== null || Object.values(picked).some(Boolean);
  useEffect(() => { onDirty?.(unsaved); }, [unsaved, onDirty]);
  useEffect(() => () => onDirty?.(false), [onDirty]);
  const unresolved = manual === null ? remaining : hasMarkers(manual) ? 1 : 0;
  const releaseManual = async () => {
    if (manual === null) return true;
    if (live && manual !== live.text && !await A.confirm(tt("Discard manual edits?"), tt("Choosing a side replaces the text you edited by hand."), tt("Discard edits"), true)) return false;
    setManual(null); setEditing(false);
    return true;
  };
  const confirmBoth = async (affected: Card[]) => {
    if (!affected.some((card) => bothRepeatsLines(card.left.lines, card.right.lines))) return true;
    return A.confirm(tt("Both can repeat lines"), tt("Both adds the two complete versions one after the other. Lines present on both sides, including hidden lines, will appear twice. Review the result before confirming."), tt("Use both"));
  };
  const choose = async (index: number, choice: MergeChoice) => {
    const card = cards[index];
    if (busy || !card || (choice === "combine" && chunksById.get(card.id)?.combinedLines == null) || (choice === "both" && !await confirmBoth([card])) || !await releaseManual()) return;
    setPicked((current) => ({ ...current, [card.id]: choice })); setActive(index);
  };
  const chooseAll = async (choice: MergeChoice) => {
    if (busy || !model || (choice === "combine" && model.chunks.some((chunk) => chunk.kind === "conflict" && chunk.combinedLines == null)) || (choice === "both" && !await confirmBoth(cards)) || !await releaseManual()) return;
    setPicked(Object.fromEntries(model.conflicts.map((id) => [id, choice])));
  };
  const step = (direction: 1 | -1) => { const next = stepCard(active, direction, cards.length); if (next >= 0) setActive(next); };
  const toggleEdit = () => { if (busy || !live) return; if (!editing) { setManual((current) => current ?? live.text); setEditing(true); } else setEditing(false); };
  const propose = async (index: number) => {
    const card = cards[index];
    if (!card || !model || busy) return;
    stopProposal();
    const seq = ++proposalSeq.current;
    // An id from the same source as the explanation's, so that a cancel reaches this request and no other.
    proposalRequest.current = nextRequestId();
    proposalPending.current = true;
    setActive(index);
    setProposal({ id: card.id, outcome: null });
    const done = (outcome: MergeAiOutcome) => { if (seq === proposalSeq.current) { proposalPending.current = false; setProposal({ id: card.id, outcome }); } };
    try {
      const input = conflictInput(card);
      const revision = await api.mergeAiRevision(input);
      // The grammar is asked about the whole file only when no other conflict is open and the result is not hand-edited.
      const parts = composeWithProposal(model, sources.base, picked, card.id, []);
      const syntax = !parts.othersOpen && manual === null ? { fileName: conflict.file, before: parts.before, after: parts.after } : null;
      done(await api.mergeAiPropose(proposalRequest.current, input, revision, syntax));
    } catch (e) { done({ outcome: "failed", message: String(e instanceof Error ? e.message : e) }); }
  };
  const rejectProposal = () => { stopProposal(); setProposal(null); root.current?.focus(); };
  const acceptProposal = async () => {
    if (busy || !model || !proposal || proposal.outcome?.outcome !== "proposed") return;
    const card = cards.find((c) => c.id === proposal.id);
    if (!card) return;
    const made = proposal.outcome.proposal;
    // Taken only against the text it was made for: the revision of the conflict on screen is asked again.
    const pickedAtClick = picked;
    const current = await api.mergeAiRevision(conflictInput(card)).catch(() => null);
    // A choice made while the revision was asked for ends the proposal as well.
    if (pickedNow.current !== pickedAtClick) return;
    if (!canAccept(made, current)) { A.notify(tt("The conflict changed since this was proposed. Nothing was taken."), "warn"); return; }
    if (manual !== null && live && manual !== live.text && !await A.confirm(tt("Discard manual edits?"), tt("Taking the proposal replaces the text you edited by hand."), tt("Discard edits"), true)) return;
    // Into the editable result, and nothing else: saving or staging stays the separate Confirm.
    setManual(composeWithProposal(model, sources.base, picked, card.id, made.replacement).text);
    setEditing(false);
    setProposal(null);
    root.current?.focus();
    A.notify(tt("The proposal is in the result. Review it, then Confirm."), "info");
  };
  const confirm = async () => {
    if (busy || !model) return;
    if (manual !== null) {
      if (!hasMarkers(manual)) await onConfirm(manual);
      else A.notify(tt("The result still has conflict markers. Settle them, then Confirm."), "warn");
      return;
    }
    if (remaining) return;
    // The text that is staged is the core's: only a merge with every conflict decided is a result.
    try {
      const resolution = await api.mergeResolve(request, picked as Record<string, MergeChoice>, ++tickets);
      if (resolution.kind === "resolved") await onConfirm(resolution.text);
      else A.notify(tt("The merge changed. Review the conflicts again."), "warn");
    } catch (e) {
      if (onFailure) onFailure(String(e instanceof Error ? e.message : e));
      else A.fail(e);
    }
  };
  const gitMode = gitOperation !== undefined;
  const titles = gitMode ? {
    base: tt("Base / original version"),
    left: gitOperation === "rebase" ? tt("Ours / rebased so far") : tt("Current / ours (often called local)"),
    right: gitOperation === "rebase" ? tt("Theirs / replayed commit") : tt("Incoming / theirs (sometimes called remote)"),
  } : { base: tt("Base"), left: tt("Local"), right: tt("Remote") };
  const choiceLabel = (id: MergeChoice, fallback: string) => gitMode && id === "left" ? tt("Ours") : gitMode && id === "right" ? tt("Theirs") : tt(fallback);
  const allLabel = (id: MergeChoice, fallback: string) => gitMode && id === "left" ? tt("All ours") : gitMode && id === "right" ? tt("All theirs") : tt(fallback);
  const gapOpen = (index: number) => full || opened.has(index);
  const column = (name: "base" | "left" | "right", title: string, side: CardSide, card: Card, index: number) => {
    const empty = placeholder(name, side);
    const hidden = full ? new Set<number>() : commonLines[index][name];
    const highlight = syntax[name];
    const renderLines = (lines: string[], start: number) => highlight
      ? lines.map((line, i) => <Fragment key={i}>{colouredLine(line, highlight, start + i)}</Fragment>)
      : lines.join("");
    const pieces: { start: number; count: number; hidden: boolean }[] = [];
    side.lines.forEach((_, i) => {
      const isHidden = hidden.has(i), previous = pieces[pieces.length - 1];
      if (previous?.hidden === isHidden) previous.count++;
      else pieces.push({ start: i, count: 1, hidden: isHidden });
    });
    const chosen = picked[card.id] === name || (name !== "base" && (picked[card.id] === "both" || picked[card.id] === "combine"));
    return (
      <div className={`gm-col gm-choice gm-${name} gm-kind-${side.kind} ${chosen ? "picked" : ""}`} role="button" tabIndex={busy ? -1 : 0} aria-label={title} aria-describedby={`gm-${index}-${name}-content`} aria-pressed={chosen} aria-disabled={busy}
        onPointerDown={(event) => { pointer.current = { x: event.clientX, y: event.clientY }; }}
        onClick={(event) => { event.stopPropagation(); const start = pointer.current; pointer.current = null; if (!busy && (!start || Math.hypot(event.clientX - start.x, event.clientY - start.y) < 5)) void choose(index, name); }}
        onKeyDown={(event) => { if ((event.key === "Enter" || event.key === " ") && !event.ctrlKey && !event.metaKey && !event.altKey && !event.shiftKey) { event.preventDefault(); event.stopPropagation(); if (!busy) void choose(index, name); } }}>
        <h4>{title}<small>{tt("lines {range}", { range: lineRange(side.span) })}</small>{chosen && <strong className="gm-selected">✓ {tt("Selected")}</strong>}</h4>
        <pre id={`gm-${index}-${name}-content`}>
          {side.before.length > 0 && <span className="gm-ctx">{renderLines(side.before, side.span.start - side.before.length)}</span>}
          {empty ? <i className="gm-placeholder">{empty}{"\n"}</i> : pieces.map((piece, i) => piece.hidden
            ? <span key={i} className="gm-ctx">{tt("… {count} common lines hidden", { count: piece.count })}{"\n"}</span>
            : <span key={i} className="gm-lines">{renderLines(side.lines.slice(piece.start, piece.start + piece.count), side.span.start + piece.start)}</span>)}
          {side.after.length > 0 && <span className="gm-ctx">{renderLines(side.after, side.span.start + side.span.len)}</span>}
        </pre>
      </div>
    );
  };
  const gap = (g: Gap, index: number) => {
    if (!gapHasLines(g)) return null;
    const count = Math.max(g.hidden.base, g.hidden.left, g.hidden.right);
    const offsets = (["base", "left", "right"] as const).map((name) => {
      const previous = cards[index - 1]?.[name];
      return previous ? previous.span.start + previous.span.len + previous.after.length : 0;
    });
    if (!gapOpen(index)) {
      return <button key={`gap${index}`} className="gm-gap" disabled={busy} onClick={() => setOpened((current) => new Set(current).add(index))} title={tt("Show the lines that are left out here, in every pane")}>{tt("… {count} lines left out · show", { count })}</button>;
    }
    return (
      <div key={`gap${index}`} className="gm-gap-open">
        <button className="gm-gap" disabled={busy || full} onClick={() => setOpened((current) => { const next = new Set(current); next.delete(index); return next; })}>{tt("Hide these {count} lines", { count })}</button>
        {Math.max(g.lines.base.length, g.lines.left.length, g.lines.right.length) > WINDOW_FROM
          // A long stretch is windowed: only the rows in view exist, aligned in the three columns.
          ? <MergeWindow columns={[g.lines.base, g.lines.left, g.lines.right]} labels={[titles.base, titles.left, titles.right]} classes={["gm-base", "gm-left", "gm-right"]} syntax={[baseSyntax, leftSyntax, rightSyntax]} offsets={offsets} vim={vim} />
          : <div className="gm-row">
            {(["base", "left", "right"] as const).map((name, columnIndex) => <pre key={name} className={`gm-col gm-${name}`}>{syntax[name] ? g.lines[name].map((line, i) => <Fragment key={i}>{colouredLine(line, syntax[name], offsets[columnIndex] + i)}</Fragment>) : g.lines[name].join("")}</pre>)}
          </div>}
      </div>
    );
  };
  // A result of many lines is windowed as well, so that it is never one block of thousands of lines. By the number of
  // lines only: a few very long lines stay plain text, which scrolls sideways.
  const resultText = manual !== null ? manual : live ? live.text : "";
  const resultLines = useMemo(() => splitLines(resultText), [resultText]);
  const resultSyntax = useMergeHighlight(conflict.file, resultLines);
  const resultRows = useMemo(() => ((resultText.match(/\n/g)?.length ?? 0) > WINDOW_FROM * 2 ? textRows(resultText) : null), [resultText]);
  const resultView = () => (resultRows ? <MergeWindow columns={[resultRows]} labels={[tt("Result")]} classes={["gm-result-cell"]} syntax={[resultSyntax]} vim={vim} /> : <pre>{resultSyntax ? resultLines.map((line, i) => <Fragment key={i}>{colouredLine(line, resultSyntax, i)}</Fragment>) : resultText}</pre>);
  const status = manual !== null ? (editing ? tt("Editing result by hand") : tt("Result edited by hand"))
    : !outcome && !problem ? tt("Computing the merge…")
    : cards.length ? tt("{resolved} of {total} conflicts resolved · {remaining} left", { resolved: cards.length - remaining, total: cards.length, remaining })
    : model ? tt("No conflicts: the two sides merge cleanly") : "";
  const cannot = outcome && outcome.kind !== "text"
    ? tt("This file cannot be merged as text: {reason}", { reason: outcome.kind === "binary" ? tt("a version is binary") : outcome.reason === "tooLarge" ? tt("a version is too large") : tt("a version is missing") })
    : problem;
  useEffect(() => { if (cannot) onFailure?.(cannot); }, [cannot, onFailure]);
  return <div className="gm" ref={root} tabIndex={-1} aria-label={tt("Merge editor")} onKeyDown={(event) => {
    const target = event.target as HTMLElement, { key, on } = gitKeys.press(["merge"], event);
    if (key === "Ctrl+Enter") { event.preventDefault(); event.stopPropagation(); void confirm(); return; }
    if (target.closest("textarea,input")) {
      // Out of the text first; the editor itself is left with the next Esc.
      if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); setEditing(false); root.current?.focus(); }
      return;
    }
    if ((event.ctrlKey || event.metaKey || event.altKey) && (!on || key === gitKeyOf(event))) return;
    if (event.key === "ArrowDown" || (vim && key === "j")) { event.preventDefault(); event.stopPropagation(); step(1); }
    else if (event.key === "ArrowUp" || (vim && key === "k")) { event.preventDefault(); event.stopPropagation(); step(-1); }
    else if (on && active >= 0 && choices.some((c) => c.key === key)) { event.preventDefault(); event.stopPropagation(); void choose(active, choices.find((c) => c.key === key)!.id); }
    else if (on && key === "]") { event.preventDefault(); event.stopPropagation(); step(1); }
    else if (on && key === "[") { event.preventDefault(); event.stopPropagation(); step(-1); }
    else if (on && key === "e") { event.preventDefault(); event.stopPropagation(); toggleEdit(); }
    else if (on && key === "p" && active >= 0 && !editing && !(proposal && !proposal.outcome)) { event.preventDefault(); event.stopPropagation(); void propose(active); }
    else if (event.key === "Escape" && proposal) { event.preventDefault(); event.stopPropagation(); rejectProposal(); }
  }}>
    <div className="gm-bar">
      <span className="gm-status" role="status">{status}{cards.length > 0 && manual === null && active >= 0 ? ` · ${tt("Conflict {current}/{total}", { current: active + 1, total: cards.length })}` : ""}</span>
      <div className="gm-actions">
        {!editing && cards.length > 1 && <>
          <button disabled={busy} onClick={() => step(-1)} title={tt("Previous conflict")}>↑<GitKeyCap scope="merge" k="[" /></button><button disabled={busy} onClick={() => step(1)} title={tt("Next conflict")}>↓<GitKeyCap scope="merge" k="]" /></button>
          {choices.slice(0, 3).map((c) => <button key={c.id} disabled={busy || (c.id === "combine" && model?.chunks.some((chunk) => chunk.kind === "conflict" && chunk.combinedLines == null))} onClick={() => void chooseAll(c.id)}>{allLabel(c.id, c.allLabel)}</button>)}
        </>}
        {cards.length > 0 && <button disabled={busy} aria-pressed={full} onClick={() => setFull((value) => !value)} title={tt("Show every line of the three versions, not only the conflicts")}>{tt("Full file")}</button>}
        <button disabled={busy || !live} aria-pressed={editing} onClick={toggleEdit}>{editing ? tt("Done editing") : `${tt("Edit")}${gitKeys.cap("merge", "e") ? ` · ${gitKeys.cap("merge", "e")}` : ""}`}</button>{manual !== null && !editing && <button disabled={busy} onClick={() => void releaseManual()}>{tt("Discard edits")}</button>}
        <button disabled={busy} onClick={onOpenExternal} title={tt("Open the working file in the external editor")}>{tt("Open file")}</button>
        <button className="gm-confirm" disabled={busy || !model || unresolved > 0} onClick={() => void confirm()} title={gitKeys.cap("merge", "Ctrl+Enter") || platformLabel("Ctrl+Enter")}>{tt("Confirm")}</button>
      </div>
    </div>
    {cannot && <p className="gm-note" role="alert">{cannot}</p>}
    {!full && commonLines.some(({ base, left, right }) => base.size || left.size || right.size) &&
      <p className="gm-hint">{tt("Selecting a side keeps its full conflict block, including hidden common lines.")}</p>}
    {gitOperation === "rebase" && <p className="gm-hint">{tt("During rebase, ours is the target branch with any commits already replayed; theirs is the commit being replayed. Local and remote can mean different things.")}</p>}
    <section className={`gm-pane gm-result ${editing ? "editing" : ""}`} aria-label={tt("Result")}>
      <h3>{tt("Result")}<small>{editing ? tt("editable") : manual !== null ? tt("edited by hand") : tt("live preview")}</small></h3>
      {editing && manual !== null ? <textarea aria-label={tt("Resolved file")} spellCheck={false} autoFocus value={manual} onChange={(event) => setManual(event.target.value)} /> : manual === null && remaining > 0 ? <p className="gm-result-hint">{tt("Choose a version for each conflict to preview the result.")}</p> : resultView()}
    </section>
    {proposal && (() => {
      const card = cards.find((c) => c.id === proposal.id);
      return card ? <MergeProposalPanel sideLabels={titles} outcome={proposal.outcome} left={card.left.lines.map((l) => l.replace(/\r?\n$/, ""))} right={card.right.lines.map((l) => l.replace(/\r?\n$/, ""))} base={card.base.lines.map((l) => l.replace(/\r?\n$/, ""))} othersOpen={cards.some((c) => c.id !== card.id && !picked[c.id])} busy={busy} onAccept={() => void acceptProposal()} onReject={rejectProposal} onCancel={() => { const was = proposal; stopProposal(); setProposal({ id: was.id, outcome: { outcome: "cancelled" } }); }} /> : null;
    })()}
    {model && cards.length > 0 && view && <section className="gm-pane gm-cards" aria-label={tt("Conflicts")}>
      {view.gaps.map((g, index) => (
        <div key={index} className="gm-seq">
          {gap(g, index)}
          {cards[index] && <div className={`gm-card ${active === index ? "active" : ""} ${picked[cards[index].id] ? "decided" : "open"}`} data-card={index} onClick={() => setActive(index)}>
            <div className="gm-toolbar">
              <b>{tt("Conflict {current}/{total}", { current: cards[index].number, total: cards.length })}</b>
              <button disabled={busy || (!!proposal && !proposal.outcome)} aria-label={tt("Propose a resolution for conflict {current}", { current: cards[index].number })} onClick={(event) => { event.stopPropagation(); void propose(index); }} title={tt("Ask the local model you have set up for a resolution of this conflict (p). Nothing is taken until you accept it.")}>{tt("Propose…")}<GitKeyCap scope="merge" k="p" /></button>
              {choices.map((c) => <button key={c.id} disabled={busy || (c.id === "combine" && chunksById.get(cards[index].id)?.combinedLines == null)} aria-pressed={picked[cards[index].id] === c.id} onClick={(event) => { event.stopPropagation(); void choose(index, c.id); }} title={c.id === "combine" ? tt("Combine compatible changes. Insertions at the same position use the left side first; review the result before confirming.") : c.key && gitKeys.cap("merge", c.key) ? tt("Key {key}", { key: gitKeys.cap("merge", c.key) }) : undefined}>{choiceLabel(c.id, c.label)}{c.key && <GitKeyCap scope="merge" k={c.key} />}</button>)}
            </div>
            {picked[cards[index].id] === "combine" && <p className="gm-ending-note" role="note">{tt("Combined insertions at the same position are ordered left first, then right. Review the result before confirming.")}</p>}
            {chunksById.get(cards[index].id)?.combinedLines == null && <p className="gm-ending-note" role="note">{tt("Changes cannot be combined automatically. Choose a version or edit the result.")}</p>}
            {!full && commonLines[index].endingsDiffer && <p className="gm-ending-note" role="note">{tt("Some hidden matching lines have different line endings; the selected version keeps its own endings.")}</p>}
            <div className="gm-row">
              {column("left", titles.left, cards[index].left, cards[index], index)}
              {column("right", titles.right, cards[index].right, cards[index], index)}
              {column("base", titles.base, cards[index].base, cards[index], index)}
            </div>
          </div>}
        </div>
      ))}
    </section>}
  </div>;
}
