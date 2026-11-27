// Quick Open (Ctrl+P): instant fuzzy search over the whole-system file index,
// with fzf-style query syntax, a live preview and a ">" command palette mode.
import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { tt } from "../i18n";
import { api, events } from "../api/ipc";
import type { FileEntry, HiddenCount, IndexHit, IndexStatus } from "../api/types";
import { fuzzyMatch } from "../core/fuzzy";
import { emptyHiddenText, footerHiddenText } from "../core/hiddenHint";
import { ACTION_LABELS, buildKeymap, keyboardProfile, primaryMod, type Action } from "../core/keymap";
import { basename, isInside, parent, tildify } from "../core/path";
import { listBindings, treeForSettings, showKeys } from "../core/vim";
import { archiveOf } from "../core/archive";
import { actionLabel, actionPaths, actionsForEntries, mergeZoxide, zoxideName } from "../core/actionsMenu";
import * as A from "../state/actions";
import { controller } from "../state/controllers";
import { run } from "../state/run";
import { getState, pushRecent, useStore } from "../state/store";
import { FileIcon, Icon } from "./Icon";
import { Preview, usePreviewInfo } from "./Preview";
import { platformLabel } from "../core/keymap";

/** Actions that only make sense as list motions, not as palette commands. */
const HIDDEN_COMMANDS = new Set<Action>(["cursorDown", "cursorUp", "cursorTop", "cursorBottom", "halfPageDown", "halfPageUp", "enter", "selectDown", "contextMenu", "commandPalette",
  "tab1", "tab2", "tab3", "tab4", "tab5", "tab6", "tab7", "tab8", "tab9"]);

type Row =
  | { kind: "file"; hit: IndexHit; recent?: boolean; zoxide?: boolean }
  | { kind: "custom"; id: string; label: string; run: () => void; positions: number[] }
  | { kind: "cmd"; action: Action; label: string; chord?: string; vim?: string; positions: number[] };

/** The characters of `text` as the fuzzy matcher counts them: composed (NFC) code points, so a
 *  decomposed name (macOS) and an emoji are one position each, not two UTF-16 units. */
const glyphs = (text: string): string[] => Array.from(text.normalize("NFC"));

/** Renders `text` with the characters at `positions` (indices into its composed characters, shifted
 *  by `offset`) emphasised. The text is shown composed, which looks the same as the original. */
export function Highlight({ text, positions, offset = 0 }: { text: string; positions: Iterable<number>; offset?: number }) {
  const chars = glyphs(text);
  const set = new Set<number>();
  for (const p of positions) if (p - offset >= 0 && p - offset < chars.length) set.add(p - offset);
  if (!set.size) return <>{chars.join("")}</>;
  const parts: { s: string; on: boolean }[] = [];
  for (let i = 0; i < chars.length; i++) {
    const on = set.has(i);
    const last = parts[parts.length - 1];
    if (last && last.on === on) last.s += chars[i];
    else parts.push({ s: chars[i], on });
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

export function QuickOpen({ initial }: { initial?: string }) {
  const settings = useStore((s) => s.settings)!;
  const recent = useStore((s) => s.recentFiles);
  const history = useStore((s) => s.history);
  const [q, setQ] = useState(initial ?? "");
  // Starts in the open folder and below (what one looks for is usually in the project at
  // hand); Tab switches to everywhere. Not inside an archive, which the index does not hold.
  const [scopeHere, setScopeHere] = useState(() => {
    const p = A.activePath(A.activeBrowser());
    return !!p && !archiveOf(p);
  });
  const [hits, setHits] = useState<IndexHit[]>([]);
  const [stats, setStats] = useState<{ total: number; ms: number; scanning: boolean } | null>(null);
  const [everything, setEverything] = useState(false);
  const [hiddenCount, setHiddenCount] = useState<HiddenCount | null>(null);
  // False in a folder the system index does not hold (a WSL distribution, a network
  // share): Quick Open then searches this folder, which the backend scans itself.
  const [covered, setCovered] = useState<boolean | null>(null);
  const [scan, setScan] = useState<{ scanning: boolean; entries: number } | null>(null);
  const [status, setStatus] = useState<IndexStatus | null>(null);
  const [idx, setIdx] = useState(0);
  const [revision, setRevision] = useState(0);
  const [existingRecent, setExistingRecent] = useState<Set<string>>(new Set());
  // Palette entries that depend on the system: actions.d actions for the current selection,
  // "Connect to server…" and "Open active terminal's folder" (only when a terminal is found).
  const [extraCmds, setExtraCmds] = useState<{ id: string; label: string; run: () => void }[]>([]);
  // Folders from `zoxide query -l`, read once when first needed; failures mean none.
  const [zox, setZox] = useState<string[] | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const b = A.activeBrowser();
  const here = A.activePath(b);
  const inArchive = !!archiveOf(here);
  const cmdMode = q.startsWith(">");
  const pathMode = !cmdMode && /^\s*(\/|~(?:[/\\]|$)|[A-Za-z]:[/\\])/.test(q);

  useEffect(() => {
    inputRef.current?.focus();
    const scope = () =>
      inArchive ? Promise.resolve(true) : api.indexCovers(here).then((c) => {
        setCovered(c.covered);
        setScan(c.covered ? null : { scanning: c.scanning, entries: c.entries });
        return c.covered;
      });
    scope().catch(() => {});
    api.indexStatus().then(setStatus).catch(() => {});
    api.indexRefresh(false).then((started) => { if (started) return api.indexStatus().then(setStatus); }).catch(() => {});
    const un = events.indexUpdated((s) => {
      setStatus(s);
      scope().catch(() => {});
      setRevision((v) => v + 1);
      runQuery.current(true);
    });
    const unFs = events.fsChanged(() => {
      setRevision((v) => v + 1);
      runQuery.current(true);
    });
    return () => {
      void un.then((f) => f());
      void unFs.then((f) => f());
    };
  }, []);

  useEffect(() => {
    let active = true;
    const paths = [...new Set([...recent, ...history])]
      .filter((p) => !scopeHere || isInside(p, here));
    api.findExisting(paths).then((paths) => {
      if (active) setExistingRecent(new Set(paths));
    }).catch(() => {});
    return () => { active = false; };
  }, [recent, history, scopeHere, here, revision]);

  // ───────── file query (debounced, stale responses dropped) ─────────
  // The highlighted file, so a refresh of the same query keeps it highlighted.
  const selectedPath = useRef<string | null>(null);
  const querySeq = useRef(0);
  const runQuery = useRef((_keep?: boolean) => {});
  /** `keep`: the index changed under the same query (it is refreshed when Quick Open
   *  opens): the highlighted file stays highlighted, so Enter opens what was chosen. */
  runQuery.current = (keep = false) => {
    const id = ++querySeq.current;
    if (cmdMode || !q.trim()) {
      setHits([]);
      setStats(null);
      return;
    }
    const req = everything
      ? api.indexQuery(q, scopeHere ? here : null, 200, true, !scopeHere && !inArchive ? here : undefined)
      : api.indexQuery(q, scopeHere ? here : null, 200);
    req
      .then((r) => {
        if (id !== querySeq.current) return;
        setHits(r.hits);
        setStats({ total: r.total, ms: r.elapsedMs, scanning: r.scanning });
        const at = keep && selectedPath.current ? r.hits.findIndex((h) => h.path === selectedPath.current) : -1;
        setIdx(at >= 0 ? at : 0);
      })
      .catch((e) => { if (id === querySeq.current) A.fail(e); });
  };
  useEffect(() => {
    const t = setTimeout(() => runQuery.current(), 25);
    return () => { clearTimeout(t); ++querySeq.current; };
  }, [q, scopeHere, here, everything]);

  // ───────── lazy background hidden-match count (when filters are active) ─────────
  const hiddenSeq = useRef(0);
  useEffect(() => {
    const id = ++hiddenSeq.current;
    setHiddenCount(null);
    if (cmdMode || inArchive || everything || !q.trim()) {
      api.indexHiddenCancel?.().catch(() => {});
      return;
    }
    const t = setTimeout(() => {
      api.indexHiddenCount?.(q, scopeHere ? here : null, scopeHere ? undefined : here)
        .then((res) => {
          if (id === hiddenSeq.current) setHiddenCount(res.count > 0 ? res : null);
        })
        .catch(() => {});
    }, 150);
    return () => {
      clearTimeout(t);
      ++hiddenSeq.current;
      api.indexHiddenCancel?.().catch(() => {});
    };
  }, [q, scopeHere, here, cmdMode, inArchive, everything, revision]);

  // ───────── zoxide folders (lazy, once) ─────────
  // The answer is kept in a ref: an effect cleanup (StrictMode, a cleared query) must not lose it.
  const zoxCache = useRef<string[] | null>(null);
  const wantZox = !cmdMode && !!q.trim();
  useEffect(() => {
    if (!wantZox) return;
    if (zoxCache.current) {
      setZox(zoxCache.current);
      return;
    }
    let live = true;
    const done = (d: unknown) => {
      zoxCache.current = Array.isArray(d) ? (d as string[]) : [];
      if (live) setZox(zoxCache.current);
    };
    Promise.resolve(api.zoxideDirs?.()).then(done).catch(() => done([]));
    return () => { live = false; };
  }, [wantZox]);

  // ───────── system palette entries (fetched when the palette is used) ─────────
  useEffect(() => {
    if (!cmdMode) return;
    let live = true;
    setExtraCmds([]); // entries of the previous folder or selection must not be run on the new one
    const cwd = Promise.resolve(api.omarchyTerminalCwd?.()).catch(() => null);
    const defs = Promise.resolve(api.actionsList?.()).catch(() => []);
    const sel = controller(b)?.selection() ?? [];
    const matching = actionsForEntries(sel, here);
    void Promise.all([cwd, defs, matching]).then(([dir, all, ok]) => {
      if (!live) return;
      const out: { id: string; label: string; run: () => void }[] = [];
      if (getState().info?.platform === "linux") out.push({ id: "connect-server", label: tt("Connect to server…"), run: () => void A.connectToServer() });
      if (dir) out.push({ id: "terminal-cwd", label: tt("Open active terminal's folder"), run: () => A.navigate(b, dir) });
      const okIds = new Set(ok.map((d) => d.id));
      const paths = actionPaths(sel, here);
      for (const d of all ?? []) {
        if (!okIds.has(d.id)) continue;
        out.push({ id: `action:${d.id}`, label: tt("Action: {value}", { value: actionLabel(d, paths.length) }), run: () => { api.actionRun(d.id, paths, here).catch(A.fail); } });
      }
      setExtraCmds(out);
    });
    return () => { live = false; };
  }, [cmdMode, b, here]);

  // ───────── command palette rows ─────────
  const commands = useMemo(() => {
    const km = buildKeymap(settings.shortcuts, keyboardProfile(settings));
    const chords = new Map<Action, string>();
    for (const [c, a] of km) if (!chords.has(a)) chords.set(a, c);
    const vim = new Map<Action, string>();
    for (const r of listBindings(treeForSettings(settings))) if (!vim.has(r.action)) vim.set(r.action, showKeys(r.keys));
    return (Object.keys(ACTION_LABELS) as Action[])
      .filter((a) => !HIDDEN_COMMANDS.has(a))
      .map((a) => ({ action: a, label: tt(ACTION_LABELS[a]), chord: chords.get(a), vim: vim.get(a) }));
  }, [settings]);

  const rows: Row[] = useMemo(() => {
    if (cmdMode) {
      const term = q.slice(1);
      const all: ({ label: string } & ({ action: Action; chord?: string; vim?: string } | { id: string; run: () => void }))[] = [...commands, ...extraCmds];
      return all
        .map((c) => ({ c, m: fuzzyMatch(term, c.label) }))
        .filter((x) => x.m)
        .sort((x, y) => y.m!.score - x.m!.score)
        .map(({ c, m }): Row => "action" in c
          ? { kind: "cmd", ...c, positions: m!.positions }
          : { kind: "custom", id: c.id, label: c.label, run: c.run, positions: m!.positions });
    }
    if (!q.trim()) {
      // Empty query: recently opened files, then recently visited folders.
      const seen = new Set<string>();
      const list: Row[] = [];
      for (const p of [...recent, ...history]) {
        if (!existingRecent.has(p) || seen.has(p) || (scopeHere && !isInside(p, here))) continue;
        seen.add(p);
        const isDir = !recent.includes(p);
        list.push({ kind: "file", recent: true, hit: { path: p, name: basename(p), isDir, score: 0, positions: [] } });
        if (list.length >= 30) break;
      }
      return list;
    }
    const list: Row[] = hits.map((hit) => ({ kind: "file" as const, hit }));
    if (zox?.length && !pathMode) {
      const matched = new Map<string, number[]>();
      const extra = mergeZoxide(zox, hits.map((h) => h.path), (p) => {
        const m = fuzzyMatch(q.trim(), p);
        if (m) matched.set(p, m.positions);
        return !!m;
      }, scopeHere ? here : null);
      for (const p of extra) list.push({ kind: "file", zoxide: true, hit: { path: p, name: zoxideName(p), isDir: true, score: 0, positions: matched.get(p) ?? [] } });
    }
    return list;
  }, [cmdMode, pathMode, q, commands, extraCmds, hits, zox, recent, history, scopeHere, here, existingRecent]);

  const activeIdx = Math.min(idx, rows.length - 1);
  const cur = rows[activeIdx];
  selectedPath.current = cur?.kind === "file" ? cur.hit.path : null;
  const curEntry = cur?.kind === "file" ? pseudoEntry(cur.hit.path, cur.hit.isDir) : null;
  const info = usePreviewInfo(curEntry);

  useEffect(() => {
    listRef.current?.querySelector(".qo-row.on")?.scrollIntoView({ block: "nearest" });
  }, [idx, rows.length]);

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
    if (row.kind === "custom") {
      close();
      setTimeout(row.run, 0);
      return;
    }
    const { path, isDir } = row.hit;
    if (!isDir) pushRecent(path);
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

  const toggleEverything = () => {
    if (cmdMode || inArchive) return;
    setEverything((prev) => !prev);
    setIdx(0);
  };

  const onKey = (e: React.KeyboardEvent) => {
    e.stopPropagation();
    if (e.nativeEvent.isComposing || e.keyCode === 229) return;
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
    if (e.key === "Tab" && pathMode && cur?.kind === "file") {
      e.preventDefault();
      const sep = cur.hit.path.includes("\\") ? "\\" : "/";
      const next = cur.hit.isDir && !cur.hit.path.endsWith(sep) ? cur.hit.path + sep : cur.hit.path;
      setQ(next);
      setIdx(0);
      return;
    }
    if (e.key === "Tab" && !cmdMode && !inArchive) {
      e.preventDefault();
      setScopeHere((v) => !v);
      return;
    }
    // Alt+I toggles searching everything (filters disabled) in-place in Quick Open.
    if (!cmdMode && !inArchive && e.altKey && !e.ctrlKey && !e.metaKey && e.key.toLowerCase() === "i") {
      e.preventDefault();
      toggleEverything();
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      accept(cur, primaryMod(e) || e.altKey ? "reveal" : e.shiftKey ? "other" : "open");
    }
  };

  const indexing = status?.building;
  // This folder is searched directly (it is not in the system index: WSL, a network share).
  const live = scopeHere && covered === false;
  const liveName = basename(here) || here;
  // The scope switch and the footer are narrow: a long folder name reads "this folder".
  const shortName = liveName.length <= 16 ? liveName : "this folder";
  const liveBusy = live && (scan?.scanning || stats?.scanning);
  const everythingBusy = everything && !!stats?.scanning;
  const showHiddenHint = !cmdMode && !inArchive && !everything && !!q.trim() && !!hiddenCount && hiddenCount.count > 0;
  const placeholder = cmdMode ? tt("Type a command…") : tt("Search files by name…  'exact  ^start  end$  !not  ext:pdf  src/");

  return (
    <div className="backdrop glass" onMouseDown={close}>
      <div className="qo" onMouseDown={(e) => e.stopPropagation()} tabIndex={-1} onKeyDown={onKey}>
        <div className="qo-input">
          <Icon name={cmdMode ? "wand" : "search"} size={17} />
          <input ref={inputRef} value={q} spellCheck={false} placeholder={placeholder} onChange={(e) => { setQ(e.target.value); setIdx(0); }} />
          {!cmdMode && !inArchive && (
            <button
              className={`tgl ${everything ? "on" : ""}`}
              aria-pressed={everything}
              title={tt("Also search ignored files: .gallignore, .gitignore, node_modules, cache folders (Alt+I)")}
              onClick={() => { toggleEverything(); inputRef.current?.focus(); }}
            >
              <Icon name="eye" size={15} />
            </button>
          )}
          {!cmdMode && (
            // Tab switches scopes when the current folder can be searched directly.
            <div className="qo-scope-wrap">
            <div className="qo-scope" role="group" title={inArchive ? tt("Search scope") : tt("Tab switches between this folder and everywhere")}>
              {!inArchive && <button aria-pressed={scopeHere} className={scopeHere ? "on" : ""} title={`${here} and its subfolders`} onClick={() => { setScopeHere(true); inputRef.current?.focus(); }}>
                <Icon name="folder" size={12} /> {shortName}
              </button>}
              <button aria-pressed={!scopeHere} className={!scopeHere ? "on" : ""} onClick={() => { setScopeHere(false); inputRef.current?.focus(); }}>
                <Icon name="drive" size={12} /> {tt("everywhere")}
              </button>
            </div>
            {!inArchive && <kbd className="scope-key" title={tt("Switch scope")}>{tt("Tab")}</kbd>}
            </div>
          )}
        </div>
        <div className="qo-body">
          <div className="qo-list-col">
            <div className="qo-list" ref={listRef}>
              {!cmdMode && !q.trim() && rows.length > 0 && <div className="qo-section">{tt("Recent")}</div>}
              {rows.map((r, i) =>
                r.kind === "custom" ? (
                  <div key={r.id} className={`qo-row cmd ${i === activeIdx ? "on" : ""}`} onMouseMove={() => i !== idx && setIdx(i)} onClick={() => accept(r, "open")}>
                    <span className="qo-name"><Highlight text={r.label} positions={r.positions} /></span>
                  </div>
                ) : r.kind === "cmd" ? (
                  <div key={r.action} className={`qo-row cmd ${i === activeIdx ? "on" : ""}`} onMouseMove={() => i !== idx && setIdx(i)} onClick={() => accept(r, "open")}>
                    <span className="qo-name"><Highlight text={r.label} positions={r.positions} /></span>
                    <span className="qo-keys">
                      {r.vim && <kbd className="vim">{r.vim}</kbd>}
                      {r.chord && <kbd>{r.chord}</kbd>}
                    </span>
                  </div>
                ) : (
                  <FileRow key={r.hit.path} hit={r.hit} on={i === activeIdx} recent={r.recent} zoxide={r.zoxide}
                    onHover={() => i !== idx && setIdx(i)} onClick={(e) => accept(r, primaryMod(e) ? "reveal" : "open")} />
                ),
              )}
              {scopeHere && !cmdMode && !pathMode && q.trim() && rows.length > 0 && (
                <div className="qo-row qo-wider" onClick={() => { setScopeHere(false); inputRef.current?.focus(); }}>
                  <Icon name="drive" size={14} />
                  <span className="qo-name">{tt("Search everywhere for “")}{q.trim()}”</span>
                  <span className="qo-keys"><kbd>{tt("Tab")}</kbd></span>
                </div>
              )}
              {rows.length === 0 && (
                <div className="qo-empty">
                  {cmdMode ? tt("No matching command")
                    : !q.trim() ? (live ? (liveBusy ? tt("Reading {liveName}… you can start typing", { liveName }) : tt("Start typing to search {liveName}", { liveName })) : scopeHere ? <>{tt("Start typing to search")} {basename(here) || here} {tt("and below")}<br /><small><kbd>{tt("Tab")}</kbd> {tt("searches every file on this computer")}</small></> : tt("Start typing to search every file on this computer"))
                    : everythingBusy && !hits.length ? (scopeHere ? tt("Searching {liveName} including hidden files…", { liveName }) : tt("Searching everywhere including hidden files…"))
                    : liveBusy && !hits.length ? tt("Reading {liveName}…", { liveName })
                    : !everything && indexing && !status?.entries ? tt("Building the index…")
                    : showHiddenHint ? (
                      <>
                        <div>{tt("No results.")} <b>{emptyHiddenText(hiddenCount!)}</b>.</div>
                        <button className="hidden-action" onClick={() => { toggleEverything(); inputRef.current?.focus(); }}>
                          {tt("Press")} <kbd>{platformLabel("Alt+I")}</kbd> {tt("to include hidden files")}
                        </button>
                        {scopeHere && <><br /><small><kbd>{tt("Tab")}</kbd> {tt("searches everywhere")}</small></>}
                      </>
                    ) : (
                      <>{tt("No matches")}{scopeHere ? <> {tt("in")} {basename(here) || here}</> : null}
                        {scopeHere && <><br /><small><kbd>{tt("Tab")}</kbd> {tt("searches everywhere")}</small></>}
                      </>
                    )}
                </div>
              )}
            </div>
            {rows.length > 0 && showHiddenHint && (
              <button className="hidden-bar" onClick={() => { toggleEverything(); inputRef.current?.focus(); }}>
                <Icon name="eye" size={13} />
                <span>{footerHiddenText(hiddenCount!)}, <kbd>{platformLabel("Alt+I")}</kbd> {tt("to include")}</span>
              </button>
            )}
            {!cmdMode && getState().info?.platform === "macos" && (status?.restrictedDirs ?? 0) > 0 && (
              <div className="fda-banner qo-fda" role="note">
                <span>{tt("Some ~/Library folders were skipped without macOS Full Disk Access")}</span>
                <button className="btn sm" onClick={() => { api.openFullDiskAccessSettings().catch(A.fail); }}>
                  {tt("Full Disk Access…")}
                </button>
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
            {(everything ? everythingBusy : live ? liveBusy : indexing) && <span className="spinner" />}
            {cmdMode
              ? tt("{count} commands", { count: rows.length })
              : everything
                ? <>{everythingBusy ? (scopeHere ? tt("Searching {liveName} (including hidden)…", { liveName }) : tt("Searching everywhere (including hidden)…")) : (scopeHere ? tt("Including hidden files in {liveName}", { liveName }) : tt("Including hidden files everywhere"))}{stats ? <> · <b>{stats.total.toLocaleString()}</b> {tt("matches ·")} {stats.ms} {tt("ms")}</> : null}</>
                : status
                  ? scopeHere && covered === false
                    ? <span title={tt("Folders in WSL, on network shares and outside Settings › Search › Indexed folders are not in the system index; Quick Open reads them when it opens there.")}>{liveBusy ? tt("Reading {liveName}…", { liveName }) : `${(scan?.entries ?? 0).toLocaleString()} items in ${liveName}`}{stats ? <> · <b>{stats.total.toLocaleString()}</b> {tt("matches ·")} {stats.ms} {tt("ms")}</> : null}</span>
                    : <>{status.entries.toLocaleString()} {tt("items indexed")}{indexing ? " · updating…" : ""}{stats ? <> · <b>{stats.total.toLocaleString()}</b> {tt("matches ·")} {stats.ms} {tt("ms")}</> : null}</>
                  : tt("Loading index…")}
          </span>
          <span className="qo-hints">
            {!cmdMode && !inArchive && <><kbd>{tt("Tab")}</kbd> {scopeHere ? tt("search everywhere") : tt("search {name}", { name: shortName })} </>}<kbd>↵</kbd> {tt("open")} <kbd>{platformLabel(tt("Ctrl ↵"))}</kbd> {tt("reveal")} <kbd>⇧ ↵</kbd> {tt("other side")} {!cmdMode && <><kbd>&gt;</kbd> {tt("commands")}</>}
          </span>
        </div>
      </div>
    </div>
  );
}

function FileRow({ hit, on, recent, zoxide, onHover, onClick }: { hit: IndexHit; on: boolean; recent?: boolean; zoxide?: boolean; onHover: () => void; onClick: (e: React.MouseEvent) => void }) {
  // Positions count composed characters of the path (see `Highlight`), whatever the name's spelling on disk.
  const pathChars = glyphs(hit.path);
  const nameStart = pathChars.length - glyphs(hit.name).length;
  const dir = pathChars.slice(0, nameStart).join("");
  const home = getState().info?.home ?? "";
  const shownDir = tildify(dir, home);
  const shift = nameStart - glyphs(shownDir).length;
  return (
    <div className={`qo-row ${on ? "on" : ""} ${hit.hidden ? "is-hidden" : ""}`} onMouseMove={onHover} onClick={onClick}>
      <FileIcon entry={pseudoEntry(hit.path, hit.isDir)} />
      <span className="qo-name"><Highlight text={hit.name} positions={hit.positions} offset={nameStart} /></span>
      {hit.hidden && <span className="hidden-badge">{tt("hidden")}</span>}
      {zoxide && <span className="hidden-badge" title={tt("Frequently used folder (zoxide)")}>{tt("zoxide")}</span>}
      <span className="qo-dir"><bdi><Highlight text={shownDir} positions={hit.positions.map((p) => p - shift)} /></bdi></span>
      {recent && <Icon name="history" size={13} style={{ opacity: 0.5 }} />}
    </div>
  );
}
