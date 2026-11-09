import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { getState, useStore } from "../state/store";
import * as A from "../state/actions";
import "./GitAdvanced.css";

type Worktree = { path: string; head: string; branch: string | null; bare: boolean; locked: boolean; prunable: boolean };
type Submodule = { name: string; path: string; url: string; id: string; state: string };
type AdvancedState = { repository: string | null; token: string; worktrees: Worktree[]; submodules: Submodule[]; bisect: { current: string; candidate: string; checkoutPending: boolean; log: string; remaining: string } | null; notes: string };
type Tab = "worktrees" | "submodules" | "bisect" | "patches" | "notes" | "repository";
type Request = { action: string; [field: string]: string | null };
type Props = { path: string; commit?: string; initialTab?: string; onChanged: () => void; onBusy: (busy: boolean) => void; onBack?: () => void; onClose?: () => void };
const tabs: [Tab, string, string][] = [["worktrees", "Worktrees", "w"], ["submodules", "Submodules", "s"], ["bisect", "Bisect", "b"], ["patches", "Patches", "p"], ["notes", "Notes", "n"], ["repository", "Repository", "r"]];

export function GitAdvanced({ path, commit, initialTab, onChanged, onBusy, onBack, onClose }: Props) {
  const [state, setState] = useState<AdvancedState | null>(null);
  const [tab, setTab] = useState<Tab>(() => tabs.find(([value]) => value === initialTab)?.[0] ?? "worktrees");
  const [error, setError] = useState("");
  const [output, setOutput] = useState("");
  const [busy, setBusy] = useState(false);
  const [activeJob, setActiveJob] = useState("");
  const [loading, setLoading] = useState(false);
  const [keys, setKeys] = useState(false);
  const [destination, setDestination] = useState("");
  const [branch, setBranch] = useState("");
  const [target, setTarget] = useState(commit ?? "");
  const [good, setGood] = useState("");
  const [bad, setBad] = useState(commit ?? "");
  const [patchFile, setPatchFile] = useState("");
  const [exportDestination, setExportDestination] = useState("");
  const [note, setNote] = useState("");
  const [url, setUrl] = useState("");
  const [clonePath, setClonePath] = useState("");
  const [initialBranch, setInitialBranch] = useState("main");
  const root = useRef<HTMLElement>(null);
  const request = useRef(0);
  const job = useRef("");
  const vim = useStore((s) => s.settings?.vimKeys ?? false);
  const load = useCallback(async () => {
    const ticket = ++request.current;
    setLoading(true);
    try {
      const value = await invoke<AdvancedState>("git_advanced", { path, commit: commit ?? null });
      if (ticket === request.current) { setState(value); setNote(value.notes); }
    } catch (e) { if (ticket === request.current) setError(String(e)); }
    finally { if (ticket === request.current) setLoading(false); }
  }, [path, commit]);
  useEffect(() => { setState(null); setError(""); setTarget(commit ?? ""); setBad(commit ?? ""); void load(); return () => { request.current++; }; }, [load, commit]);
  useEffect(() => {
    let active = true;
    const listener = listen<{ id: string; text: string }>("git-advanced-progress", (e) => {
      if (active && e.payload.id === job.current) setOutput((value) => (value + e.payload.text).slice(-16000));
    });
    return () => { active = false; void listener.then((unlisten) => unlisten()); };
  }, []);
  useEffect(() => {
    const element = root.current;
    const command = (event: Event) => {
      const key = (event as CustomEvent<string>).detail;
      if (key === "?") setKeys((value) => !value);
      else if (!busy) { const selected = tabs.find(([, , value]) => value === key); if (selected) setTab(selected[0]); }
    };
    element?.addEventListener("gall-git-command", command);
    return () => element?.removeEventListener("gall-git-command", command);
  }, [busy]);
  const act = async (action: Request, title: string, description: string, danger = false) => {
    if (!state || busy || loading) return;
    setBusy(true); onBusy(true); setError(""); setOutput("");
    try {
      if (!await A.confirm(title, description, title.replace(/…$/, ""), danger)) return;
      job.current = `advanced-${Date.now()}-${Math.random().toString(36).slice(2)}`;
      setActiveJob(job.current);
      const result = await invoke<string>("git_advanced_action", { path, request: action, expected: state.token, id: job.current });
      setOutput(result || "Git operation completed.");
      await load(); onChanged();
    } catch (e) { setError(String(e)); await load(); onChanged(); }
    finally { job.current = ""; setActiveJob(""); setBusy(false); onBusy(false); }
  };
  const openPath = (folder: string) => { A.openOverlay(null); A.navigate(getState().session?.activeBrowser ?? 0, folder); };
  const browsePatch = async () => { try { const value = await open({ multiple: false, filters: [{ name: "Git patches", extensions: ["patch", "diff"] }] }); if (typeof value === "string") setPatchFile(value); } catch (e) { setError(String(e)); } };
  const exportPatch = async () => {
    if (!commit) return;
    try {
      const destination = exportDestination.trim() || await save({ defaultPath: `${commit.slice(0, 8)}.patch`, filters: [{ name: "Git patch", extensions: ["patch"] }] });
      if (destination) await act({ action: "exportPatch", commit, destination }, "Export patch", `Export commit ${commit} to a new file:\n${destination}\n\nExisting files are never overwritten.`);
    } catch (e) { setError(String(e)); }
  };
  const disabled = busy || loading;
  return <section className="git-advanced" ref={root} tabIndex={-1} aria-label="Advanced Git" onKeyDown={(e) => {
    if ((e.target as HTMLElement).closest("input,textarea,select")) return;
    if (e.key === "Escape") {
      // Close the tools and return to the commit list; never the whole workspace.
      if (onClose) { e.preventDefault(); e.stopPropagation(); onClose(); }
      return;
    }
    e.stopPropagation();
    if (!vim || e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === "h" && onBack) { e.preventDefault(); onBack(); }
    else if (e.key === "?") { e.preventDefault(); setKeys((value) => !value); }
    else if (!busy) { const selected = tabs.find(([, , key]) => key === e.key); if (selected) { e.preventDefault(); setTab(selected[0]); } }
  }}>
    <div className="ga-header"><strong>Repository tools</strong><button disabled={disabled} onClick={() => { setError(""); void load(); }}>Refresh</button><button aria-expanded={keys} onClick={() => setKeys((value) => !value)}>Keys ?</button></div>
    <div className="ga-tabs" role="tablist" aria-label="Repository tools">{tabs.map(([value, name, key]) => <button role="tab" key={value} aria-selected={value === tab} disabled={busy} className={value === tab ? "on" : ""} onClick={() => setTab(value)}>{name}{vim && <kbd>{key}</kbd>}</button>)}</div>
    {keys && <p className="ga-help">{vim ? "w worktrees · s submodules · b bisect · p patches · n notes · r repository · ? keys" : "Use Tab to select controls, arrow keys inside fields, Enter or Space to activate buttons."}</p>}
    {error && <p className="ga-error" role="alert">{error}</p>}
    {!state ? <p className="ga-notice">{loading ? "Loading repository tools…" : "Repository tools could not be loaded. Refresh to retry."}</p> : <div className="ga-content" role="tabpanel" aria-label={tabs.find(([value]) => value === tab)?.[1]}>
      {!state.repository && tab !== "repository" ? <p className="ga-notice">This folder is outside a Git repository. Choose Repository to initialize this folder or clone into a new destination.</p> : tab === "worktrees" ? <>
        <p className="ga-description">A worktree gives a branch its own working folder. Removing a worktree requires it to be clean; no force removal is used.</p>
        <div className="ga-list">{state.worktrees.map((tree) => <div className="ga-item" key={tree.path}><div><b>{tree.branch ?? (tree.bare ? "Bare repository" : "Detached HEAD")}</b><code title={tree.head}>{tree.head.slice(0, 8)}</code>{tree.locked && <span className="ga-badge">Locked</span>}{tree.prunable && <span className="ga-badge">Prunable</span>}<span className="ga-path">{tree.path}</span></div><div className="ga-item-actions"><button disabled={disabled || tree.bare} onClick={() => openPath(tree.path)}>Open</button><button disabled={disabled || tree.bare || tree.path === state.repository} onClick={() => void act({ action: tree.locked ? "unlockWorktree" : "lockWorktree", destination: tree.path }, tree.locked ? "Unlock worktree" : "Lock worktree", `${tree.locked ? "Unlock" : "Lock"} this worktree:\n${tree.path}`)}>{tree.locked ? "Unlock" : "Lock"}</button><button disabled={disabled || tree.bare || tree.locked || tree.path === state.repository} onClick={() => void act({ action: "removeWorktree", destination: tree.path }, "Remove worktree", `Remove this clean working folder and its worktree registration:\n${tree.path}\n\nThe branch and commits are retained. Local changes prevent removal.`, true)}>Remove…</button></div></div>)}</div>
        <div className="ga-form"><h3>Create worktree</h3><label>New absolute folder<input aria-label="Worktree destination" value={destination} onChange={(e) => setDestination(e.target.value)} placeholder={`${state.repository}-worktree`} disabled={disabled} /></label><label>Commit ID<input aria-label="Worktree commit" value={target} onChange={(e) => setTarget(e.target.value)} placeholder="Full commit ID" disabled={disabled} /></label><label>New branch (optional)<input aria-label="Worktree branch" value={branch} onChange={(e) => setBranch(e.target.value)} placeholder="Leave blank for detached HEAD" disabled={disabled} /></label><button disabled={disabled || !destination.trim() || !target.trim()} onClick={() => void act({ action: "createWorktree", destination, target, branch: branch.trim() || null }, "Create worktree", `Create a new working folder:\n${destination}\n\nAt commit ${target}${branch.trim() ? ` on new branch ${branch.trim()}` : " with detached HEAD"}.`)}>Create worktree…</button></div>
      </> : tab === "submodules" ? <><p className="ga-description">Initialize or update a submodule to the commit recorded in this repository. Local changes prevent updates. Nested modules are not updated automatically.</p><div className="ga-list">{state.submodules.length === 0 ? <p className="ga-notice">No submodules configured.</p> : state.submodules.map((module) => <div className="ga-item" key={module.path}><div><b>{module.name}</b><span className="ga-badge">{module.state}</span><span className="ga-path">{module.path}</span><small>{module.url}</small><code>{module.id.slice(0, 12)}</code></div><button disabled={disabled || module.state === "unsupportedPath"} onClick={() => void act({ action: module.state === "notInitialized" ? "initSubmodule" : "updateSubmodule", file: module.path }, module.state === "notInitialized" ? "Initialize submodule" : "Update submodule", `Update ${module.path} to ${module.id || "its recorded commit"}.\n\nSource: ${module.url}\n\nThis may access the network. Local edits are preserved by refusing a dirty submodule.`)}>{module.state === "notInitialized" ? "Initialize…" : "Update…"}</button></div>)}</div></>
      : tab === "bisect" ? <><p className="ga-description">Find the first bad commit by testing successive revisions. Starting a bisect preserves the current commit and requires a clean working folder.</p>{state.bisect ? <><div className="ga-bisect-state"><b>Test revision {state.bisect.current.slice(0, 12)}</b>{state.bisect.checkoutPending && <p className="ga-notice">Checkout to {state.bisect.candidate.slice(0, 12)} is pending. Preserve any colliding ignored files, then <button disabled={disabled} onClick={() => void act({ action: "bisectCheckout" }, "Checkout test revision", `Move the working folder to ${state.bisect!.candidate}. Existing colliding files prevent checkout.`)}>Retry checkout…</button></p>}<div className="ga-buttons"><button disabled={disabled || state.bisect.checkoutPending} onClick={() => void act({ action: "bisectGood" }, "Mark good", `Mark ${state.bisect!.current} good and move to the next test revision.`)}>Good</button><button disabled={disabled || state.bisect.checkoutPending} onClick={() => void act({ action: "bisectBad" }, "Mark bad", `Mark ${state.bisect!.current} bad and move to the next test revision.`)}>Bad</button><button disabled={disabled || state.bisect.checkoutPending} onClick={() => void act({ action: "bisectSkip" }, "Skip revision", `Skip testing ${state.bisect!.current} and try another revision.`)}>Skip</button><button disabled={disabled} onClick={() => void act({ action: "bisectReset" }, "Finish bisect", "End this bisect and restore the branch or commit from which it started.")}>Finish / reset…</button></div><pre>{state.bisect.log}</pre></div></> : <div className="ga-form"><label>Known good commit<input aria-label="Bisect good commit" value={good} onChange={(e) => setGood(e.target.value)} disabled={disabled} placeholder="Full commit ID" /></label><label>Known bad commit<input aria-label="Bisect bad commit" value={bad} onChange={(e) => setBad(e.target.value)} disabled={disabled} placeholder="Full commit ID" /></label><button disabled={disabled || !good.trim() || !bad.trim()} onClick={() => void act({ action: "bisectStart", good, bad }, "Start bisect", `Test the history between good ${good} and bad ${bad}.\n\nThe working folder will move to a test revision. Finish / reset returns to the starting revision.`)}>Start bisect…</button></div>}</>
      : tab === "patches" ? <><p className="ga-description">Export a commit as an email patch, or apply a patch to both the index and working folder. Applying first checks the exact patch against a clean repository.</p><div className="ga-form"><h3>Export selected commit</h3><code className="ga-path">{commit ?? "Select a commit in the graph first."}</code><label>New patch file<input aria-label="Patch export destination" value={exportDestination} onChange={(e) => setExportDestination(e.target.value)} disabled={disabled} placeholder="Absolute path, or leave blank to choose a file" /></label><button disabled={disabled || !commit} onClick={() => void exportPatch()}>Export commit patch…</button></div><div className="ga-form"><h3>Apply patch</h3><label>Patch file<div className="ga-input-row"><input aria-label="Patch file" value={patchFile} onChange={(e) => setPatchFile(e.target.value)} placeholder="Absolute .patch or .diff path" disabled={disabled} /><button disabled={disabled} onClick={() => void browsePatch()}>Browse…</button></div></label><button disabled={disabled || !patchFile.trim()} onClick={() => void act({ action: "applyPatch", file: patchFile }, "Apply patch", `Check and apply this patch to the index and working folder:\n${patchFile}\n\nNo commit is created. Existing work must be committed or stashed first.`)}>Check & apply…</button></div></>
      : tab === "notes" ? <><p className="ga-description">A Git note annotates a commit without changing its ID. Replaced notes remain recoverable through saved references.</p><div className="ga-form"><label>Selected commit<code className="ga-path">{commit ?? "Select a commit in the graph first."}</code></label><label>Commit note<textarea aria-label="Git note" value={note} onChange={(e) => setNote(e.target.value)} disabled={disabled || !commit} placeholder="Add a note to this commit" /></label><button disabled={disabled || !commit || note === state.notes} onClick={() => commit && void act({ action: "writeNote", commit, message: note }, note.trim() ? "Save note" : "Remove note", `${note.trim() ? "Save this note on" : "Remove the note from"} commit ${commit}.\n\nThe commit ID and working files stay unchanged.`)}>{note.trim() ? "Save note…" : "Remove note…"}</button></div></>
      : <><p className="ga-description">Initialize the current folder, or clone into a new folder. Clone failures retain their partial destination for inspection.</p>{!state.repository && <div className="ga-form"><h3>Initialize this folder</h3><code className="ga-path">{path}</code><label>Initial branch<input aria-label="Initial branch" value={initialBranch} onChange={(e) => setInitialBranch(e.target.value)} disabled={disabled} /></label><button disabled={disabled || !initialBranch.trim()} onClick={() => void act({ action: "init", branch: initialBranch }, "Initialize repository", `Create a Git repository in this existing folder:\n${path}\n\nInitial branch: ${initialBranch}`)}>Initialize repository…</button></div>}<div className="ga-form"><h3>Clone repository</h3><label>Repository URL<input aria-label="Clone URL" value={url} onChange={(e) => setUrl(e.target.value)} disabled={disabled} placeholder="https://host/owner/repository.git" /></label><label>New absolute destination<input aria-label="Clone destination" value={clonePath} onChange={(e) => setClonePath(e.target.value)} disabled={disabled} placeholder="/path/to/new-folder" /></label><button disabled={disabled || !url.trim() || !clonePath.trim()} onClick={() => void act({ action: "clone", url, destination: clonePath }, "Clone repository", `Clone ${url}\n\nInto a new folder:\n${clonePath}\n\nExisting destinations are rejected. Configured credentials are used; interactive login requires your terminal.`)}>Clone…</button></div></>}
    </div>}
    {(busy || output) && <div className="ga-output"><div><b role="status">{busy ? "Running Git operation…" : "Operation result"}</b>{busy && activeJob && <button onClick={() => void invoke("git_advanced_cancel", { id: activeJob }).catch((e) => setError(String(e)))}>Cancel operation</button>}</div><pre>{output || "Waiting for Git…"}</pre></div>}
  </section>;
}
