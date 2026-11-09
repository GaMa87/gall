import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import * as A from "../state/actions";
import { chosenLines, composeResult, conflictMarkers, mergeThreeWay, type MergeChoice } from "../core/gitMerge";
import "./GitMerge.css";

type Side = { file: string; base: string; ours: string; theirs: string; result: string };
const choices: { id: MergeChoice; label: string; key: string }[] = [
  { id: "ours", label: "Ours", key: "o" },
  { id: "theirs", label: "Theirs", key: "t" },
  { id: "both", label: "Both", key: "b" },
  { id: "base", label: "Base", key: "B" },
];
const hasMarkers = (text: string) => /^(<{7}|={7}|>{7})( |$)/m.test(text);

export function GitMergeEditor({ conflict, vim, busy, onConfirm, onOpenExternal }: { conflict: Side; vim: boolean; busy: boolean; onConfirm: (content: string) => Promise<void>; onOpenExternal: () => void }) {
  const chunks = useMemo(() => mergeThreeWay(conflict.base, conflict.ours, conflict.theirs), [conflict.base, conflict.ours, conflict.theirs]);
  const conflictIds = useMemo(() => chunks.flatMap((chunk, index) => (chunk.conflict ? [index] : [])), [chunks]);
  const [picked, setPicked] = useState<Record<number, MergeChoice | undefined>>({});
  const [active, setActive] = useState(conflictIds[0] ?? -1);
  const [manual, setManual] = useState<string | null>(null), [editing, setEditing] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const live = useMemo(() => composeResult(chunks, picked), [chunks, picked]);
  useEffect(() => { setPicked({}); setManual(null); setEditing(false); setActive(conflictIds[0] ?? -1); root.current?.focus(); }, [conflict.file, conflict.base, conflict.ours, conflict.theirs, conflictIds]);
  useEffect(() => { root.current?.querySelectorAll(`[data-chunk="${active}"]`).forEach((element) => element.scrollIntoView({ block: "nearest" })); }, [active]);
  const unresolved = manual === null ? live.unresolved : hasMarkers(manual) ? 1 : 0;
  const releaseManual = async () => {
    if (manual === null) return true;
    if (manual !== live.text && !await A.confirm("Discard manual edits?", "Choosing a side replaces the text you edited by hand.", "Discard edits", true)) return false;
    setManual(null); setEditing(false);
    return true;
  };
  const choose = async (index: number, choice: MergeChoice) => { if (busy || !await releaseManual()) return; setPicked((current) => ({ ...current, [index]: choice })); setActive(index); };
  const chooseAll = async (choice: MergeChoice) => { if (busy || !await releaseManual()) return; setPicked(Object.fromEntries(conflictIds.map((index) => [index, choice]))); };
  const step = (direction: number) => { if (!conflictIds.length) return; const at = conflictIds.indexOf(active); setActive(conflictIds[(Math.max(at, 0) + direction + conflictIds.length) % conflictIds.length]); };
  const toggleEdit = () => { if (busy) return; if (!editing) { setManual((current) => current ?? live.text); setEditing(true); } else setEditing(false); };
  const confirm = async () => {
    if (busy) return;
    const text = manual ?? live.text;
    if (manual === null && live.unresolved) return;
    if (manual !== null && hasMarkers(manual) && !await A.confirm("Conflict markers remain", "The result still contains <<<<<<<, ======= or >>>>>>> lines. Stage it anyway?", "Stage anyway", true)) return;
    await onConfirm(text);
  };
  const pane = (name: "ours" | "theirs" | "base", title: string, hint: string) => (
    <section className={`gm-pane gm-${name}`} aria-label={title}>
      <h3>{title}<small>{hint}</small></h3>
      <pre>{chunks.map((chunk, index) => {
        const lines = chunk[name];
        if (!lines.length && !chunk.conflict) return null;
        const differs = name !== "base" && lines.join("") !== chunk.base.join("");
        return <span key={index} data-chunk={chunk.conflict ? index : undefined} className={`gm-chunk ${chunk.conflict ? "conflict" : differs ? "changed" : ""} ${active === index ? "active" : ""} ${chunk.conflict && (picked[index] === name || picked[index] === "both" && name !== "base") ? "picked" : ""}`} title={chunk.conflict ? `Click to use ${name}` : undefined} onClick={() => { if (chunk.conflict) void choose(index, name); }}>{lines.join("") || (chunk.conflict ? "∅ (nothing)\n" : "")}</span>;
      })}</pre>
    </section>
  );
  const resolvedClass = (choice: MergeChoice | undefined) => (choice ? `resolved-${choice}` : "unresolved");
  return <div className="gm" ref={root} tabIndex={-1} aria-label="Merge editor" onKeyDown={(event) => {
    const target = event.target as HTMLElement;
    if ((event.ctrlKey || event.metaKey) && event.key === "Enter") { event.preventDefault(); event.stopPropagation(); void confirm(); return; }
    if (target.closest("textarea,input") || event.ctrlKey || event.metaKey || event.altKey) return;
    if (event.key === "ArrowDown" || (vim && event.key === "j")) { event.preventDefault(); event.stopPropagation(); step(1); }
    else if (event.key === "ArrowUp" || (vim && event.key === "k")) { event.preventDefault(); event.stopPropagation(); step(-1); }
    else if (vim && active >= 0 && choices.some((c) => c.key === event.key)) { event.preventDefault(); event.stopPropagation(); void choose(active, choices.find((c) => c.key === event.key)!.id); }
    else if (vim && event.key === "e") { event.preventDefault(); event.stopPropagation(); toggleEdit(); }
  }}>
    <div className="gm-bar">
      <span className="gm-status" role="status">{manual !== null ? editing ? "Editing result by hand" : "Result edited by hand" : conflictIds.length ? `${conflictIds.length - live.unresolved} of ${conflictIds.length} conflicts resolved` : "No overlapping changes"}</span>
      <div className="gm-actions">
        {!editing && conflictIds.length > 1 && <>
          <button disabled={busy} onClick={() => step(-1)} title="Previous conflict">↑</button><button disabled={busy} onClick={() => step(1)} title="Next conflict">↓</button>
          {choices.slice(0, 3).map((c) => <button key={c.id} disabled={busy} onClick={() => void chooseAll(c.id)}>All {c.label.toLowerCase()}</button>)}
        </>}
        <button disabled={busy} aria-pressed={editing} onClick={toggleEdit}>{editing ? "Done editing" : `Edit${vim ? " · e" : ""}`}</button>{manual !== null && !editing && <button disabled={busy} onClick={() => void releaseManual()}>Discard edits</button>}
        <button disabled={busy} onClick={onOpenExternal} title="Open the working file in the external editor">Open file</button>
        <button className="gm-confirm" disabled={busy || unresolved > 0 && manual === null} onClick={() => void confirm()} title="Ctrl+Enter">Confirm</button>
      </div>
    </div>
    <section className="gm-pane gm-result" aria-label="Result">
      <h3>Result<small>{editing ? "editable" : manual !== null ? "edited by hand" : "live preview"}</small></h3>
      {editing && manual !== null ? <textarea aria-label="Resolved file" spellCheck={false} autoFocus value={manual} onChange={(event) => setManual(event.target.value)} /> : manual !== null ? <pre><span className="gm-chunk">{manual}</span></pre> : <pre>{chunks.map((chunk, index) => {
        if (!chunk.conflict) return <span key={index} className="gm-chunk">{chunk.merged.join("")}</span>;
        const choice = picked[index], lines = chosenLines(chunk, choice) ?? conflictMarkers(chunk);
        return <Fragment key={index}>
          <span className={`gm-toolbar ${active === index ? "active" : ""}`} data-chunk={index}>
            <b>Conflict {conflictIds.indexOf(index) + 1}/{conflictIds.length}</b>
            {choices.map((c) => <button key={c.id} disabled={busy} aria-pressed={choice === c.id} onClick={() => void choose(index, c.id)} title={vim ? `Key ${c.key}` : undefined}>{c.label}</button>)}
          </span>
          <span className={`gm-chunk conflict ${resolvedClass(choice)} ${active === index ? "active" : ""}`} onClick={() => setActive(index)}>{lines.join("") || "(empty)\n"}</span>
        </Fragment>;
      })}</pre>}
    </section>
    <div className="gm-sides">{pane("ours", "Ours", "Git current side")}{pane("theirs", "Theirs", "Git incoming side")}</div>
    {pane("base", "Base", "common ancestor")}
  </div>;
}
