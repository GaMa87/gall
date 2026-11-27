import { ThemedSelect } from "./ThemedSelect";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { gitKeyOf } from "../core/gitCommands";
import { useGitKeys } from "../state/gitKeys";
import { GitKeyCap } from "./GitKeyCap";
import * as A from "../state/actions";
import "./GitRebase.css";
import { platformLabel } from "../core/keymap";
import { tt } from "../i18n";
import { EXEC_PREFIX, ROOT_BASE, amendMessage, autosquash, execProblem, listWindow, type RebaseAction } from "../core/gitRebase";
import { commitMessages } from "../state/gitMessages";
import { createProgressId } from "../core/gitProgress";
import { GitProgressBar, useGitProgress, useLocalProgress } from "./GitProgressBar";

type Action = RebaseAction;
type Commit = { id: string; subject: string; message: string };
type Plan = { head: string; base: string; token: string; skippedMerges: number; commits: Omit<Commit, "message">[] };
type Entry = Commit & { action: Action };
type Conflicts = Record<string, string[]>;
const actions: Action[] = ["pick", "reword", "squash", "fixup", "drop", "edit", "exec"];
// An exec line is added with `x` and has no action to choose.
const choices = actions.filter((action) => action !== "exec");
/** What the backend takes: an exec line has no commit ID, and its command is the message. */
const wire = ({ id, action, message }: Entry) => ({ id: action === "exec" ? "" : id, action, message });
export function GitRebase({ path, commit, onto = false, onDirty, onBusy, onChanged, onClose }: { path: string; commit?: string; onto?: boolean; onDirty?: (dirty: boolean) => void; onBusy: (busy: boolean) => void; onChanged: () => void; onClose: () => void }) {
  const [base, setBase] = useState("");
  const [plan, setPlan] = useState<Plan | null>(null);
  const [entries, setEntries] = useState<Entry[]>([]);
  const [selected, setSelected] = useState(0);
  const [error, setError] = useState("");
  const [result, setResult] = useState("");
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [help, setHelp] = useState(false);
  const [dragOver, setDragOver] = useState<number | null>(null);
  const [dragging, setDragging] = useState<number | null>(null);
  // Commits that would conflict in the current order; null while unknown (loading, not checked or Git too old).
  const [conflicts, setConflicts] = useState<Conflicts | null>(null);
  const [checking, setChecking] = useState(false);
  const [checkProgressId, setCheckProgressId] = useState<string | null>(null);
  const [runProgressId, setRunProgressId] = useState<string | null>(null);
  const checkTicket = useRef(0);
  const checkProgress = useGitProgress(checkProgressId, { operation: "rebaseConflicts", phase: "Checking rebase conflicts", cancellable: true });
  const runProgress = useGitProgress(runProgressId, { operation: "rebase", phase: "Rebasing", cancellable: false });
  const loadProgress = useLocalProgress(loading, "Preparing the history range…");
  const root = useRef<HTMLDivElement>(null);
  const latestPlan = useRef(plan); latestPlan.current = plan;
  const ticket = useRef(0);
  const gate = useRef(false);
  const dragged = useRef<number | null>(null);
  const gitKeys = useGitKeys(), { vim } = gitKeys;
  const load = useCallback(async (target: string) => {
    const current = ++ticket.current;
    setLoading(true); setError(""); setResult(""); setPlan(null); setEntries([]);
    try {
      const value = await invoke<Plan>("git_rebase_plan", { path, base: target });
      if (current === ticket.current) { setPlan(value); setEntries(value.commits.map((entry) => ({ ...entry, message: "", action: "pick" }))); setSelected(0); }
    } catch (e) { if (current === ticket.current) setError(String(e)); }
    finally { if (current === ticket.current) setLoading(false); }
  }, [path]);
  useEffect(() => {
    let active = true;
    if (commit) {
      const current = ++ticket.current;
      setLoading(true); setPlan(null); setEntries([]); setError("");
      if (onto) { setBase(commit); void load(commit); return () => { active = false; ticket.current++; }; }
      invoke<{ parents: string[] }>("git_commit_details", { path, commit, base: null }).then((details) => {
        if (!active || current !== ticket.current) return;
        const parent = details.parents[0];
        // The root commit has no parent: the plan then starts from the root (git rebase -i --root).
        const from = parent ?? ROOT_BASE;
        setBase(from); void load(from);
      }).catch((error) => { if (active && current === ticket.current) { setError(String(error)); setLoading(false); } });
    }
    root.current?.focus();
    return () => { active = false; ticket.current++; };
  }, [commit, load, onto, path]);
  // Unsaved work: a plan that differs from the untouched list (an action other than pick, or a new order).
  const unsaved = !!plan && entries.some((entry, i) => entry.action !== "pick" || entry.id !== plan.commits[i]?.id);
  useEffect(() => { onDirty?.(unsaved); }, [unsaved, onDirty]);
  useEffect(() => () => onDirty?.(false), [onDirty]);
  // Long plans draw only the part on screen; the newest commit is on top.
  const list = useRef<HTMLDivElement>(null), [scroll, setScroll] = useState({ top: 0, height: 800 }), [itemHeight, setItemHeight] = useState(76);
  const shown = listWindow(entries.length, scroll.top, scroll.height, itemHeight);
  useLayoutEffect(() => {
    const el = list.current;
    if (!el || entries.length <= 200) return;
    const cards = el.querySelectorAll(".grw-entry");
    if (cards.length > 10) {
      // Estimated once for a plan: changing it while scrolling would move the list under the pointer.
      const height = Math.round((cards[cards.length - 1].getBoundingClientRect().bottom - cards[0].getBoundingClientRect().top) / cards.length);
      setItemHeight((old) => old === 76 && height > 20 ? height : old);
    }
  }, [plan, entries.length]);
  useEffect(() => {
    const el = list.current, card = root.current?.querySelector(".grw-entry.selected");
    if (card) card.scrollIntoView({ block: "nearest" });
    else if (el) el.scrollTop = Math.max(0, (entries.length - 1 - selected) * itemHeight - el.clientHeight / 2);
    // Only a new selection scrolls, not a new length.
  }, [selected]); // eslint-disable-line react-hooks/exhaustive-deps
  const updateAction = (index: number, action: Action) => {
    const changed = entries[index];
    if (!changed) return;
    setEntries((current) => current.map((entry, i) => i === index ? { ...entry, action, message: action === "squash" && entry.action !== "squash" ? "" : entry.message } : entry));
    if (action !== "reword" || changed.message) return;
    // The message is asked for now, and only fills an editor that is still empty.
    commitMessages(path, [changed.id]).then((found) => setEntries((current) => current.map((entry) => entry.id === changed.id && entry.action === "reword" && !entry.message ? { ...entry, message: found.get(changed.id) ?? "" } : entry))).catch((e) => setError(String(e)));
  };
  const move = (index: number, direction: number) => {
    const target = index + direction;
    if (target < 0 || target >= entries.length || busy) return;
    setEntries((current) => { const next = [...current]; [next[index], next[target]] = [next[target], next[index]]; return next; });
    setSelected(target);
  };
  const drop = (target: number) => {
    const source = dragged.current; dragged.current = null;
    if (source === null || source === target || busy) return;
    setEntries((current) => { const next = [...current]; const [entry] = next.splice(source, 1); next.splice(target, 0, entry); return next; }); setSelected(target);
  };
  // Replays the current order in memory (no files change) so a conflict shows up as soon as a commit is moved or dropped.
  const orderKey = useMemo(() => entries.map((entry) => `${entry.id}:${entry.action}`).join(","), [entries]);
  // Where a commit was in the plan, and where it is now among the commits.
  const original = useMemo(() => new Map((plan?.commits ?? []).map((entry, i) => [entry.id, i])), [plan]);
  const positions = useMemo(() => { let at = 0; return entries.map((entry) => entry.action === "exec" ? at : at++); }, [entries]);
  useEffect(() => {
    const current = (checkTicket.current = Math.max(checkTicket.current + 1, Date.now()));
    setConflicts(null);
    if (!plan || !entries.length) { setChecking(false); setCheckProgressId(null); return; }
    const progressId = createProgressId("rebase-check");
    setChecking(true);
    setCheckProgressId(progressId);
    const timer = window.setTimeout(() => {
      invoke<{ id: string; files: string[] }[]>("git_rebase_conflicts", { path, base: plan.base, entries: entries.filter((entry) => entry.action !== "exec").map(({ id, action }) => ({ id, action, message: "" })), ticket: current, progressId })
        .then((found) => { if (current === checkTicket.current) setConflicts(Object.fromEntries(found.map((item) => [item.id, item.files]))); })
        .catch(() => { if (current === checkTicket.current) setConflicts(null); })
        .finally(() => { if (current === checkTicket.current) { setChecking(false); setCheckProgressId(null); } });
    }, 120);
    return () => { window.clearTimeout(timer); invoke("git_rebase_conflicts_cancel", { ticket: current }).catch(() => {}); };
    // The messages do not matter for conflicts, so typing one does not trigger a new check.
  }, [plan, orderKey, path]);
  const run = async () => {
    if (gate.current || !plan || !entries.length) return;
    gate.current = true; setBusy(true); onBusy(true); setError(""); setResult("");
    try {
      const problem = entries.filter((entry) => entry.action === "exec").map((entry) => execProblem(entry.message)).find(Boolean);
      if (problem) { setError(tt(problem === "empty" ? "An exec line needs a command." : problem === "line" ? "An exec command must be a single line." : "An exec command is too long.")); return; }
      const commands = entries.filter((entry) => entry.action === "exec").map((entry) => entry.message.trim());
      const counts = actions.map((action) => `${entries.filter((entry) => entry.action === action).length} ${action}`).filter((value) => !value.startsWith("0 "));
      if (!await A.confirm(tt("Rewrite this commit range?"), tt("Replay {length} commits after {value} through {value2}.{value3}\n\n{counts}\n\nThis changes commit IDs and may conflict. Avoid rewriting shared history. Gall retains the original HEAD as a recovery reference. Working files must be clean; commits are unsigned and repository hooks are disabled.", { length: entries.length, value: plan.base.slice(0, 12), value2: plan.head.slice(0, 12), value3: (plan.skippedMerges ? tt(plan.skippedMerges === 1 ? "\n\n{count} merge commit is flattened: its commits are replayed linearly and conflict resolutions made in the merge are not kept." : "\n\n{count} merge commits are flattened: their commits are replayed linearly and conflict resolutions made in the merges are not kept.", { count: plan.skippedMerges }) : "") + (commands.length ? `\n\n${tt("These commands run in a shell, in this order, after the commits before them:")}\n${commands.join("\n")}` : ""), counts: counts.join(" · ") }), tt("Start interactive rebase"), true)) return;
      const progressId = createProgressId("rebase-run");
      setRunProgressId(progressId);
      const output = await invoke<string>("git_rebase_execute", { path, base: plan.base, expected: plan.token, entries: entries.map(wire), progressId });
      setResult(output || tt("Rebase completed.")); onChanged();
      setPlan(null); setEntries([]);
    } catch (e) { setError(String(e)); onChanged(); }
    finally { setRunProgressId(null); gate.current = false; setBusy(false); onBusy(false); }
  };
  const changedCount = entries.filter((entry) => entry.action !== "pick").length;
  const movedCount = entries.filter((entry) => entry.action !== "exec").filter((entry, index) => plan?.commits[index]?.id !== entry.id).length;
  const outputCount = entries.filter((entry) => !["drop", "fixup", "squash", "exec"].includes(entry.action)).length;
  const execSerial = useRef(0);
  /** Adds an exec line right after the selected commit (it runs once the commits before it are replayed) and focuses its command. */
  const addExec = () => {
    if (busy || !plan) return;
    const at = Math.min(selected + 1, entries.length);
    const id = `${EXEC_PREFIX}${++execSerial.current}`;
    setEntries((current) => { const next = [...current]; next.splice(at, 0, { id, subject: "", message: "", action: "exec" }); return next; });
    setSelected(at);
    window.setTimeout(() => root.current?.querySelector<HTMLInputElement>(".grw-entry.selected .grw-exec-input")?.focus(), 30);
  };
  const removeExec = (index: number) => { setEntries((current) => current.filter((_, i) => i !== index)); setSelected((value) => Math.max(0, Math.min(value, entries.length - 2))); root.current?.focus(); };
  // Offered, never applied on its own, so a freshly loaded plan is not unsaved work.
  const found = useMemo(() => plan ? autosquash(plan.commits) : null, [plan]);
  const suggestion = !unsaved && !busy ? found : null;
  const applyAutosquash = async () => {
    if (!plan || !suggestion) return;
    const byId = new Map(plan.commits.map((commit) => [commit.id, commit])), applied = plan;
    try {
      const amends = Object.values(suggestion.rewords);
      const messages = amends.length ? await commitMessages(path, amends) : new Map<string, string>();
      if (applied !== latestPlan.current) return;
      setEntries(suggestion.order.map((id) => {
        const message = suggestion.rewords[id] ? amendMessage(messages.get(suggestion.rewords[id]) ?? "") : "";
        return { ...byId.get(id)!, message, action: suggestion.actions[id] ?? (message ? "reword" : "pick") };
      }));
      setSelected(0);
    } catch (e) { setError(String(e)); }
  };
  const destination = (index: number) => {
    let previous = { id: "", subject: "" };
    let found = false;
    for (const entry of entries.slice(0, index)) {
      if (["pick", "reword", "edit"].includes(entry.action)) { previous = { id: entry.id, subject: entry.action === "reword" && entry.message.trim() ? entry.message.trim().split("\n")[0] : entry.subject }; found = true; }
      else if (entry.action === "squash" && found && entry.message.trim()) previous = { id: previous.id, subject: entry.message.trim().split("\n")[0] };
    }
    return found ? previous : null;
  };
  return <div className="git-rebase" ref={root} tabIndex={-1} aria-label={tt("Interactive rebase")} aria-busy={busy || loading} onKeyDown={(e) => {
    const { key, on } = gitKeys.press(["rebase"], e);
    // Starting works from a message too, with and without Vim keys.
    if (key === "Ctrl+Enter") { e.preventDefault(); e.stopPropagation(); if (!busy && !loading) void run(); return; }
    if ((e.target as HTMLElement).closest("input,textarea,select")) {
      // Out of the field first; the editor is left with the next Esc.
      if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); root.current?.focus(); }
      return;
    }
    e.stopPropagation();
    if (e.key === "Escape" && !busy) { e.preventDefault(); onClose(); return; }
    if (key === "?" && vim) { e.preventDefault(); setHelp((value) => !value); return; }
    const chord = e.ctrlKey || e.metaKey || e.altKey;
    // Moving with Ctrl or Alt needs no Vim keys.
    if (["J", "Ctrl+j", "Alt+ArrowDown"].includes(key) && (on || chord)) { e.preventDefault(); move(selected, -1); }
    else if (["K", "Ctrl+k", "Alt+ArrowUp"].includes(key) && (on || chord)) { e.preventDefault(); move(selected, 1); }
    else if (chord && key === gitKeyOf(e)) return;
    else if (e.key === "ArrowDown" || (vim && key === "j")) { e.preventDefault(); setSelected((index) => Math.max(0, index - 1)); }
    else if (e.key === "ArrowUp" || (vim && key === "k")) { e.preventDefault(); setSelected((index) => Math.min(index + 1, Math.max(0, entries.length - 1))); }
    else if (on && !busy) {
      const action = ({ p: "pick", r: "reword", s: "squash", f: "fixup", d: "drop", e: "edit" } as Record<string, Action>)[key];
      if (action) { e.preventDefault(); updateAction(selected, action); }
      else if (key === "x") { e.preventDefault(); addExec(); }
      else if (key === "a" && suggestion) { e.preventDefault(); void applyAutosquash(); }
      else if (key === "m") { const field = root.current?.querySelector<HTMLTextAreaElement>(".grw-entry.selected textarea"); if (field) { e.preventDefault(); field.focus(); } }
    }
  }}>
    <div className="grw-heading">
      <div className="grw-hero">
        <span className="grw-hero-icon" aria-hidden="true"><svg viewBox="0 0 32 32" fill="none"><path d="M9 7v18M23 7v4c0 7-14 3-14 10" /><circle cx="9" cy="7" r="3" /><circle cx="23" cy="7" r="3" /><circle cx="9" cy="25" r="3" /></svg></span>
        <div className="grw-hero-copy"><span className="grw-eyebrow">{tt("History editor")}</span><strong>{tt("Edit commits through HEAD")}</strong><small>{tt("Reorder the story. Keep the changes that matter.")}</small></div>
      </div>
      <div className="grw-heading-actions"><button disabled={busy || !plan} onClick={addExec} title={tt("Add a command that runs after the selected commit")}>{tt("+ exec")}</button><button disabled={busy} onClick={() => setHelp((value) => !value)} aria-expanded={help} title={tt("Rebase commands")}>{tt("? Commands")}</button><button className="grw-close" disabled={busy} onClick={onClose} aria-label={tt("Close interactive rebase")} title={tt("Close interactive rebase")}>✕</button></div>
    </div>
    <div className="grw-overview">
      <div className="grw-summary" aria-label={tt("Rebase plan summary")}><span className="grw-chip"><b>{entries.length}</b> {entries.length === 1 ? "commit" : "commits"}</span>{changedCount > 0 && <span className="grw-chip changed"><b>{changedCount}</b> {changedCount === 1 ? "action" : "actions"}</span>}{movedCount > 0 && <span className="grw-chip moved"><b>{movedCount}</b> {tt("moved")}</span>}{plan && entries.length > 0 && (conflicts ? (Object.keys(conflicts).length ? <span className="grw-chip conflict" role="status" title={tt("These commits would stop the rebase with a conflict in this order")}><b>{Object.keys(conflicts).length}</b> {Object.keys(conflicts).length === 1 ? "conflict" : "conflicts"}</span> : <span className="grw-chip clean" role="status">{tt("No conflicts")}</span>) : checking ? <span className="grw-chip" role="status">{tt("Checking conflicts…")}</span> : null)}<span className="grw-direction"><svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M8 2v12m-4-4 4 4 4-4" /></svg>{tt("Newest first")}</span></div>
      <details className="grw-range-options"><summary aria-disabled={busy} onClick={(event) => { if (busy) event.preventDefault(); }}>{tt("Change range")}<svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="m4 6 4 4 4-4" /></svg></summary><form className="grw-base" onSubmit={(e) => { e.preventDefault(); if (!busy) void load(base); }}><label>{tt("Unchanged base (exclusive)")}<input aria-label={tt("Rebase base commit")} value={base} disabled={busy} onChange={(e) => { setBase(e.target.value); setPlan(null); setEntries([]); }} placeholder={tt("Full commit ID")} /></label><button disabled={loading || busy || !base.trim()} type="submit">{tt("Load commits")}</button></form></details>
    </div>
    {plan && <div className="grw-range" aria-label={tt("Commit range")}><span className="grw-range-point"><i aria-hidden="true" /><span>{tt("Base")}</span><code title={plan.base}>{plan.base === ROOT_BASE ? tt("root") : plan.base.slice(0, 8)}</code><small>{plan.base === ROOT_BASE ? tt("the first commit is rewritten too") : tt("unchanged")}</small></span><span className="grw-range-line" aria-hidden="true" /><span className="grw-range-point head"><i aria-hidden="true" /><span>{tt("HEAD")}</span><code title={plan.head}>{plan.head.slice(0, 8)}</code></span></div>}
    {plan && plan.skippedMerges > 0 && <p className="grw-note" role="note">{tt(plan.skippedMerges === 1 ? "{count} merge commit is not listed; its commits are replayed linearly, like git rebase -i." : "{count} merge commits are not listed; their commits are replayed linearly, like git rebase -i.", { count: plan.skippedMerges })}</p>}
    {suggestion && <div className="grw-suggest" role="note"><span>{tt(suggestion.count === 1 ? "{count} commit is a fixup! or squash! of an earlier one." : "{count} commits are fixup! or squash! of earlier ones.", { count: suggestion.count })}</span><button disabled={busy} onClick={() => void applyAutosquash()}>{tt("Apply autosquash")}<GitKeyCap scope="rebase" k="a" /></button></div>}
    {help && <div className="grw-help"><span><kbd>j</kbd>/<kbd>k</kbd> {tt("select")}</span><span><GitKeyCap scope="rebase" k="J" /> /<GitKeyCap scope="rebase" k="K" /> {tt("or")} <kbd>{platformLabel("Ctrl+j")}</kbd>/<kbd>{platformLabel("Ctrl+k")}</kbd> {tt("reorder")}</span>{([["p", "pick"], ["r", "reword"], ["s", "squash"], ["f", "fixup"], ["d", "drop"], ["e", "edit stop"], ["m", "message"], ["x", "exec line"], ["a", "autosquash"], ["Ctrl+Enter", "start"]] as const).map(([key, label]) => <span key={key}><GitKeyCap scope="rebase" k={key} /> {tt(label)}</span>)}<span>{tt("Drag a card or use its arrows to change the order.")}</span></div>}
    {error && <p className="grw-error" role="alert">{error}</p>}
    {runProgress && <GitProgressBar progress={runProgress} />}
    {!runProgress && checking && checkProgress && <GitProgressBar progress={checkProgress} compact />}
    {result && <pre className="grw-result" role="status">{result}</pre>}
    <div className="grw-entries" ref={list} onScroll={entries.length > 200 ? (e) => { const el = e.currentTarget; setScroll((old) => Math.abs(old.top - el.scrollTop) > itemHeight * 8 || old.height !== el.clientHeight ? { top: el.scrollTop, height: el.clientHeight } : old); } : undefined}>{shown.above > 0 && <div style={{ height: shown.above }} aria-hidden="true" />}{loading ? (loadProgress ? <GitProgressBar progress={loadProgress} /> : <div className="grw-state" role="status"><span className="grw-loading-dot" /><strong>{tt("Loading your commits")}</strong><span>{tt("Preparing the history range…")}</span></div>) : plan && !entries.length ? <div className="grw-state"><strong>{tt("No commits in this range")}</strong><span>{tt("Choose an earlier base to include commits through HEAD.")}</span></div> : !plan && !result && !error ? <div className="grw-state"><strong>{tt("Choose a range to begin")}</strong><span>{tt("Open Change range and select a base commit.")}</span></div> : Array.from({ length: shown.last - shown.first }, (_, k) => entries.length - 1 - shown.first - k).map((i) => {
      const entry = entries[i];
      if (entry.action === "exec") {
        const problem = execProblem(entry.message);
        return <div key={entry.id} className={`grw-entry exec ${selected === i ? "selected" : ""}`} tabIndex={0} onFocus={() => setSelected(i)} onClick={() => setSelected(i)}>
          <span className="grw-step" aria-hidden="true">{i + 1}</span>
          <div className="grw-card"><div className="grw-entry-line">
            <span className="grw-exec-label" title={tt("Runs in a shell after the commits before it; the rebase stops here when it fails.")}>exec</span>
            <input className="grw-exec-input" aria-label={tt("Command to run after this step")} aria-invalid={!!problem && !!entry.message} value={entry.message} disabled={busy} placeholder={tt("Command, for example: cargo test")} onChange={(e) => setEntries((current) => current.map((value, index) => index === i ? { ...value, message: e.target.value, subject: e.target.value } : value))} />
            <div className="grw-move-controls"><button disabled={busy || i === entries.length - 1} onClick={(event) => { event.stopPropagation(); move(i, 1); }} aria-label={tt("Move the exec line up")} title={tt("Move up (newer)")}><svg viewBox="0 0 20 20" fill="none" aria-hidden="true"><path d="M10 15V5m-4 4 4-4 4 4" /></svg></button><button disabled={busy || i === 0} onClick={(event) => { event.stopPropagation(); move(i, -1); }} aria-label={tt("Move the exec line down")} title={tt("Move down (older)")}><svg viewBox="0 0 20 20" fill="none" aria-hidden="true"><path d="M10 5v10m-4-4 4 4 4-4" /></svg></button><button disabled={busy} onClick={(event) => { event.stopPropagation(); removeExec(i); }} aria-label={tt("Remove the exec line")} title={tt("Remove the exec line")}>✕</button></div>
          </div></div>
        </div>;
      }
      const into = entry.action === "fixup" || entry.action === "squash" ? destination(i) : null;
      const clash = conflicts?.[entry.id];
      // Exec lines are no commits: the position among the commits is what "from N" compares.
      const position = positions[i], originalPosition = original.get(entry.id) ?? position;
      return <div key={entry.id} className={`grw-entry ${selected === i ? "selected" : ""} ${entry.action} ${dragOver === i && dragging !== i ? "drag-over" : ""} ${dragging === i ? "dragging" : ""} ${clash ? "conflict" : ""}`} tabIndex={0} draggable={!busy} onDragStart={(event) => {
        if ((event.target as HTMLElement).closest("input,textarea,select,button")) { event.preventDefault(); return; }
        dragged.current = i; setDragging(i); event.dataTransfer.effectAllowed = "move"; event.dataTransfer.setData("text/plain", entry.id);
      }} onDragOver={(event) => { if (dragged.current !== null && !busy) { event.preventDefault(); event.dataTransfer.dropEffect = "move"; setDragOver(i); } }} onDrop={(event) => { event.preventDefault(); drop(i); setDragOver(null); setDragging(null); }} onDragEnd={() => { dragged.current = null; setDragOver(null); setDragging(null); }} onFocus={() => setSelected(i)} onClick={() => setSelected(i)}>
        <span className="grw-step" aria-hidden="true">{i + 1}</span>
        <div className="grw-card"><div className="grw-entry-line">
          <span className="grw-drag" aria-hidden="true"><svg viewBox="0 0 12 20" fill="currentColor"><circle cx="3" cy="5" r="1.3" /><circle cx="9" cy="5" r="1.3" /><circle cx="3" cy="10" r="1.3" /><circle cx="9" cy="10" r="1.3" /><circle cx="3" cy="15" r="1.3" /><circle cx="9" cy="15" r="1.3" /></svg></span>
          <div className="grw-commit-copy"><strong className="grw-subject" title={entry.subject}>{entry.subject}</strong><div className="grw-commit-meta"><code title={entry.id}>{entry.id.slice(0, 8)}</code>{entry.id === plan?.head && <span className="grw-head-badge">{tt("HEAD")}</span>}{originalPosition !== position && <span className="grw-moved-badge">{tt("from")} {originalPosition + 1}</span>}{clash && <span className="grw-conflict-badge" title={tt("This commit would conflict in the current order")}>{tt("Conflict")}</span>}</div></div>
          <span className="grw-action-picker"><ThemedSelect aria-label={tt("Action for {subject}", { subject: entry.subject })} title={tt("What to do with this commit")} value={entry.action} disabled={busy} onChange={(e) => updateAction(i, e.target.value as Action)}>{choices.map((action) => <option key={action} value={action}>{action[0].toUpperCase() + action.slice(1)}</option>)}</ThemedSelect><svg viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="m4 6 4 4 4-4" /></svg></span>
          <div className="grw-move-controls"><button disabled={busy || i === entries.length - 1} onClick={(event) => { event.stopPropagation(); move(i, 1); }} aria-label={tt("Move {subject} up", { subject: entry.subject })} title={tt("Move up (newer)")}><svg viewBox="0 0 20 20" fill="none" aria-hidden="true"><path d="M10 15V5m-4 4 4-4 4 4" /></svg></button><button disabled={busy || i === 0} onClick={(event) => { event.stopPropagation(); move(i, -1); }} aria-label={tt("Move {subject} down", { subject: entry.subject })} title={tt("Move down (older)")}><svg viewBox="0 0 20 20" fill="none" aria-hidden="true"><path d="M10 5v10m-4-4 4 4 4-4" /></svg></button></div>
        </div>
        {clash && <div className="grw-conflict-note" role="note"><span className="grw-note-dot" /><span>{tt("Would conflict")}{clash.length ? tt(" in {files}", { files: clash.slice(0, 3).join(", ") + (clash.length > 3 ? tt(" and {count} more", { count: clash.length - 3 }) : "") }) : ""}{tt(". The rebase would stop here.")}</span></div>}
        {(entry.action === "fixup" || entry.action === "squash") && <div className={`grw-combine-note ${!into ? "invalid" : ""}`}><svg viewBox="0 0 18 18" fill="none" aria-hidden="true"><path d="M13 14V9a4 4 0 0 0-4-4H4m3-3L4 5l3 3" /></svg><div>{into ? <><span>{entry.action === "fixup" ? tt("Fix up into") : tt("Squash into")} <strong title={into.subject}>{into.subject}</strong></span><small>{entry.action === "fixup" ? tt("Keep its message and combine the changes.") : tt("Combine changes and choose the final message below.")}</small></> : <span>{tt("Move a retained commit below this one to combine them.")}</span>}</div></div>}
        {(entry.action === "reword" || entry.action === "squash") && <label className="grw-message-editor"><span>{entry.action === "squash" ? tt("Combined commit message") : tt("New commit message")}{entry.action === "squash" && <small>{tt("Empty keeps both messages")}</small>}</span><textarea aria-label={tt("Message for {subject}", { subject: entry.subject })} value={entry.message} disabled={busy} onChange={(e) => setEntries((current) => current.map((value, index) => index === i ? { ...value, message: e.target.value } : value))} placeholder={entry.action === "squash" ? tt("Keep the existing messages, or write a new one…") : tt("Write the updated commit message…")} /></label>}
        {entry.action === "edit" && <div className="grw-action-note"><span className="grw-note-dot" />{tt("Pause here to amend this commit, then continue.")}</div>}
        {entry.action === "drop" && <div className="grw-action-note"><span className="grw-note-dot" />{tt("This commit will be removed from the rewritten history.")}</div>}
        </div>
      </div>;
    })}{shown.below > 0 && <div style={{ height: shown.below }} aria-hidden="true" />}</div>
    <div className="grw-footer"><div className="grw-outcome"><span>{entries.length ? tt(outputCount === 1 ? "{count} commit after rebase" : "{count} commits after rebase", { count: outputCount }) : tt("Review before applying")}</span><small>{tt("Original HEAD saved for recovery")}</small></div><button className="grw-start" disabled={busy || loading || !plan || !entries.length} onClick={() => void run()}><svg viewBox="0 0 18 18" fill="none" aria-hidden="true"><path d="M4 9h10m-4-4 4 4-4 4" /></svg><span>{tt("Start rebase…")}</span><GitKeyCap scope="rebase" k="Ctrl+Enter" /></button></div>
  </div>;
}
