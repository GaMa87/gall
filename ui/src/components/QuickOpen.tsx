// Quick Open (Ctrl+P): instant fuzzy search over the whole-system file index,
// with fzf-style query syntax, a live preview and a ">" command palette mode.
import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { api, events } from "../api/ipc";
import type { FileEntry, IndexHit, IndexStatus } from "../api/types";
import { fuzzyMatch } from "../core/fuzzy";
import { ACTION_LABELS, buildKeymap, type Action } from "../core/keymap";
import { basename, isInside, parent, tildify } from "../core/path";
import { listBindings, treeForSettings, showKeys } from "../core/vim";
import * as A from "../state/actions";
import { controller } from "../state/controllers";
import { run } from "../state/run";
import { getState, pushRecent, useStore } from "../state/store";
import { FileIcon, Icon } from "./Icon";
import { Preview, usePreviewInfo } from "./Preview";

/** Actions that only make sense as list motions, not as palette commands. */
const HIDDEN_COMMANDS = new Set<Action>(["cursorDown", "cursorUp", "cursorTop", "cursorBottom", "halfPageDown", "halfPageUp", "enter", "selectDown", "contextMenu", "commandPalette",
  "tab1", "tab2", "tab3", "tab4", "tab5", "tab6", "tab7", "tab8", "tab9"]);

type Row =
  | { kind: "file"; hit: IndexHit; recent?: boolean }
  | { kind: "cmd"; action: Action; label: string; chord?: string; vim?: string; positions: number[] };

/** Renders `text` with the characters at `positions` (UTF-16 offsets, shifted by `offset`) emphasised. */
export function Highlight({ text, positions, offset = 0 }: { text: string; positions: Iterable<number>; offset?: number }) {
  const set = new Set<number>();
  for (const p of positions) if (p - offset >= 0 && p - offset < text.length) set.add(p - offset);
  if (!set.size) return <>{text}</>;
  const parts: { s: string; on: boolean }[] = [];
  for (let i = 0; i < text.length; i++) {
    const on = set.has(i);
    const last = parts[parts.length - 1];
    if (last && last.on === on) last.s += text[i];
    else parts.push({ s: text[i], on });
  }
  return <>{parts.map((p, i) => (p.on ? <mark key={i}>{p.s}</mark> : <Fragment key={i}>{p.s}</Fragment>))}</>;
}

function pseudoEntry(path: string, isDir: boolean): FileEntry {
  const name = basename(path);
  const dot = name.lastIndexOf(".");
  return {
    name, path, kind: isDir ? "dir" : "file", isDir, isHidden: name.startsWith("."), isSymlink: false,
    size: 0, modified: null, created: null, ext: !isDir && dot > 0 ? name.slice(dot + 1).toLowerCase() : "", readonly: false,
  };
}

let querySeq = 0;

export function QuickOpen({ initial }: { initial?: string }) {
  const settings = useStore((s) => s.settings)!;
  const recent = useStore((s) => s.recentFiles);
  const history = useStore((s) => s.history);
  const [q, setQ] = useState(initial ?? "");
  const [scopeHere, setScopeHere] = useState(false);
  const [hits, setHits] = useState<IndexHit[]>([]);
  const [stats, setStats] = useState<{ total: number; ms: number } | null>(null);
  const [status, setStatus] = useState<IndexStatus | null>(null);
  const [idx, setIdx] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const b = A.activeBrowser();
  const here = A.activePath(b);
  const cmdMode = q.startsWith(">");

  useEffect(() => {
    inputRef.current?.focus();
    api.indexStatus().then(setStatus).catch(() => {});
    api.indexRefresh(false).then((started) => { if (started) api.indexStatus().then(setStatus); }).catch(() => {});
    const un = events.indexUpdated((s) => {
      setStatus(s);
      runQuery.current(true);
    });
    return () => void un.then((f) => f());
  }, []);

  // ───────── file query (debounced, stale responses dropped) ─────────
  // The highlighted file, so a refresh of the same query keeps it highlighted.
  const selectedPath = useRef<string | null>(null);
  const runQuery = useRef((_keep?: boolean) => {});
  /** `keep`: the index changed under the same query (it is refreshed when Quick Open
   *  opens): the highlighted file stays highlighted, so Enter opens what was chosen. */
  runQuery.current = (keep = false) => {
    if (cmdMode || !q.trim()) {
      setHits([]);
      setStats(null);
      return;
    }
    const id = ++querySeq;
    api.indexQuery(q, scopeHere ? here : null, 200)
      .then((r) => {
        if (id !== querySeq) return;
        setHits(r.hits);
        setStats({ total: r.total, ms: r.elapsedMs });
        const at = keep && selectedPath.current ? r.hits.findIndex((h) => h.path === selectedPath.current) : -1;
        setIdx(at >= 0 ? at : 0);
      })
      .catch(A.fail);
  };
  useEffect(() => {
    const t = setTimeout(() => runQuery.current(), 25);
    return () => clearTimeout(t);
  }, [q, scopeHere]);

  // ───────── command palette rows ─────────
  const commands = useMemo(() => {
    const km = buildKeymap(settings.shortcuts);
    const chords = new Map<Action, string>();
    for (const [c, a] of km) if (!chords.has(a)) chords.set(a, c);
    const vim = new Map<Action, string>();
    for (const r of listBindings(treeForSettings(settings))) if (!vim.has(r.action)) vim.set(r.action, showKeys(r.keys));
    return (Object.keys(ACTION_LABELS) as Action[])
      .filter((a) => !HIDDEN_COMMANDS.has(a))
      .map((a) => ({ action: a, label: ACTION_LABELS[a], chord: chords.get(a), vim: vim.get(a) }));
  }, [settings]);

  const rows: Row[] = useMemo(() => {
    if (cmdMode) {
      const term = q.slice(1);
      return commands
        .map((c) => ({ c, m: fuzzyMatch(term, c.label) }))
        .filter((x) => x.m)
        .sort((x, y) => y.m!.score - x.m!.score)
        .map(({ c, m }) => ({ kind: "cmd" as const, ...c, positions: m!.positions }));
    }
    if (!q.trim()) {
      // Empty query: recently opened files, then recently visited folders.
      const seen = new Set<string>();
      const list: Row[] = [];
      for (const p of [...recent, ...history]) {
        if (seen.has(p) || (scopeHere && !isInside(p, here))) continue;
        seen.add(p);
        const isDir = !recent.includes(p);
        list.push({ kind: "file", recent: true, hit: { path: p, name: basename(p), isDir, score: 0, positions: [] } });
        if (list.length >= 30) break;
      }
      return list;
    }
    return hits.map((hit) => ({ kind: "file" as const, hit }));
  }, [cmdMode, q, commands, hits, recent, history, scopeHere, here]);

  const cur = rows[Math.min(idx, rows.length - 1)];
  selectedPath.current = cur?.kind === "file" ? cur.hit.path : null;
  const curEntry = cur?.kind === "file" ? pseudoEntry(cur.hit.path, cur.hit.isDir) : null;
  const info = usePreviewInfo(curEntry);

  useEffect(() => {
    listRef.current?.querySelector(".qo-row.on")?.scrollIntoView({ block: "nearest" });
  }, [idx]);

  // ───────── actions ─────────
  const close = () => A.openOverlay(null);

  const reveal = (path: string, target: number) => {
    const dir = parent(path);
    close();
    if (!dir) return;
    A.focusBrowser(target);
    A.navigate(target, dir);
    setTimeout(() => controller(target)?.select([path], path), 220);
  };

  const accept = (row: Row | undefined, how: "open" | "reveal" | "other") => {
    if (!row) return;
    if (row.kind === "cmd") {
      close();
      setTimeout(() => run(row.action), 0);
      return;
    }
    const { path, isDir } = row.hit;
    pushRecent(path);
    if (how === "reveal") return reveal(path, b);
    if (how === "other") {
      // No other browser on screen (Classic, a narrow window): this one.
      const o = A.otherTarget(b) ?? b;
      if (isDir) {
        close();
        A.focusBrowser(o);
        A.navigate(o, path);
      } else reveal(path, o);
      return;
    }
    close();
    if (isDir) A.navigate(b, path);
    else A.openEntries(b, [pseudoEntry(path, false)]);
  };

  const onKey = (e: React.KeyboardEvent) => {
    e.stopPropagation();
    const n = rows.length;
    const move = (d: number) => {
      e.preventDefault();
      if (n) setIdx((i) => (Math.min(i, n - 1) + d + n) % n);
    };
    if (e.key === "Escape") return close();
    if (e.key === "ArrowDown" || (e.ctrlKey && (e.key === "j" || e.key === "n"))) return move(1);
    if (e.key === "ArrowUp" || (e.ctrlKey && (e.key === "k" || e.key === "p"))) return move(-1);
    if (e.key === "PageDown") return move(10);
    if (e.key === "PageUp") return move(-10);
    if (e.key === "Tab" && !cmdMode) {
      e.preventDefault();
      setScopeHere((v) => !v);
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      accept(cur, e.ctrlKey || e.altKey ? "reveal" : e.shiftKey ? "other" : "open");
    }
  };

  const indexing = status?.building;
  const placeholder = cmdMode ? "Type a command…" : "Search files by name…  'exact  ^start  end$  !not  ext:pdf  src/";

  return (
    <div className="backdrop glass" onMouseDown={close}>
      <div className="qo" onMouseDown={(e) => e.stopPropagation()} tabIndex={-1} onKeyDown={onKey}>
        <div className="qo-input">
          <Icon name={cmdMode ? "wand" : "search"} size={17} />
          <input ref={inputRef} value={q} spellCheck={false} placeholder={placeholder} onChange={(e) => { setQ(e.target.value); setIdx(0); }} />
          {!cmdMode && (
            <button className={`chip ${scopeHere ? "on" : ""}`} title="Toggle scope (Tab)" onClick={() => { setScopeHere((v) => !v); inputRef.current?.focus(); }}>
              {scopeHere ? <>in {basename(here) || here}</> : <>everywhere</>}
              <kbd>Tab</kbd>
            </button>
          )}
        </div>
        <div className="qo-body">
          <div className="qo-list" ref={listRef}>
            {!cmdMode && !q.trim() && rows.length > 0 && <div className="qo-section">Recent</div>}
            {rows.map((r, i) =>
              r.kind === "cmd" ? (
                <div key={r.action} className={`qo-row cmd ${i === idx ? "on" : ""}`} onMouseMove={() => i !== idx && setIdx(i)} onClick={() => accept(r, "open")}>
                  <span className="qo-name"><Highlight text={r.label} positions={r.positions} /></span>
                  <span className="qo-keys">
                    {r.vim && <kbd className="vim">{r.vim}</kbd>}
                    {r.chord && <kbd>{r.chord}</kbd>}
                  </span>
                </div>
              ) : (
                <FileRow key={r.hit.path} hit={r.hit} on={i === idx} recent={r.recent}
                  onHover={() => i !== idx && setIdx(i)} onClick={(e) => accept(r, e.ctrlKey ? "reveal" : "open")} />
              ),
            )}
            {rows.length === 0 && (
              <div className="qo-empty">
                {cmdMode ? "No matching command" : !q.trim() ? "Start typing to search every file on this computer" : indexing && !status?.entries ? "Building the index…" : "No matches"}
              </div>
            )}
          </div>
          {curEntry && (
            <div className="qo-preview">
              <div className="qo-pv">{info ? <Preview entry={info.entry ?? curEntry} info={info} /> : <div className="pv-icon"><FileIcon entry={curEntry} size={56} /></div>}</div>
              <div className="qo-pv-path" title={curEntry.path}><bdi>{curEntry.path}</bdi></div>
            </div>
          )}
        </div>
        <div className="qo-foot">
          <span className="qo-status">
            {indexing && <span className="spinner" />}
            {cmdMode
              ? `${rows.length} commands`
              : status
                ? <>{status.entries.toLocaleString()} items indexed{indexing ? " · updating…" : ""}{stats ? <> · <b>{stats.total.toLocaleString()}</b> matches · {stats.ms} ms</> : null}</>
                : "Loading index…"}
          </span>
          <span className="qo-hints">
            <kbd>↵</kbd> open <kbd>Ctrl ↵</kbd> reveal <kbd>⇧ ↵</kbd> other side {!cmdMode && <><kbd>&gt;</kbd> commands</>}
          </span>
        </div>
      </div>
    </div>
  );
}

function FileRow({ hit, on, recent, onHover, onClick }: { hit: IndexHit; on: boolean; recent?: boolean; onHover: () => void; onClick: (e: React.MouseEvent) => void }) {
  const nameStart = hit.path.length - hit.name.length;
  const dir = hit.path.slice(0, nameStart);
  const home = getState().info?.home ?? "";
  const shownDir = tildify(dir, home);
  const shift = dir.length - shownDir.length;
  return (
    <div className={`qo-row ${on ? "on" : ""}`} onMouseMove={onHover} onClick={onClick}>
      <FileIcon entry={pseudoEntry(hit.path, hit.isDir)} />
      <span className="qo-name"><Highlight text={hit.name} positions={hit.positions} offset={nameStart} /></span>
      <span className="qo-dir"><bdi><Highlight text={shownDir} positions={hit.positions.map((p) => p - shift)} /></bdi></span>
      {recent && <Icon name="history" size={13} style={{ opacity: 0.5 }} />}
    </div>
  );
}
