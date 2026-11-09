import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useStore } from "../state/store";
import * as A from "../state/actions";
import { api } from "../api/ipc";
import { joinRel } from "../core/path";
import "./GitCommitDetails.css";

type ChangedFile = { path: string; original: string | null; status: string; added: number | null; deleted: number | null };
type Details = { root: string; id: string; parents: string[]; author: string; email: string; date: string; committer: string; commitDate: string; message: string; base: string | null; files: ChangedFile[] };
type HistoryEntry = { id: string; subject: string; author: string; date: string };
type BlameLine = { commit: string; line: number; originalLine: number; author: string; date: string; text: string };
type View = "diff" | "file" | "history" | "blame";
type Props = { path: string; commit: string; base?: string; onClose: () => void; onBack?: () => void; onNavigate: (id: string) => void; onAction?: (action: string) => void };

export function GitCommitDetails({ path, commit, base, onClose, onBack, onNavigate, onAction }: Props) {
  const [details, setDetails] = useState<Details | null>(null);
  const [parent, setParent] = useState<{ commit: string; id: string } | null>(null);
  const [selected, setSelected] = useState(0);
  const [view, setView] = useState<View>("diff");
  const [patch, setPatch] = useState("");
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [blame, setBlame] = useState<BlameLine[]>([]);
  const [error, setError] = useState("");
  const [previewError, setPreviewError] = useState("");
  const [loading, setLoading] = useState(true);
  const [loadingPreview, setLoadingPreview] = useState(false);
  const [copied, setCopied] = useState(false);
  const root = useRef<HTMLElement>(null);
  const fileList = useRef<HTMLDivElement>(null);
  const preview = useRef<HTMLDivElement>(null);
  const vim = useStore((s) => s.settings?.vimKeys ?? false);
  const effectiveBase = base ?? (parent?.commit === commit ? parent.id : undefined);
  const current = details?.files[selected];
  const deleted = current?.status.startsWith("D") ?? false;
  const inspectedCommit = deleted ? details?.base : details?.id;

  useEffect(() => {
    let active = true;
    setDetails(null); setLoading(true); setError(""); setSelected(0); setView("diff"); setCopied(false);
    invoke<Details>("git_commit_details", { path, commit, base: effectiveBase ?? null }).then((value) => {
      if (active) setDetails(value);
    }).catch((e) => { if (active) setError(String(e)); }).finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [path, commit, effectiveBase]);

  useEffect(() => {
    let active = true;
    setPatch(""); setHistory([]); setBlame([]); setPreviewError("");
    if (!current || !details) { setLoadingPreview(false); return; }
    setLoadingPreview(true);
    const load = async () => {
      if (view === "history") {
        const value = await invoke<HistoryEntry[]>("git_file_history", { path, file: current.path, limit: 200, commit: details.id });
        if (active) setHistory(value);
      } else if (view === "blame") {
        if (!inspectedCommit) throw new Error("This file has no revision to annotate.");
        const value = await invoke<BlameLine[]>("git_blame", { path, commit: inspectedCommit, file: current.path });
        if (active) setBlame(value);
      } else if (view === "file") {
        if (!inspectedCommit) throw new Error("This file has no revision to preview.");
        const value = await invoke<string>("git_commit_content", { path, commit: inspectedCommit, file: current.path });
        if (active) setPatch(value);
      } else {
        const value = await invoke<string>("git_commit_patch", { path, commit: details.id, base: effectiveBase ?? null, file: current.path });
        if (active) setPatch(value);
      }
    };
    void load().catch((e) => { if (active) setPreviewError(String(e)); }).finally(() => { if (active) setLoadingPreview(false); });
    return () => { active = false; };
  }, [path, current?.path, details, effectiveBase, inspectedCommit, view]);

  useEffect(() => { fileList.current?.querySelector(".selected")?.scrollIntoView({ block: "nearest" }); }, [selected]);

  const copy = async () => {
    try { await invoke("clip_set_text", { text: commit }); setCopied(true); }
    catch (e) { setError(String(e)); }
  };
  const copyPath = async () => {
    if (!current) return;
    try { await invoke("clip_set_text", { text: current.path }); A.notify("File path copied"); }
    catch (e) { setError(String(e)); }
  };
  const edit = async () => {
    if (!details || !current) return;
    try {
      const resolved = await api.pathResolve(joinRel(details.root, current.path));
      if (!resolved.exists || resolved.isDir) throw new Error("This historical file is not present in the working folder.");
      A.editPaths([resolved.path]);
    } catch (e) { setError(String(e)); }
  };
  useEffect(() => {
    const element = root.current;
    const command = (event: Event) => {
      const key = (event as CustomEvent<string>).detail;
      if (key === "b") setView("blame");
      else if (key === "f") setView("history");
      else if (key === "d") setView("diff");
      else if (key === "v") setView("file");
      else if (key === "Enter") preview.current?.focus();
      else if (key === "y") void copyPath();
      else if (key === "e") void edit();
    };
    element?.addEventListener("gall-git-command", command);
    return () => element?.removeEventListener("gall-git-command", command);
  }, [current, details]);
  const move = (direction: number) => setSelected((value) => Math.max(0, Math.min(value + direction, (details?.files.length ?? 1) - 1)));
  const totalAdded = details?.files.reduce((sum, file) => sum + (file.added ?? 0), 0) ?? 0;
  const totalDeleted = details?.files.reduce((sum, file) => sum + (file.deleted ?? 0), 0) ?? 0;
  const patchLines = patch.split("\n", 20001);
  const visibleBlame = blame.slice(0, 10000);

  return <section className="git-commit-details" ref={root} aria-label={base ? "Commit comparison" : "Commit details"} tabIndex={-1} onKeyDown={(e) => {
    if ((e.target as HTMLElement).closest("input,textarea,select")) return;
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(); return; }
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    e.stopPropagation();
    const inPreview = preview.current?.contains(e.target as Node);
    if (vim && e.key === "h") { e.preventDefault(); if (inPreview) fileList.current?.focus(); else onBack?.(); }
    else if (vim && e.key === "l") { e.preventDefault(); preview.current?.focus(); }
    else if (vim && e.key === "b") { e.preventDefault(); setView("blame"); }
    else if (vim && e.key === "f") { e.preventDefault(); setView("history"); }
    else if (vim && e.key === "d") { e.preventDefault(); setView("diff"); }
    else if (vim && e.key === "v") { e.preventDefault(); setView("file"); }
    else if (vim && e.key === "y") { e.preventDefault(); void copyPath(); }
    else if (vim && e.key === "e") { e.preventDefault(); void edit(); }
    else if (e.key === "Enter" && !(e.target as HTMLElement).closest("button")) { e.preventDefault(); preview.current?.focus(); }
    else if (e.key === "ArrowDown" || (vim && e.key === "j")) {
      if (!inPreview) { e.preventDefault(); move(1); }
      else if (e.key === "j") { e.preventDefault(); preview.current?.scrollBy(0, 24); }
    } else if (e.key === "ArrowUp" || (vim && e.key === "k")) {
      if (!inPreview) { e.preventDefault(); move(-1); }
      else if (e.key === "k") { e.preventDefault(); preview.current?.scrollBy(0, -24); }
    }
  }}>
    <div className="gcd-heading"><div><strong>{base ? "Commit comparison" : "Commit details"}</strong><button className="gcd-hash" title="Copy full commit ID" onClick={() => void copy()}>{commit}{copied && " · Copied"}</button></div><button onClick={onClose} aria-label="Close commit details">✕</button></div>
    {error && <p className="gcd-error" role="alert">{error}</p>}
    {loading ? <p className="gcd-notice" role="status">Loading commit…</p> : !details ? <p className="gcd-notice">Commit could not be loaded.</p> : <>
      <div className="gcd-metadata"><div className="gcd-identity"><span><b>{details.author}</b> <span className="gcd-dim">&lt;{details.email}&gt;</span></span><time dateTime={details.date}>{new Date(details.date).toLocaleString()}</time><span className="gcd-dim" title={`Committed ${details.commitDate}`}>Committer: {details.committer}</span></div><pre className="gcd-message">{details.message}</pre><div className="gcd-parents"><span className="gcd-dim">{details.parents.length ? "Parents" : "Root commit"}</span>{details.parents.map((id) => <button key={id} title={id} onClick={() => onNavigate(id)}>{id.slice(0, 8)}</button>)}{onAction && <button className="gcd-actions" onClick={() => onAction("menu")}>Commit actions…</button>}</div></div>
      <div className="gcd-toolbar"><span className="gcd-count">{details.files.length} {details.files.length === 1 ? "file" : "files"} <span className="gcd-added">+{totalAdded}</span> <span className="gcd-removed">−{totalDeleted}</span></span>{base ? <span className="gcd-base" title={`${base} → ${commit}`}>{base.slice(0, 8)} → {commit.slice(0, 8)}</span> : details.parents.length > 1 ? <label>Against parent <select aria-label="Comparison parent" value={effectiveBase ?? details.parents[0]} onChange={(e) => setParent({ commit, id: e.target.value })}>{details.parents.map((id, index) => <option key={id} value={id}>Parent {index + 1} · {id.slice(0, 8)}</option>)}</select></label> : <span className="gcd-base">{details.base ? `Against ${details.base.slice(0, 8)}` : "Against empty tree"}</span>}<div className="gcd-tabs" role="group" aria-label="File views">{([ ["diff", "Diff"], ["file", "File"], ["history", "History"], ["blame", "Blame"] ] as const).map(([id, label]) => <button key={id} disabled={!current} aria-pressed={view === id} className={view === id ? "on" : ""} onClick={() => setView(id)}>{label}</button>)}</div></div>
      {details.files.length === 0 ? <p className="gcd-notice">No file changes between these revisions.</p> : <div className="gcd-body"><div className="gcd-files" ref={fileList} tabIndex={0} aria-label="Commit files">{details.files.map((file, index) => <button key={file.path} className={`gcd-file ${index === selected ? "selected" : ""}`} title={file.original ? `${file.original} → ${file.path}` : file.path} onFocus={() => setSelected(index)} onClick={() => { setSelected(index); fileList.current?.focus(); }}><span className={`gcd-status gcd-status-${file.status[0]}`}>{file.status[0]}</span><span className="gcd-file-path">{file.original && <small>{file.original} → </small>}{file.path}</span><span className="gcd-file-count">{file.added === null ? "Binary" : <><span className="gcd-added">+{file.added}</span> <span className="gcd-removed">−{file.deleted}</span></>}</span></button>)}</div><div className="gcd-preview" ref={preview} tabIndex={0} aria-label={view === "diff" ? "Commit file diff" : view === "file" ? "Historical file" : view === "history" ? "File history" : "File blame"}>
        {loadingPreview ? <p className="gcd-notice" role="status">Loading {view}…</p> : previewError ? <p className="gcd-error" role="alert">{previewError}</p> : view === "history" ? history.length === 0 ? <p className="gcd-notice">No file history at this revision.</p> : history.map((entry) => <button className="gcd-history" key={entry.id} title={entry.id} onClick={() => onNavigate(entry.id)}><code>{entry.id.slice(0, 8)}</code><span>{entry.subject}</span><small>{entry.author} · {entry.date.slice(0, 10)}</small></button>) : view === "blame" ? <div className="gcd-blame">{deleted && <p className="gcd-notice">Showing the file before deletion.</p>}{blame.length === 0 ? <p className="gcd-notice">This file is empty.</p> : visibleBlame.map((line) => <div className="gcd-blame-line" key={line.line}><button title={`${line.author} · ${line.date}\nOriginal line ${line.originalLine}`} onClick={() => onNavigate(line.commit)}><code>{line.commit.slice(0, 8)}</code><span>{line.author}</span></button><span className="gcd-line-number">{line.line}</span><code>{line.text || " "}</code></div>)}{blame.length > visibleBlame.length && <p className="gcd-notice">Blame preview limited to 10,000 lines.</p>}</div> : <pre className="gcd-patch">{deleted && view === "file" && <span className="gcd-dim">[File before deletion]{"\n"}</span>}{patch ? view === "diff" ? <>{patchLines.slice(0, 20000).map((line, index) => <span key={index} className={line.startsWith("+") && !line.startsWith("+++") ? "gcd-added" : line.startsWith("-") && !line.startsWith("---") ? "gcd-removed" : line.startsWith("@@") ? "gcd-hunk" : undefined}>{line}{"\n"}</span>)}{patchLines.length > 20000 && <span className="gcd-dim">[Diff preview limited to 20,000 lines]</span>}</> : patch : view === "file" ? "Empty file" : "No textual difference"}</pre>}
      </div></div>}
      <div className="gcd-foot">{vim ? "j/k files · h/l focus · d diff · v file · f history · b blame · y copy path · e edit · Esc close details" : "Arrow keys select files · Tab selects controls · Esc closes details"}</div>
    </>}
  </section>;
}
