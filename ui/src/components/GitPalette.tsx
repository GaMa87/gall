import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { alwaysPicks, gitPickers, hasCandidate, paletteCommands, pickCandidates, rankCandidates, sensibleCandidates, rankCommands, recentCommands, rememberCommand, type Candidate, type CandidateSource, type PaletteCommand, type PaletteState, type RankedCommand } from "../core/gitPalette";
import { gitShownKey, type GitScope } from "../core/gitCommands";
import { HoverGuard } from "../core/hover";
import { platformLabel } from "../core/keymap";
import { useGitKeys } from "../state/gitKeys";
import { tt } from "../i18n";
import "./GitPalette.css";

type Props = {
  /** The panel in use: its commands come first. */
  scope: GitScope;
  source: CandidateSource;
  current: { branch: string | null; head: string };
  /** Whether the command can run on what is selected now. */
  canRun: (item: PaletteCommand) => boolean;
  onRun: (item: PaletteCommand, target?: Candidate) => void;
  onClose: () => void;
};
const kindLabels: Record<Candidate["kind"], string> = { commit: "commit", branch: "branch", remote: "remote branch", tag: "tag", stash: "stash", remoteRepository: "remote", conflict: "conflict" };
const MAX_ROWS = 200;

function Marked({ text, positions }: { text: string; positions: number[] }) {
  if (!positions.length) return <>{text}</>;
  const marked = new Set(positions);
  return <>{[...text].map((c, i) => marked.has(i) ? <b key={i}>{c}</b> : c)}</>;
}

/** `open` with an action starts in the choice of its target. */
export type PaletteHandle = { open: (action?: string) => void; close: () => void; isOpen: () => boolean };
/** Holds whether the palette is open, so opening it does not render the workspace (thousands of commit rows) again. */
export function GitPaletteHost({ handle, ...props }: Props & { handle: React.RefObject<PaletteHandle | null> }) {
  const [open, setOpen] = useState<{ action?: string } | null>(null);
  const opened = useRef(open); opened.current = open;
  useEffect(() => {
    handle.current = { open: (action) => setOpen({ action }), close: () => setOpen(null), isOpen: () => !!opened.current };
    return () => { handle.current = null; };
  }, [handle]);
  return open ? <GitPalette {...props} start={open.action} /> : null;
}

/** All Git commands by name (`:` or Ctrl+Shift+P); a command with a parameter asks for it in a second step. */
function GitPalette({ scope, source, current, canRun, onRun, onClose, start }: Props & { start?: string }) {
  const { shortcuts, vim } = useGitKeys();
  const [query, setQuery] = useState(""), [index, setIndex] = useState(0), [step, setStep] = useState<RankedCommand | null>(() => {
    const item = start ? paletteCommands(scope, shortcuts).find((entry) => entry.command.action === start) : undefined;
    return item ? { ...item, state: "pick", label: tt(item.command.label), positions: [] } : null;
  });
  const input = useRef<HTMLInputElement>(null), list = useRef<HTMLDivElement>(null);
  const commands = useMemo(() => paletteCommands(scope, shortcuts), [scope, shortcuts]);
  const targetsOf = (action: string) => sensibleCandidates(action, pickCandidates(gitPickers[action] ?? [], source), current);
  const canPick = (item: PaletteCommand) => hasCandidate(item.command.action, source, current);
  const states = useMemo(() => new Map<string, PaletteState>(commands.map((item) => {
    const able = canRun(item);
    // A branch to cherry-pick to is no use without a commit to bring there.
    if (item.command.action === "cherryPickTo") return [item.command.id, able && canPick(item) ? "pick" : "off"];
    return [item.command.id, able && !alwaysPicks(item.command.action) ? "run" : canPick(item) ? "pick" : able ? "run" : "off"];
    // The selection behind the palette cannot change while it is open.
  })), [commands, source]); // eslint-disable-line react-hooks/exhaustive-deps
  const rows = useMemo(() => step ? [] : rankCommands(commands, query, { stateOf: (item) => states.get(item.command.id) ?? "off", recent: recentCommands(), current: scope, translate: tt }), [commands, states, query, scope, step]);
  const candidates = useMemo(() => step ? targetsOf(step.command.action) : [], [step, source]); // eslint-disable-line react-hooks/exhaustive-deps
  const targets = useMemo(() => step ? rankCandidates(candidates, query, MAX_ROWS) : [], [step, candidates, query]);
  const count = step ? targets.length : rows.length, active = Math.min(index, Math.max(0, count - 1));
  const mounted = useRef(false);
  useEffect(() => { input.current?.focus(); }, [step]);
  // Initial mount starts at scrollTop 0 with the first item active; avoid forcing synchronous layout across a 2000-row graph behind the backdrop.
  useLayoutEffect(() => {
    if (!mounted.current) { mounted.current = true; return; }
    list.current?.querySelector(".on")?.scrollIntoView({ block: "nearest" });
  }, [active, step, query]);
  // Opened with a key: a mouse at rest chooses nothing, also when the list moves under it.
  const guard = useRef<HoverGuard | null>(null);
  const hover = (e: React.MouseEvent, at: number) => { if ((guard.current ??= new HoverGuard(false)).moved(e) && at !== index) setIndex(at); };
  const enterStep = (item: RankedCommand) => { setStep(item); setQuery(""); setIndex(0); };
  const finish = (item: RankedCommand, target?: Candidate) => { rememberCommand(item.command.id); onRun(item, target); };
  const accept = (at: number, other = false) => {
    if (step) { const target = targets[at]; if (target) finish(step, target); return; }
    const row = rows[at];
    if (!row || row.state === "off") return;
    // Tab names another target although the selection would do.
    if (row.state === "pick" || (other && canPick(row))) enterStep(row); else if (!other) finish(row);
  };
  const onKeyDown = (e: React.KeyboardEvent) => {
    // Nothing typed here reaches the workspace behind it.
    e.stopPropagation();
    if (e.nativeEvent.isComposing) return;
    const down = e.key === "ArrowDown" || (e.ctrlKey && (e.key === "n" || e.key === "j")), up = e.key === "ArrowUp" || (e.ctrlKey && (e.key === "p" || e.key === "k") && !e.shiftKey);
    if (e.key === "Escape") { e.preventDefault(); if (step && !start) { setStep(null); setQuery(""); setIndex(0); } else onClose(); }
    else if (e.key === "Backspace" && step && !start && !query) { e.preventDefault(); setStep(null); setIndex(0); }
    else if (down || up) { e.preventDefault(); if (count) setIndex((active + (down ? 1 : -1) + count) % count); }
    else if (e.key === "PageDown" || e.key === "PageUp") { e.preventDefault(); setIndex(Math.max(0, Math.min(active + (e.key === "PageDown" ? 10 : -10), count - 1))); }
    else if (e.key === "Enter") { e.preventDefault(); accept(active); }
    else if (e.key === "Tab") { e.preventDefault(); if (!step) accept(active, true); }
  };
  return <div className="gp-backdrop" onMouseDown={(e) => { e.stopPropagation(); onClose(); }} onContextMenu={(e) => e.preventDefault()}>
    <div className="gp" role="dialog" aria-label={tt("Git commands")} onMouseDown={(e) => e.stopPropagation()} onKeyDown={onKeyDown}>
      <div className="gp-input">
        {step && <span className="gp-step" title={tt("Backspace goes back to the commands")}>{step.label.replace(/…$/, "").trim()}</span>}
        <input ref={input} role="combobox" aria-expanded="true" aria-controls="gp-list" aria-activedescendant={count ? `gp-${active}` : undefined} aria-label={step ? tt("Choose a target") : tt("Search Git commands")} spellCheck={false} value={query} placeholder={step ? tt("Choose a target…") : tt("Type a Git command…")} onChange={(e) => { setQuery(e.target.value); setIndex(0); }} />
      </div>
      <div className="gp-list" id="gp-list" role="listbox" ref={list}>
        {step ? targets.map((item, i) => <div key={item.key} id={`gp-${i}`} role="option" aria-selected={i === active} className={`gp-row ${i === active ? "on" : ""}`} onMouseMove={(e) => hover(e, i)} onClick={() => accept(i)}>
          <span className="gp-label"><Marked text={item.label} positions={item.positions} /></span><small className="gp-detail">{item.detail}</small><small className="gp-scope">{tt(kindLabels[item.kind])}</small>
        </div>) : rows.map((row, i) => { const key = platformLabel(gitShownKey(row.command, vim)); return <div key={row.command.id} id={`gp-${i}`} role="option" aria-selected={i === active} aria-disabled={row.state === "off"} className={`gp-row ${i === active ? "on" : ""} ${row.state === "off" ? "off" : ""} ${row.command.danger ? "danger" : ""}`} title={row.state === "off" ? tt("Not possible with the current selection") : undefined} onMouseMove={(e) => hover(e, i)} onClick={() => accept(i)}>
          <span className="gp-label"><Marked text={row.label} positions={row.positions} /></span>{row.state === "pick" && <small className="gp-detail">{tt("choose a target")}</small>}<small className="gp-scope">{tt(row.scopeLabel)}</small>{key && <kbd>{key === "Space" ? tt("Space") : key}</kbd>}
        </div>; })}
        {!count && <p className="gp-empty">{step ? tt("No matching target") : tt("No matching Git command")}</p>}
      </div>
      <div className="gp-foot"><span>{step ? tt("{count} targets", { count: candidates.length }) : tt("{count} commands", { count: rows.length })}{step && targets.length >= MAX_ROWS ? ` · ${tt("first {count} shown", { count: MAX_ROWS })}` : ""}</span><span><kbd>↵</kbd> {tt("run")} {!step && <><kbd>{tt("Tab")}</kbd> {tt("choose a target")} </>}<kbd>Esc</kbd> {step && !start ? tt("back") : tt("close")}</span></div>
    </div>
  </div>;
}
