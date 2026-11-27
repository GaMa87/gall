import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { age, ageWords } from "../core/age";
import type { GitStash } from "../core/gitRepository";
import { patchFiles, searchStashes, stashName, type StashInfo } from "../core/gitStash";
import { tt } from "../i18n";
import { GitProgressBar, useLocalProgress } from "./GitProgressBar";
import "./GitStashes.css";

/** Previews of stashes; a stash never changes, so they are kept. */
const previews = new Map<string, Promise<{ patch: string; truncated: boolean }>>();
function previewOf(path: string, id: string) {
  const key = `${path}\0${id}`;
  let value = previews.get(key);
  if (!value) {
    value = invoke<{ patch: string; truncated: boolean }>("git_stash_preview", { path, id });
    value.catch(() => previews.delete(key));
    previews.set(key, value);
    if (previews.size > 24) previews.delete(previews.keys().next().value!);
  }
  return value;
}

type ListProps = { path: string; stashes: GitStash[]; row: (stash: GitStash, label: string, subtitle: string, title: string) => ReactNode; onFirst: () => void; onLeave: () => void };

/** The Stashes tab: name, branch, age and files of every stash, with a search over names and files. */
export function GitStashList({ path, stashes, row, onFirst, onLeave }: ListProps) {
  const [infos, setInfos] = useState<ReadonlyMap<string, StashInfo>>(new Map()), [query, setQuery] = useState("");
  const ids = stashes.map((stash) => stash.id).join(" ");
  // Read when the tab is open, once for all stashes and again only when the list changed.
  useEffect(() => {
    if (!ids) { setInfos(new Map()); return; }
    let alive = true;
    invoke<StashInfo[]>("git_stash_list", { path }).then((list) => { if (alive) setInfos(new Map(list.map((info) => [info.id, info]))); }).catch(() => {});
    return () => { alive = false; };
  }, [path, ids]);
  const found = useMemo(() => searchStashes(stashes, infos, query), [stashes, infos, query]);
  const now = Date.now();
  if (!stashes.length) return <p className="gg-empty">{tt("No saved stashes")}</p>;
  return <>
    <input className="gs-search" type="search" aria-label={tt("Search stashes")} placeholder={tt("Name or file…")} spellCheck={false} value={query} onChange={(e) => setQuery(e.target.value)} onKeyDown={(e) => {
      if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); if (query) setQuery(""); else onLeave(); }
      else if (e.key === "Enter" || e.key === "ArrowDown") { e.preventDefault(); e.stopPropagation(); onFirst(); }
    }} />
    {found.map(({ stash, file }) => {
      const { branch, name } = stashName(stash.message), info = infos.get(stash.id), time = info ? info.time * 1000 : 0;
      const parts = [branch, info && age(time, now).label, info && tt(info.files.length === 1 ? "{count} file" : "{count} files", { count: info.files.length }), file].filter(Boolean);
      return row(stash, name || stash.selector, [stash.selector, ...parts].join(" · "), [stash.message, info && tt("Saved {value} ago", { value: ageWords(time, now) }), ...(info?.files.slice(0, 20) ?? [])].filter(Boolean).join("\n"));
    })}
    {!found.length && <p className="gg-empty">{tt("No stash matches.")}</p>}
  </>;
}

/** The changes of the selected stash, new files included. */
export function GitStashPreview({ path, stash, onClose, onBack, actions }: { path: string; stash: GitStash; onClose: () => void; onBack: () => void; actions: ReactNode }) {
  const [loaded, setLoaded] = useState<{ id: string; patch: string; truncated: boolean } | null>(null), [error, setError] = useState("");
  const body = useRef<HTMLPreElement>(null);
  useEffect(() => {
    let alive = true;
    setError("");
    // Moving quickly over the list asks only for the stash one stays on.
    const timer = setTimeout(() => {
      previewOf(path, stash.id).then((value) => { if (alive) setLoaded({ id: stash.id, ...value }); }).catch((e) => { if (alive) setError(String(e)); });
    }, previews.has(`${path}\0${stash.id}`) ? 0 : 120);
    return () => { alive = false; clearTimeout(timer); };
  }, [path, stash.id]);
  const shown = loaded?.id === stash.id ? loaded : null;
  const loadProgress = useLocalProgress(!shown && !error, "Loading stash");
  const lines = useMemo(() => shown?.patch.split("\n", 20001) ?? [], [shown]);
  const files = useMemo(() => patchFiles(lines), [lines]);
  const { branch, name } = stashName(stash.message);
  return <section className="gs-preview" aria-label={tt("Stash preview")} tabIndex={-1} onKeyDown={(e) => {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(); }
    else if (e.key === "h" && !e.ctrlKey && !e.altKey && !e.metaKey) { e.preventDefault(); e.stopPropagation(); onBack(); }
  }}>
    <div className="gs-head"><div><strong>{name || stash.selector}</strong><small>{[stash.selector, branch, stash.id.slice(0, 8)].filter(Boolean).join(" · ")}</small></div><div className="gs-actions">{actions}<button onClick={onClose} aria-label={tt("Close stash preview")} title={tt("Close (Esc)")}>✕</button></div></div>
    {error ? <p className="gs-error" role="alert">{error}</p> : !shown ? (loadProgress ? <GitProgressBar progress={loadProgress} compact /> : <p className="gg-empty" role="status">{tt("Loading stash…")}</p>) : <>
      <div className="gs-files">{files.map((file) => <button key={file.line} title={file.path} onClick={() => body.current?.querySelector(`[data-line="${file.line}"]`)?.scrollIntoView({ block: "start" })}><span>{file.path}</span><em className="gcd-added">+{file.added}</em><em className="gcd-removed">−{file.deleted}</em></button>)}{!files.length && <p className="gg-empty">{tt("No textual difference")}</p>}</div>
      <pre className="gcd-patch gs-patch" ref={body} tabIndex={0}>{lines.slice(0, 20000).map((line, i) => <span key={i} data-line={line.startsWith("diff --git ") ? i : undefined} className={line.startsWith("diff --git ") ? "gs-file" : line.startsWith("@@") ? "gcd-hunk" : line.startsWith("+") && !line.startsWith("+++ ") ? "gcd-added" : line.startsWith("-") && !line.startsWith("--- ") ? "gcd-removed" : ""}>{line}{"\n"}</span>)}{(shown.truncated || lines.length > 20000) && <span className="gcd-dim">{tt("[The preview is limited to 1 MiB and 20,000 lines]")}</span>}</pre>
    </>}
  </section>;
}
