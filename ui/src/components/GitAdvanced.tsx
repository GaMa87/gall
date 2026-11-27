import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { getState, useStore } from "../state/store";
import * as A from "../state/actions";
import "./GitAdvanced.css";
import { tt } from "../i18n";
import { gitKeyOf } from "../core/gitCommands";
import { useGitKeys } from "../state/gitKeys";
import { GitKeyCap } from "./GitKeyCap";
import type { GitReference, Repository } from "../core/gitRepository";
import { worktreeLabel, worktreeSource } from "../core/gitWorktree";
import { createWorktree, worktreeLocation, type WorktreeLocation } from "../state/gitWorktree";
import { joinCommandLine, splitCommandLine } from "../core/gitCustom";
import { GitProgressBar, useGitProgress, useLocalProgress, type GitProgressSeed } from "./GitProgressBar";

type Worktree = { path: string; head: string; branch: string | null; bare: boolean; locked: boolean; prunable: boolean };
type Submodule = { name: string; path: string; url: string; id: string; state: string };
type AdvancedState = { repository: string | null; token: string; worktrees: Worktree[]; submodules: Submodule[]; bisect: { current: string; candidate: string; checkoutPending: boolean; log: string; remaining: string } | null; notes: string };
type Tab = "worktrees" | "submodules" | "bisect" | "patches" | "notes" | "repository";
type Request = { action: string; [field: string]: string | null };
type Props = { path: string; commit?: string; reference?: GitReference; repository?: Repository; initialTab?: string; onChanged: () => void; onBusy: (busy: boolean) => void; onBack?: () => void; onClose?: () => void };
const tabs: [Tab, string, string][] = [["worktrees", "Worktrees", "w"], ["submodules", "Submodules", "s"], ["bisect", "Bisect", "b"], ["patches", "Patches", "p"], ["notes", "Notes", "n"], ["repository", "Repository", "r"]];

export function GitAdvanced({ path, commit, reference, repository, initialTab, onChanged, onBusy, onBack, onClose }: Props) {
  const [state, setState] = useState<AdvancedState | null>(null);
  const [tab, setTab] = useState<Tab>(() => tabs.find(([value]) => value === initialTab)?.[0] ?? "worktrees");
  const [error, setError] = useState("");
  const [output, setOutput] = useState("");
  const [busy, setBusy] = useState(false);
  const [activeJob, setActiveJob] = useState("");
  const [activeSeed, setActiveSeed] = useState<GitProgressSeed | undefined>(undefined);
  const [loading, setLoading] = useState(false);
  const jobProgress = useGitProgress(activeJob || null, activeSeed);
  const loadProgress = useLocalProgress(loading && !state, "Loading repository tools");
  const [removing, setRemoving] = useState("");
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
  const gitKeys = useGitKeys(), { vim } = gitKeys;
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
  const settings = useStore((s) => s.settings);
  // The form proposes the preset folder until another one is typed.
  const [typed, setTyped] = useState(false), [place, setPlace] = useState<WorktreeLocation | null>(null), [template, setTemplate] = useState<string | null>(null);
  const chosen = reference && repository && target === reference.id && !branch.trim() ? worktreeSource({ kind: "reference", ref: reference }, repository) : null;
  const source = chosen && !("error" in chosen) && chosen.kind !== "commit" ? chosen : null;
  const folder = branch.trim() || (source ? worktreeLabel(source) : reference?.kind === "tag" && target === reference.id ? reference.name : target.slice(0, 8)) || "worktree";
  const own = settings?.worktreeLocations ?? {};
  // Another repository: what was typed for the previous one must not be used or saved here.
  useEffect(() => { setTyped(false); setTemplate(null); setPlace(null); setDestination(""); }, [path]);
  useEffect(() => {
    if (tab !== "worktrees") return;
    let active = true;
    const timer = window.setTimeout(() => {
      const general = settings?.worktreeLocation ?? "";
      // A typed template is previewed before it is saved; an empty one means the general template.
      void worktreeLocation(path, folder, template?.trim() || general, template === null ? undefined : {}).then((value) => {
        if (!active) return;
        setPlace(value);
        if (!typed) setDestination(value.path ?? "");
      }).catch((e) => { if (active) setPlace({ main: "", path: null, error: String(e), inside: false }); });
    }, 120);
    return () => { active = false; window.clearTimeout(timer); };
  }, [tab, path, folder, template, typed, settings?.worktreeLocation, settings?.worktreeLocations, state]);
  const saveTemplate = () => {
    if (template === null || !place?.main) return;
    const value = template.trim();
    if (value && place.error) return;
    const next = { ...own };
    if (value) next[place.main] = value; else delete next[place.main];
    setTemplate(null);
    void A.updateSettings({ worktreeLocations: next });
  };
  // The command a new worktree of this repository runs after it was created: typed as a line, kept as arguments.
  const [afterText, setAfterText] = useState<string | null>(null);
  const afters = settings?.worktreeCommands ?? {};
  const afterProblem = afterText === null ? "" : afterText.trim() && splitCommandLine(afterText) === null ? tt("A quote is not closed.") : "";
  useEffect(() => { setAfterText(null); }, [path]);
  const saveAfter = () => {
    if (afterText === null || !place?.main) return;
    const argv = splitCommandLine(afterText);
    if (argv === null) return;
    const next = { ...afters };
    if (argv.length) next[place.main] = argv; else delete next[place.main];
    setAfterText(null);
    void A.updateSettings({ worktreeCommands: next });
  };
  const links = settings?.worktreeLinks ?? {};
  const setLinks = (on: boolean) => { if (!place?.main) return; const next = { ...links }; if (on) next[place.main] = true; else delete next[place.main]; void A.updateSettings({ worktreeLinks: next }); };
  /** A branch at the selected commit is checked out itself instead of a detached HEAD. */
  const createFromReference = async () => {
    if (!source || busy || loading) return;
    setBusy(true); onBusy(true); setError(""); setOutput("");
    setActiveSeed({ operation: "worktree", phase: "Creating worktree", cancellable: true });
    try { setOutput(await createWorktree(path, source, onChanged, destination.trim(), (id) => { job.current = id; setActiveJob(id); })); setTyped(false); await load(); onChanged(); }
    catch (e) { setError(String(e)); await load(); onChanged(); }
    finally { job.current = ""; setActiveJob(""); setActiveSeed(undefined); setBusy(false); onBusy(false); }
  };
  const act = async (action: Request, title: string, description: string, danger = false) => {
    if (!state || busy || loading) return;
    setBusy(true); onBusy(true); setError(""); setOutput("");
    if (action.action !== "removeWorktree") setRemoving("");
    try {
      if (!await A.confirm(title, description, title.replace(/…$/, ""), danger)) return;
      job.current = `advanced-${Date.now()}-${Math.random().toString(36).slice(2)}`;
      setActiveSeed({ operation: action.action, phase: title.replace(/…$/, ""), cancellable: true });
      setActiveJob(job.current);
      const result = await invoke<string>("git_advanced_action", { path, request: action, expected: state.token, id: job.current });
      setOutput(result || tt("Git operation completed."));
      await load(); onChanged();
    } catch (e) { setError(String(e)); await load(); onChanged(); }
    finally { job.current = ""; setActiveJob(""); setActiveSeed(undefined); setBusy(false); onBusy(false); }
  };
  const openPath = (folder: string) => { A.openOverlay(null); A.navigate(getState().session?.activeBrowser ?? 0, folder); };
  const browsePatch = async () => { try { const value = await open({ multiple: false, filters: [{ name: "Git patches", extensions: ["patch", "diff"] }] }); if (typeof value === "string") setPatchFile(value); } catch (e) { setError(String(e)); } };
  const exportPatch = async () => {
    if (!commit) return;
    try {
      const destination = exportDestination.trim() || await save({ defaultPath: `${commit.slice(0, 8)}.patch`, filters: [{ name: "Git patch", extensions: ["patch"] }] });
      if (destination) await act({ action: "exportPatch", commit, destination }, tt("Export patch"), tt("Export commit {commit} to a new file:\n{destination}\n\nExisting files are never overwritten.", { commit, destination }));
    } catch (e) { setError(String(e)); }
  };
  const disabled = busy || loading;
  return <section className="git-advanced" ref={root} tabIndex={-1} aria-label={tt("Advanced Git")} onKeyDown={(e) => {
    if ((e.target as HTMLElement).closest("input,textarea,select")) return;
    if (e.key === "Escape") {
      // Close the tools and return to the commit list; never the whole workspace.
      if (onClose) { e.preventDefault(); e.stopPropagation(); onClose(); }
      return;
    }
    e.stopPropagation();
    const { key, on } = gitKeys.press(["tools"], e);
    if (!on || ((e.ctrlKey || e.metaKey || e.altKey) && key === gitKeyOf(e))) return;
    if (key === "h" && onBack) { e.preventDefault(); onBack(); }
    else if (key === "?") { e.preventDefault(); setKeys((value) => !value); }
    else if (!busy) { const selected = tabs.find(([, , tabKey]) => tabKey === key); if (selected) { e.preventDefault(); setTab(selected[0]); } }
  }}>
    <div className="ga-header"><strong>{tt("Repository tools")}</strong><button disabled={disabled} onClick={() => { setError(""); void load(); }}>{tt("Refresh")}</button><button aria-expanded={keys} onClick={() => setKeys((value) => !value)}>{tt("Keys ?")}</button></div>
    <div className="ga-tabs" role="tablist" aria-label={tt("Repository tools")}>{tabs.map(([value, name, key]) => <button role="tab" key={value} aria-selected={value === tab} disabled={busy} className={value === tab ? "on" : ""} onClick={() => setTab(value)}>{tt(name)}<GitKeyCap scope="tools" k={key} /></button>)}</div>
    {keys && <p className="ga-help">{vim ? `${gitKeys.hints("tools").map((hint) => `${hint.name} ${tt(hint.label)}`).join(" · ")} · ${tt("? keys")}` : tt("Use Tab to select controls, arrow keys inside fields, Enter or Space to activate buttons.")}</p>}
    {error && <p className="ga-error" role="alert">{error}</p>}
    {error.includes("Untracked or ignored files are present") && removing && <p className="ga-description"><button disabled={disabled} onClick={() => void act({ action: "removeWorktreeWithIgnored", destination: removing }, tt("Delete and remove…"), tt("{path}\n\nThis folder contains untracked or ignored files, for example node_modules, build output or caches. Removing the worktree deletes them permanently.\n\nChanges to tracked files and new files that are not ignored still block the removal.", { path: removing }), true)}>{tt("Delete ignored files and remove…")}</button></p>}
    {!state ? (loadProgress ? <GitProgressBar progress={loadProgress} /> : <p className="ga-notice">{tt("Repository tools could not be loaded. Refresh to retry.")}</p>) : <div className="ga-content" role="tabpanel" aria-label={tt(tabs.find(([value]) => value === tab)?.[1] ?? "")}>
      {!state.repository && tab !== "repository" ? <p className="ga-notice">{tt("This folder is outside a Git repository. Choose Repository to initialize this folder or clone into a new destination.")}</p> : tab === "worktrees" ? <>
        <p className="ga-description">{tt("A worktree gives a branch its own working folder. Removing a worktree requires it to be clean; no force removal is used.")}</p>
        <div className="ga-list">{state.worktrees.map((tree) => <div className="ga-item" key={tree.path}><div><b>{tree.branch ?? (tree.bare ? tt("Bare repository") : tt("Detached HEAD"))}</b><code title={tree.head}>{tree.head.slice(0, 8)}</code>{tree.locked && <span className="ga-badge">{tt("Locked")}</span>}{tree.prunable && <span className="ga-badge">{tt("Prunable")}</span>}<span className="ga-path">{tree.path}</span></div><div className="ga-item-actions"><button disabled={disabled || tree.bare} onClick={() => openPath(tree.path)}>{tt("Open")}</button><button disabled={disabled || tree.bare || tree.path === state.repository} onClick={() => void act({ action: tree.locked ? "unlockWorktree" : "lockWorktree", destination: tree.path }, tree.locked ? tt("Unlock worktree") : tt("Lock worktree"), tt(tree.locked ? "Unlock this worktree:\n{path}" : "Lock this worktree:\n{path}", { path: tree.path }))}>{tree.locked ? tt("Unlock") : tt("Lock")}</button><button disabled={disabled || tree.bare || tree.locked || tree.path === state.repository} onClick={() => { setRemoving(tree.path); void act({ action: "removeWorktree", destination: tree.path }, tt("Remove worktree"), tt("Remove this clean working folder and its worktree registration:\n{path}\n\nThe branch and commits are retained. Local changes prevent removal.", { path: tree.path }), true); }}>{tt("Remove…")}</button></div></div>)}</div>
        <div className="ga-form"><h3>{tt("Create worktree")}</h3><label>{tt("New absolute folder")}<input aria-label={tt("Worktree destination")} value={destination} onChange={(e) => { setDestination(e.target.value); setTyped(e.target.value !== ""); }} placeholder={place?.path ?? `${state.repository}-worktree`} spellCheck={false} disabled={disabled} /></label><label>{tt("Commit ID")}<input aria-label={tt("Worktree commit")} value={target} onChange={(e) => setTarget(e.target.value)} placeholder={tt("Full commit ID")} disabled={disabled} /></label><label>{tt("New branch (optional)")}<input aria-label={tt("Worktree branch")} value={branch} onChange={(e) => setBranch(e.target.value)} placeholder={source ? tt("Leave blank to check out {name}", { name: source.name }) : tt("Leave blank for detached HEAD")} disabled={disabled} /></label><button disabled={disabled || !destination.trim() || !target.trim()} onClick={() => void (source ? createFromReference() : act({ action: "createWorktree", destination, target, branch: branch.trim() || null }, tt("Create worktree"), tt("Create a new working folder:\n{destination}\n\nAt commit {target}{value}.", { destination, target, value: branch.trim() ? tt(" on new branch {branch}", { branch: branch.trim() }) : tt(" with detached HEAD") })))}>{tt("Create worktree…")}</button></div>
        <div className="ga-form"><h3>{tt("Location of new worktrees")}</h3><label>{tt("For this repository only")}<input aria-label={tt("Worktree location of this repository")} value={template ?? own[place?.main ?? ""] ?? ""} onChange={(e) => setTemplate(e.target.value)} onBlur={saveTemplate} onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); saveTemplate(); } if (e.key === "Escape" && template !== null) { e.preventDefault(); e.stopPropagation(); setTemplate(null); } }} placeholder={settings?.worktreeLocation || "{repo}/.worktrees/{branch}"} spellCheck={false} disabled={disabled} /></label>
          {place?.error ? <p className="ga-preview ga-preview-error" role="alert">{place.error}</p> : place?.path && <p className="ga-preview">{tt("Next: {path}", { path: place.path })}{place.inside && ` · ${tt("kept out of the status by a local exclude rule")}`}</p>}
          <label>{tt("After creating a worktree, run (optional)")}<input aria-label={tt("Command after creating a worktree")} value={afterText ?? joinCommandLine(afters[place?.main ?? ""] ?? [])} onChange={(e) => setAfterText(e.target.value)} onBlur={saveAfter} onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); saveAfter(); } if (e.key === "Escape" && afterText !== null) { e.preventDefault(); e.stopPropagation(); setAfterText(null); } }} placeholder="npm install" spellCheck={false} disabled={disabled} aria-invalid={!!afterProblem} /></label>
          <label className="ga-check"><input type="checkbox" aria-label={tt("Link the folders of .worktreeinclude instead of copying them")} checked={!!links[place?.main ?? ""]} onChange={(e) => setLinks(e.target.checked)} disabled={disabled || !place?.main} />{tt("Link the folders of .worktreeinclude instead of copying them")}</label>
          <small className="ga-preview">{tt("A folder such as node_modules that .worktreeinclude names is then not copied: the new worktree gets a link (a junction on Windows) to the folder of the main worktree, so both share its content. Removing the worktree removes only the link.")}</small>
          {afterProblem ? <p className="ga-preview ga-preview-error" role="alert">{afterProblem}</p> : <small className="ga-preview">{tt("Runs in the new worktree without a shell: the first word is the program. {path}, {branch} and {repo} are filled in. Its output is shown in a message.")}</small>}
          <small className="ga-preview">{tt("{repo} is the repository, {branch} the branch. Settings › Behaviour sets the location for all repositories. Ignored files and folders named in .worktreeinclude are copied into worktrees created with W by default. Repository tools › Worktrees can link ignored folders instead.")}</small></div>
      </> : tab === "submodules" ? <><p className="ga-description">{tt("Initialize or update a submodule to the commit recorded in this repository. Local changes prevent updates. Nested modules are not updated automatically.")}</p><div className="ga-list">{state.submodules.length === 0 ? <p className="ga-notice">{tt("No submodules configured.")}</p> : state.submodules.map((module) => <div className="ga-item" key={module.path}><div><b>{module.name}</b><span className="ga-badge">{module.state}</span><span className="ga-path">{module.path}</span><small>{module.url}</small><code>{module.id.slice(0, 12)}</code></div><button disabled={disabled || module.state === "unsupportedPath"} onClick={() => void act({ action: module.state === "notInitialized" ? "initSubmodule" : "updateSubmodule", file: module.path }, module.state === "notInitialized" ? tt("Initialize submodule") : tt("Update submodule"), tt("Update {path} to {value}.\n\nSource: {url}\n\nThis may access the network. Local edits are preserved by refusing a dirty submodule.", { path: module.path, value: module.id || "its recorded commit", url: module.url }))}>{module.state === "notInitialized" ? tt("Initialize…") : tt("Update…")}</button>{module.state !== "notInitialized" && module.state !== "unsupportedPath" && <button disabled={disabled} onClick={() => void act({ action: "deinitSubmodule", file: module.path }, tt("Deinitialize submodule…"), tt("Remove the local checkout at {path} and keep its Git data so it can be initialized again.", { path: module.path }), true)}>{tt("Deinitialize submodule…")}</button>}</div>)}</div></>
      : tab === "bisect" ? <><p className="ga-description">{tt("Find the first bad commit by testing successive revisions. Starting a bisect preserves the current commit and requires a clean working folder.")}</p>{state.bisect ? <><div className="ga-bisect-state"><b>{tt("Test revision")} {state.bisect.current.slice(0, 12)}</b>{state.bisect.checkoutPending && <p className="ga-notice">{tt("Checkout to")} {state.bisect.candidate.slice(0, 12)} {tt("is pending. Preserve any colliding ignored files, then")} <button disabled={disabled} onClick={() => void act({ action: "bisectCheckout" }, tt("Checkout test revision"), tt("Move the working folder to {candidate}. Existing colliding files prevent checkout.", { candidate: state.bisect!.candidate }))}>{tt("Retry checkout…")}</button></p>}<div className="ga-buttons"><button disabled={disabled || state.bisect.checkoutPending} onClick={() => void act({ action: "bisectGood" }, tt("Mark good"), tt("Mark {current} good and move to the next test revision.", { current: state.bisect!.current }))}>{tt("Good")}</button><button disabled={disabled || state.bisect.checkoutPending} onClick={() => void act({ action: "bisectBad" }, tt("Mark bad"), tt("Mark {current} bad and move to the next test revision.", { current: state.bisect!.current }))}>{tt("Bad")}</button><button disabled={disabled || state.bisect.checkoutPending} onClick={() => void act({ action: "bisectSkip" }, tt("Skip revision"), tt("Skip testing {current} and try another revision.", { current: state.bisect!.current }))}>{tt("Skip")}</button><button disabled={disabled} onClick={() => void act({ action: "bisectReset" }, tt("Finish bisect"), tt("End this bisect and restore the branch or commit from which it started."))}>{tt("Finish / reset…")}</button></div><pre>{state.bisect.log}</pre></div></> : <div className="ga-form"><label>{tt("Known good commit")}<input aria-label={tt("Bisect good commit")} value={good} onChange={(e) => setGood(e.target.value)} disabled={disabled} placeholder={tt("Full commit ID")} /></label><label>{tt("Known bad commit")}<input aria-label={tt("Bisect bad commit")} value={bad} onChange={(e) => setBad(e.target.value)} disabled={disabled} placeholder={tt("Full commit ID")} /></label><button disabled={disabled || !good.trim() || !bad.trim()} onClick={() => void act({ action: "bisectStart", good, bad }, tt("Start bisect"), tt("Test the history between good {good} and bad {bad}.\n\nThe working folder will move to a test revision. Finish / reset returns to the starting revision.", { good, bad }))}>{tt("Start bisect…")}</button></div>}</>
      : tab === "patches" ? <><p className="ga-description">{tt("Export a commit as an email patch, or apply a patch to both the index and working folder. Applying first checks the exact patch against a clean repository.")}</p><div className="ga-form"><h3>{tt("Export selected commit")}</h3><code className="ga-path">{commit ?? tt("Select a commit in the graph first.")}</code><label>{tt("New patch file")}<input aria-label={tt("Patch export destination")} value={exportDestination} onChange={(e) => setExportDestination(e.target.value)} disabled={disabled} placeholder={tt("Absolute path, or leave blank to choose a file")} /></label><button disabled={disabled || !commit} onClick={() => void exportPatch()}>{tt("Export commit patch…")}</button></div><div className="ga-form"><h3>{tt("Apply patch")}</h3><label>{tt("Patch file")}<div className="ga-input-row"><input aria-label={tt("Patch file")} value={patchFile} onChange={(e) => setPatchFile(e.target.value)} placeholder={tt("Absolute .patch or .diff path")} disabled={disabled} /><button disabled={disabled} onClick={() => void browsePatch()}>{tt("Browse…")}</button></div></label><button disabled={disabled || !patchFile.trim()} onClick={() => void act({ action: "applyPatch", file: patchFile }, tt("Apply patch"), tt("Check and apply this patch to the index and working folder:\n{patchFile}\n\nNo commit is created. Existing work must be committed or stashed first.", { patchFile }))}>{tt("Check & apply…")}</button></div></>
      : tab === "notes" ? <><p className="ga-description">{tt("A Git note annotates a commit without changing its ID. Replaced notes remain recoverable through saved references.")}</p><div className="ga-form"><label>{tt("Selected commit")}<code className="ga-path">{commit ?? tt("Select a commit in the graph first.")}</code></label><label>{tt("Commit note")}<textarea aria-label={tt("Git note")} value={note} onChange={(e) => setNote(e.target.value)} disabled={disabled || !commit} placeholder={tt("Add a note to this commit")} /></label><button disabled={disabled || !commit || note === state.notes} onClick={() => commit && void act({ action: "writeNote", commit, message: note }, note.trim() ? tt("Save note") : tt("Remove note"), tt(note.trim() ? "Save this note on commit {commit}.\n\nThe commit ID and working files stay unchanged." : "Remove the note from commit {commit}.\n\nThe commit ID and working files stay unchanged.", { commit }))}>{note.trim() ? tt("Save note…") : tt("Remove note…")}</button></div></>
      : <><p className="ga-description">{tt("Initialize the current folder, or clone into a new folder. Clone failures retain their partial destination for inspection.")}</p>{!state.repository && <div className="ga-form"><h3>{tt("Initialize this folder")}</h3><code className="ga-path">{path}</code><label>{tt("Initial branch")}<input aria-label={tt("Initial branch")} value={initialBranch} onChange={(e) => setInitialBranch(e.target.value)} disabled={disabled} /></label><button disabled={disabled || !initialBranch.trim()} onClick={() => void act({ action: "init", branch: initialBranch }, tt("Initialize repository"), tt("Create a Git repository in this existing folder:\n{path}\n\nInitial branch: {initialBranch}", { path, initialBranch }))}>{tt("Initialize repository…")}</button></div>}<div className="ga-form"><h3>{tt("Clone repository")}</h3><label>{tt("Repository URL")}<input aria-label={tt("Clone URL")} value={url} onChange={(e) => setUrl(e.target.value)} disabled={disabled} placeholder={"https://host/owner/repository.git"} /></label><label>{tt("New absolute destination")}<input aria-label={tt("Clone destination")} value={clonePath} onChange={(e) => setClonePath(e.target.value)} disabled={disabled} placeholder={"/path/to/new-folder"} /></label><button disabled={disabled || !url.trim() || !clonePath.trim()} onClick={() => void act({ action: "clone", url, destination: clonePath }, tt("Clone repository"), tt("Clone {url}\n\nInto a new folder:\n{clonePath}\n\nExisting destinations are rejected. Configured credentials are used; interactive login requires your terminal.", { url, clonePath }))}>{tt("Clone…")}</button></div></>}
    </div>}
    {(busy || output) && <div className="ga-output">{busy && jobProgress ? <GitProgressBar progress={jobProgress} onCancel={activeJob ? () => void invoke("git_advanced_cancel", { id: activeJob }).catch((e) => setError(String(e))) : undefined} cancelLabel={tt("Cancel operation")} /> : <div><b role="status">{busy ? tt("Running Git operation…") : tt("Operation result")}</b>{busy && activeJob && <button onClick={() => void invoke("git_advanced_cancel", { id: activeJob }).catch((e) => setError(String(e)))}>{tt("Cancel operation")}</button>}</div>}<pre>{output || tt("Waiting for Git…")}</pre></div>}
  </section>;
}
