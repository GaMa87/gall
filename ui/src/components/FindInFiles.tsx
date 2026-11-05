// Find in files (Ctrl+F: this folder, Ctrl+Shift+F: whole system).
// ripgrep-powered content search with live results grouped by file and a
// context preview of the selected match.
import { useEffect, useMemo, useRef, useState } from "react";
import { api, events } from "../api/ipc";
import type { FileMatches, GrepOptions, GrepStats } from "../api/types";
import { ancestors, basename, linuxHome, parent, tildify } from "../core/path";
import * as A from "../state/actions";
import { controller } from "../state/controllers";
import { pushRecent, useStore } from "../state/store";
import { FileIcon, Icon } from "./Icon";
import { primaryMod } from "../core/keymap";

type Scope = "folder" | "home" | "root";
const ROW_H = 24;

type Row =
  | { kind: "file"; fi: number; file: FileMatches }
  | { kind: "match"; fi: number; mi: number; file: FileMatches };

let grepSeq = 1;

/** Splits text into plain and highlighted parts from UTF-16 ranges. */
function Ranges({ text, ranges }: { text: string; ranges: [number, number][] }) {
  const out: React.ReactNode[] = [];
  let at = 0;
  ranges.forEach(([s, e], i) => {
    if (s < at) return;
    if (s > at) out.push(text.slice(at, s));
    out.push(<mark key={i}>{text.slice(s, e)}</mark>);
    at = e;
  });
  out.push(text.slice(at));
  return <>{out}</>;
}

function entryOf(path: string) {
  const name = basename(path);
  const dot = name.lastIndexOf(".");
  return { isDir: false, name, ext: dot > 0 ? name.slice(dot + 1).toLowerCase() : "" };
}

export function FindInFiles({ root, scope: initialScope }: { root: string; scope: Scope }) {
  const info = useStore((s) => s.info)!;
  const [pattern, setPattern] = useState("");
  const [scope, setScope] = useState<Scope>(initialScope);
  const [opts, setOpts] = useState({ caseSensitive: false, wholeWord: false, regex: false, everything: false });
  const [include, setInclude] = useState("");
  const [exclude, setExclude] = useState("");
  const [showFilters, setShowFilters] = useState(false);
  const [files, setFiles] = useState<FileMatches[]>([]);
  const [collapsed, setCollapsed] = useState<Set<number>>(new Set());
  const [running, setRunning] = useState(false);
  const [done, setDone] = useState<{ stats: GrepStats; cancelled: boolean } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sel, setSel] = useState<{ fi: number; mi: number } | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewH, setViewH] = useState(400);
  const [startedAt, setStartedAt] = useState(0);
  const id = useRef(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const roots: Record<Scope, string> = {
    folder: root,
    home: linuxHome(root) ?? info.home,
    root: ancestors(root)[0] ?? "/",
  };
  const searchRoot = roots[scope];

  useEffect(() => {
    inputRef.current?.focus();
    const un1 = events.grepBatch((b) => b.id === id.current && setFiles((f) => [...f, ...b.files]));
    const un2 = events.grepDone((d) => {
      if (d.id !== id.current) return;
      setRunning(false);
      setDone({ stats: d.stats, cancelled: d.cancelled });
      setError(d.error);
    });
    const ro = new ResizeObserver(() => setViewH(listRef.current?.clientHeight ?? 400));
    if (listRef.current) ro.observe(listRef.current);
    return () => {
      api.grepCancel();
      un1.then((f) => f());
      un2.then((f) => f());
      ro.disconnect();
    };
  }, []);

  // Live search: restart (and cancel the previous run) as the query changes.
  useEffect(() => {
    if (!pattern) {
      api.grepCancel();
      id.current = 0;
      setFiles([]);
      setRunning(false);
      setDone(null);
      setError(null);
      return;
    }
    const t = setTimeout(() => {
      id.current = grepSeq++;
      setFiles([]);
      setCollapsed(new Set());
      setSel(null);
      setDone(null);
      setError(null);
      setRunning(true);
      setStartedAt(performance.now());
      if (listRef.current) listRef.current.scrollTop = 0;
      const split = (s: string) => s.split(",").map((x) => x.trim()).filter(Boolean);
      const o: Partial<GrepOptions> = {
        case: opts.caseSensitive ? "sensitive" : "smart",
        wholeWord: opts.wholeWord,
        regex: opts.regex,
        include: split(include),
        exclude: split(exclude),
        respectGitignore: !opts.everything,
        everything: opts.everything,
        maxMatches: 20000,
      };
      api.grepStart(id.current, searchRoot, pattern, o).catch(A.fail);
    }, scope === "root" ? 450 : 250);
    return () => clearTimeout(t);
  }, [pattern, opts, include, exclude, searchRoot]);

  // Flattened rows for the virtual list.
  const rows: Row[] = useMemo(() => {
    const r: Row[] = [];
    files.forEach((file, fi) => {
      r.push({ kind: "file", fi, file });
      if (!collapsed.has(fi)) file.matches.forEach((_, mi) => r.push({ kind: "match", fi, mi, file }));
    });
    return r;
  }, [files, collapsed]);
  const totalMatches = useMemo(() => files.reduce((n, f) => n + f.matches.length, 0), [files]);

  // Select the first match as soon as results arrive.
  useEffect(() => {
    if (!sel && files.length) setSel({ fi: 0, mi: 0 });
  }, [files.length, sel]);

  const selRow = sel ? rows.findIndex((r) => r.kind === "match" && r.fi === sel.fi && r.mi === sel.mi) : -1;
  const selFile = sel ? files[sel.fi] : null;
  const selMatch = sel && selFile ? selFile.matches[sel.mi] : null;

  const scrollToRow = (i: number) => {
    const el = listRef.current;
    if (!el || i < 0) return;
    const top = i * ROW_H;
    if (top < el.scrollTop) el.scrollTop = top - ROW_H;
    else if (top + ROW_H > el.scrollTop + el.clientHeight) el.scrollTop = top - el.clientHeight + ROW_H * 2;
  };

  const step = (d: number) => {
    const matches = rows.map((r, i) => [r, i] as const).filter(([r]) => r.kind === "match");
    if (!matches.length) return;
    const cur = matches.findIndex(([, i]) => i === selRow);
    const next = matches[(cur + d + matches.length) % matches.length];
    const r = next[0] as Extract<Row, { kind: "match" }>;
    setSel({ fi: r.fi, mi: r.mi });
    scrollToRow(next[1]);
  };

  const close = () => A.openOverlay(null);
  const reveal = (path: string, target: number) => {
    close();
    A.focusBrowser(target);
    A.navigate(target, parent(path)!);
    setTimeout(() => controller(target)?.select([path], path), 220);
  };
  const open = (how: "open" | "reveal" | "other") => {
    if (!selFile) return;
    pushRecent(selFile.path);
    if (how === "reveal") return reveal(selFile.path, A.activeBrowser());
    if (how === "other") return reveal(selFile.path, A.otherTarget(A.activeBrowser()) ?? A.activeBrowser());
    close();
    A.openEntries(A.activeBrowser(), [{
      name: basename(selFile.path), path: selFile.path, kind: "file", isDir: false, isHidden: false, isSymlink: false,
      size: 0, modified: null, created: null, ext: entryOf(selFile.path).ext, readonly: false,
    }]);
  };

  const onKey = (e: React.KeyboardEvent) => {
    e.stopPropagation();
    if (e.key === "Escape") {
      if (running) api.grepCancel();
      else close();
      return;
    }
    const k = e.key;
    if (k === "ArrowDown" || (k === "F4" && !e.shiftKey) || (e.ctrlKey && (k === "j" || k === "n"))) { e.preventDefault(); step(1); return; }
    if (k === "ArrowUp" || (k === "F4" && e.shiftKey) || (e.ctrlKey && (k === "k" || k === "p"))) { e.preventDefault(); step(-1); return; }
    if (k === "Enter" && (e.target as HTMLElement).tagName === "INPUT" && (e.target as HTMLElement).dataset.role !== "query") return;
    if (k === "Enter") { e.preventDefault(); open(primaryMod(e) ? "reveal" : e.shiftKey ? "other" : "open"); return; }
    if (e.altKey && k.toLowerCase() === "c") { e.preventDefault(); setOpts((o) => ({ ...o, caseSensitive: !o.caseSensitive })); }
    if (e.altKey && k.toLowerCase() === "w") { e.preventDefault(); setOpts((o) => ({ ...o, wholeWord: !o.wholeWord })); }
    if (e.altKey && k.toLowerCase() === "r") { e.preventDefault(); setOpts((o) => ({ ...o, regex: !o.regex })); }
    if (e.altKey && k.toLowerCase() === "i") { e.preventDefault(); setOpts((o) => ({ ...o, everything: !o.everything })); }
  };

  const first = Math.max(0, Math.floor(scrollTop / ROW_H) - 10);
  const last = Math.min(rows.length, Math.ceil((scrollTop + viewH) / ROW_H) + 10);
  const home = info.home;
  const tilde = (p: string) => tildify(p, home);
  const elapsed = done ? done.stats.elapsedMs : running ? performance.now() - startedAt : 0;

  return (
    <div className="backdrop glass center" onMouseDown={close}>
      <div className="fif" onMouseDown={(e) => e.stopPropagation()} tabIndex={-1} onKeyDown={onKey}>
        <div className="fif-head">
          <div className="fif-query">
            <Icon name="search" size={17} />
            <input ref={inputRef} data-role="query" value={pattern} spellCheck={false} placeholder="Find in files…" onChange={(e) => setPattern(e.target.value)} />
            <button className={`tgl ${opts.caseSensitive ? "on" : ""}`} title="Match case (Alt+C)" onClick={() => setOpts((o) => ({ ...o, caseSensitive: !o.caseSensitive }))}>Aa</button>
            <button className={`tgl ${opts.wholeWord ? "on" : ""}`} title="Whole word (Alt+W)" onClick={() => setOpts((o) => ({ ...o, wholeWord: !o.wholeWord }))}><u>ab</u></button>
            <button className={`tgl ${opts.regex ? "on" : ""}`} title="Regular expression (Alt+R)" onClick={() => setOpts((o) => ({ ...o, regex: !o.regex }))}>.*</button>
            <button className={`tgl ${opts.everything ? "on" : ""}`} title="Also search ignored files: node_modules and the other excluded folders, .gitignore, .gallignore, cache folders (Alt+I)" onClick={() => setOpts((o) => ({ ...o, everything: !o.everything }))}><Icon name="eye" size={15} /></button>
            <button className={`tgl ${showFilters || include || exclude ? "on" : ""}`} title="Include / exclude files" onClick={() => setShowFilters((v) => !v)}><Icon name="more" size={15} /></button>
          </div>
          <div className="seg" role="tablist">
            {(["folder", "home", "root"] as Scope[]).map((s) => (
              <button key={s} className={scope === s ? "on" : ""} title={roots[s]} onClick={() => { setScope(s); inputRef.current?.focus(); }}>
                <Icon name={s === "folder" ? "folder" : s === "home" ? "home" : "drive"} size={13} />
                {s === "folder" ? tilde(root) : s === "home" ? "Home" : roots.root}
              </button>
            ))}
          </div>
          {showFilters && (
            <div className="fif-filters">
              <label>include<input value={include} spellCheck={false} placeholder="*.rs, src/**" onChange={(e) => setInclude(e.target.value)} /></label>
              <label>exclude<input value={exclude} spellCheck={false} placeholder="*.min.js, vendor/" onChange={(e) => setExclude(e.target.value)} /></label>
            </div>
          )}
        </div>

        <div className="fif-body">
          <div className="fif-list" ref={listRef} onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}>
            <div style={{ height: rows.length * ROW_H, position: "relative" }}>
              {rows.slice(first, last).map((r, k) => {
                const i = first + k;
                const style = { top: i * ROW_H, height: ROW_H };
                if (r.kind === "file") {
                  const dir = parent(r.file.path) ?? "";
                  return (
                    <div key={"f" + r.fi} className="fif-file" style={style} onClick={() => setCollapsed((c) => { const n = new Set(c); n.has(r.fi) ? n.delete(r.fi) : n.add(r.fi); return n; })}>
                      <Icon name={collapsed.has(r.fi) ? "chevronRight" : "chevronDown"} size={12} />
                      <FileIcon entry={entryOf(r.file.path)} size={15} />
                      <span className="n">{basename(r.file.path)}</span>
                      <span className="d"><bdi>{tilde(dir)}</bdi></span>
                      <span className="badge">{r.file.matches.length}</span>
                    </div>
                  );
                }
                const m = r.file.matches[r.mi];
                const on = sel?.fi === r.fi && sel?.mi === r.mi;
                return (
                  <div key={`m${r.fi}:${r.mi}`} className={`fif-match ${on ? "on" : ""}`} style={style}
                    onClick={() => setSel({ fi: r.fi, mi: r.mi })} onDoubleClick={() => open("open")}>
                    <span className="ln">{m.line}</span>
                    <span className="t"><Ranges text={m.text} ranges={m.ranges} /></span>
                  </div>
                );
              })}
            </div>
            {!rows.length && (
              <div className="fif-empty">
                {error ? <span className="err">⚠ {error}</span>
                  : !pattern ? <>
                      <Icon name="search" size={34} />
                      <div>Search the contents of every text file in <b>{tilde(searchRoot)}</b></div>
                      <small>{opts.everything ? "Searching ignored files too" : "Respects .gitignore and .gallignore"} · skips binary files · <kbd>Alt+C</kbd> case <kbd>Alt+W</kbd> word <kbd>Alt+R</kbd> regex <kbd>Alt+I</kbd> ignored files</small>
                    </>
                  : running ? "Searching…" : "No results"}
              </div>
            )}
          </div>
          <ContextPreview file={selFile?.path ?? null} line={selMatch?.line ?? 0} pattern={pattern} opts={opts} tilde={tilde} />
        </div>

        <div className="fif-foot">
          <span className="fif-status">
            {running && <span className="spinner" />}
            {pattern
              // One span: in the flex status bar the spaces around <b> were dropped ("1matches in1files").
              ? <span><b>{totalMatches.toLocaleString()}</b> {totalMatches === 1 ? "match" : "matches"} in <b>{files.length.toLocaleString()}</b> {files.length === 1 ? "file" : "files"}
                  {done && <> · {done.stats.searched.toLocaleString()} searched</>}
                  {done && done.stats.cloudOnly > 0 && (
                    <span title="Files kept only in the cloud (OneDrive and the like) are not searched: reading them would download every one. Make them available offline to search them.">
                      {" "}· {A.items(done.stats.cloudOnly)} only in the cloud, not searched
                    </span>
                  )}
                  {elapsed > 0 && <> · {(elapsed / 1000).toFixed(1)} s</>}
                  {done?.stats.truncated && <> · <span className="warn">limit reached</span></>}
                  {done?.cancelled && <> · stopped</>}</span>
              : <span>Scope: {tilde(searchRoot)}</span>}
            {running && <button className="link" onClick={() => api.grepCancel()}>Stop</button>}
          </span>
          <span className="qo-hints"><kbd>↑↓</kbd> next match <kbd>↵</kbd> open <kbd>Ctrl ↵</kbd> reveal <kbd>⇧ ↵</kbd> other side <kbd>Esc</kbd> {running ? "stop" : "close"}</span>
        </div>
      </div>
    </div>
  );
}

/** Lines around the selected match, with the match line and hits highlighted. */
function ContextPreview({ file, line, pattern, opts, tilde }: {
  file: string | null; line: number; pattern: string;
  opts: { caseSensitive: boolean; wholeWord: boolean; regex: boolean }; tilde: (p: string) => string;
}) {
  const [lines, setLines] = useState<[number, string][]>([]);
  const boxRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!file) return setLines([]);
    let alive = true;
    const t = setTimeout(() => api.grepContext(file, line, 14, 40).then((l) => alive && setLines(l)).catch(() => alive && setLines([])), 40);
    return () => { alive = false; clearTimeout(t); };
  }, [file, line]);
  useEffect(() => {
    // Scroll only the preview box: scrollIntoView would also scroll every ancestor, including the
    // overflow:hidden app root, leaving the whole UI shifted once the overlay closes.
    const box = boxRef.current;
    const hit = box?.querySelector<HTMLElement>(".hit");
    if (!box || !hit) return;
    const off = hit.getBoundingClientRect().top - box.getBoundingClientRect().top + box.scrollTop;
    box.scrollTop = off - (box.clientHeight - hit.offsetHeight) / 2;
  }, [lines]);

  const re = useMemo(() => {
    if (!pattern) return null;
    try {
      const src = opts.regex ? pattern : pattern.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      const smartCi = !opts.caseSensitive && !/[A-Z]/.test(pattern);
      return new RegExp(opts.wholeWord ? `\\b(?:${src})\\b` : src, smartCi ? "gi" : "g");
    } catch {
      return null;
    }
  }, [pattern, opts]);

  const mark = (text: string) => {
    if (!re) return text;
    const ranges: [number, number][] = [];
    re.lastIndex = 0;
    for (let m = re.exec(text); m && ranges.length < 50; m = re.exec(text)) {
      if (m[0].length === 0) { re.lastIndex++; continue; }
      ranges.push([m.index, m.index + m[0].length]);
    }
    return <Ranges text={text} ranges={ranges} />;
  };

  if (!file) return <div className="fif-preview empty" />;
  return (
    <div className="fif-preview">
      <div className="fif-pv-head" title={file}><FileIcon entry={entryOf(file)} size={14} /> {tilde(file)}<span className="ln">:{line}</span></div>
      <div className="fif-pv-code" ref={boxRef}>
        {lines.map(([n, t]) => (
          <div key={n} className={n === line ? "hit" : ""}><span className="ln">{n}</span><span className="t">{mark(t)}</span></div>
        ))}
      </div>
    </div>
  );
}

