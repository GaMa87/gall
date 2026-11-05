// Compare view: side-by-side or unified line diff with word-level highlights,
// folded unchanged regions, an overview ruler and keyboard navigation between
// changes. Binary files get a byte-level verdict.
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { api } from "../api/ipc";
import type { DiffOptions, DiffResult, DiffSource, SyntaxHighlight } from "../api/types";
import { applyBlock, buildSplit, buildUnified, joinLines, moveTarget, offsets, rowAt, segments, wrappedLines, type Cell, type Spans, type SplitRow, type SyntaxRuns, type UnifiedRow } from "../core/diffRows";
import { refreshAll } from "../state/controllers";
import { primaryMod } from "../core/keymap";
import { formatSize } from "../core/format";
import { basename, join, parent, tildify } from "../core/path";
import * as A from "../state/actions";
import { useStore, type Overlay } from "../state/store";
import { FileIcon, Icon } from "./Icon";
import { HexDiff } from "./HexDiff";
import { ImageCompare, isImage } from "./ImageCompare";
import { isTable, TableDiff } from "./TableDiff";

const CONTEXT = 3;
const OPTS_KEY = "diff:opts";
const MODE_KEY = "diff:mode";
const WRAP_KEY = "diff:wrap";
const defaultOpts: DiffOptions = { ignoreWhitespace: false, ignoreCase: false, ignoreEol: true, minimal: false };

function loadOpts(): DiffOptions {
  try {
    const saved = JSON.parse(localStorage.getItem(OPTS_KEY) ?? "{}") as Record<string, unknown>;
    // Only known keys (older versions stored "patience").
    const o = { ...defaultOpts };
    for (const k of Object.keys(defaultOpts) as (keyof DiffOptions)[]) if (typeof saved[k] === "boolean") o[k] = saved[k] as boolean;
    return o;
  } catch {
    return defaultOpts;
  }
}

const srcName = (s: DiffSource) => (s.kind === "text" ? s.label : basename(s.path));
const srcPath = (s: DiffSource) => (s.kind === "text" ? "" : s.path);

function Text({ text, spans, sx, kind }: { text: string; spans?: Spans; sx?: SyntaxRuns; kind: "del" | "ins" | "chg" | "eq" }) {
  return (
    <>
      {segments(text, spans, sx).map(([t, hi, cat], i) => {
        const inner = cat ? <span className={`sx${cat}`}>{t}</span> : t;
        return hi ? <mark key={i} className={`dv-word ${kind}`}>{inner}</mark> : <span key={i}>{inner}</span>;
      })}
      {text === "" && "​"}
    </>
  );
}

export function DiffView({ a: a0, b: b0, back }: { a: DiffSource; b: DiffSource; back?: Overlay }) {
  const fontSize = useStore((s) => s.settings?.fontSize ?? 13);
  const home = useStore((s) => s.info?.home);
  const [src, setSrc] = useState<[DiffSource, DiffSource]>([a0, b0]);
  const [opts, setOpts] = useState<DiffOptions>(loadOpts);
  const [mode, setMode] = useState<"split" | "unified">(() => (localStorage.getItem(MODE_KEY) === "unified" ? "unified" : "split"));
  const [fold, setFold] = useState(true);
  // Wrap long lines (default): no sideways scrolling when one line is very long.
  const [wrap, setWrap] = useState(() => localStorage.getItem(WRAP_KEY) !== "0");
  const [bodyW, setBodyW] = useState(1000);
  const [charW, setCharW] = useState(8);
  const probeRef = useRef<HTMLSpanElement>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [res, setRes] = useState<DiffResult | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [cur, setCur] = useState(-1);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewH, setViewH] = useState(600);
  const bodyRef = useRef<HTMLDivElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const rowH = Math.round(fontSize * 1.55);
  // Merge edits held in memory until saved (null = side unchanged from its source).
  const [edits, setEdits] = useState<[string[] | null, string[] | null]>([null, null]);
  const [history, setHistory] = useState<[string[] | null, string[] | null][]>([]);
  const meta = useRef<[{ crlf: boolean; finalNewline: boolean }, { crlf: boolean; finalNewline: boolean }]>([
    { crlf: false, finalNewline: true }, { crlf: false, finalNewline: true }]);
  const keepView = useRef(false);
  // CSV / spreadsheets get the table view (CSV can switch to plain text).
  const [tableView, setTableView] = useState(true);
  const bothTables = src[0].kind === "file" && src[1].kind === "file" && isTable(src[0].path) && isTable(src[1].path);
  const tableMode = bothTables && tableView;
  const csvOnly = bothTables && ![src[0], src[1]].some((x) => x.kind === "file" && /\.(xlsx|xlsm|xls|xlsb|ods)$/i.test(x.path));
  const dirty = !!(edits[0] || edits[1]);
  const editable = (i: 0 | 1) => src[i].kind === "file";
  const eff = useCallback((i: 0 | 1): DiffSource => {
    const e = edits[i];
    return e ? { kind: "text", label: srcName(src[i]), text: joinLines(e, meta.current[i].crlf, meta.current[i].finalNewline) } : src[i];
  }, [edits, src]);

  // Only the latest comparison's reply counts: options toggled quickly (w, i, e…) or merges
  // in a row start several, and an older reply arriving last showed the wrong diff.
  const runSeq = useRef(0);
  const run = useCallback(() => {
    setLoading(true);
    setErr(null);
    const keep = keepView.current;
    keepView.current = false;
    const my = ++runSeq.current;
    api.diffCompare(eff(0), eff(1), opts)
      .then((r) => {
        if (my !== runSeq.current) return;
        // Remember each file's line endings while it is unedited, to save it the same way.
        if (!edits[0]) meta.current[0] = { crlf: r.left.crlf, finalNewline: r.left.finalNewline };
        if (!edits[1]) meta.current[1] = { crlf: r.right.crlf, finalNewline: r.right.finalNewline };
        setRes(r);
        if (!keep) { setCur(-1); setExpanded(new Set()); }
      })
      .catch((e) => { if (my === runSeq.current) setErr(String(e)); })
      .finally(() => { if (my === runSeq.current) setLoading(false); });
  }, [eff, edits, opts]);
  useEffect(run, [run]);
  useEffect(() => { localStorage.setItem(OPTS_KEY, JSON.stringify(opts)); }, [opts]);
  useEffect(() => { localStorage.setItem(MODE_KEY, mode); }, [mode]);
  useEffect(() => { localStorage.setItem(WRAP_KEY, wrap ? "1" : "0"); }, [wrap]);
  useEffect(() => { rootRef.current?.focus(); }, []);

  const tx = res?.text ?? null;

  // ── syntax colours: fetched in the background after the diff is shown ──
  const [hl, setHl] = useState<[SyntaxHighlight | null, SyntaxHighlight | null]>([null, null]);
  const hlFor = useRef<[string, string[]][]>([["", []], ["", []]]);
  const hlReq = useRef([0, 0]);
  useEffect(() => {
    if (!tx) return;
    // Clipboard text has no name: colour it as the file on the other side.
    const nameOf = (i: 0 | 1) => (src[i].kind === "text" && src[1 - i].kind !== "text" ? srcName(src[1 - i]) : srcName(src[i]));
    const t = setTimeout(() => {
      ([0, 1] as const).forEach((i) => {
        const lines = i === 0 ? tx.leftLines : tx.rightLines;
        const name = nameOf(i);
        const [pn, pl] = hlFor.current[i];
        if (pn === name && pl.length === lines.length && pl.every((l, k) => l === lines[k])) return;
        hlFor.current[i] = [name, lines];
        const id = ++hlReq.current[i];
        if (pn !== name) setHl((h) => (i === 0 ? [null, h[1]] : [h[0], null]));
        api.diffHighlight(name, lines)
          .then((r) => { if (hlReq.current[i] === id) setHl((h) => (i === 0 ? [r, h[1]] : [h[0], r])); })
          .catch(() => {});
      });
    }, 100);
    return () => clearTimeout(t);
  }, [tx, src]);
  const built = useMemo(() => {
    if (!tx) return null;
    const o = { context: fold ? CONTEXT : Infinity, expanded, moves: tx.moves };
    return mode === "split" ? buildSplit(tx.ops, tx.pairs, o) : buildUnified(tx.ops, tx.pairs, o);
  }, [tx, mode, fold, expanded]);
  const rows = built?.rows ?? [];
  const blocks = built?.blocks ?? [];
  const blockStart = useMemo(() => new Map(blocks.map((r, k) => [r, k])), [blocks]);
  const blockOfRow = useMemo(() => {
    const m = new Map<number, number>();
    rows.forEach((r, i) => { if (r.kind === "line" && r.block != null) m.set(i, r.block); });
    return m;
  }, [rows]);

  useLayoutEffect(() => {
    const el = bodyRef.current;
    if (!el) return;
    const measure = () => {
      setViewH(el.clientHeight);
      setBodyW(el.clientWidth);
      const p = probeRef.current;
      if (p && p.offsetWidth) setCharW(p.getBoundingClientRect().width / 100);
    };
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  }, [res, fontSize]);

  const goBlock = (i: number) => {
    if (!blocks.length) return;
    const k = (i + blocks.length) % blocks.length;
    setCur(k);
    const el = bodyRef.current;
    if (el) el.scrollTop = Math.max(0, off[blocks[k]] - el.clientHeight / 3);
  };
  const next = () => goBlock(cur < 0 ? blocks.findIndex((r) => off[r] > scrollTop + 4) : cur + 1);
  const prev = () => goBlock(cur < 0 ? blocks.length - 1 : cur - 1);

  // ── merge ──
  const applyBlock_ = (dir: "toRight" | "toLeft", k = cur) => {
    // While the last merge is being compared again, `tx` still holds the lines from before
    // it: a block applied now would be cut from those and silently undo that merge.
    if (!tx || loading) return;
    const target = dir === "toRight" ? 1 : 0;
    if (!editable(target)) return A.notify(`The ${target ? "right" : "left"} side is read-only (${src[target].kind === "gitHead" ? "Git HEAD" : "clipboard"})`, "warn");
    const op = tx.ops.filter((o) => o.tag !== "equal")[k < 0 ? 0 : k];
    if (!op) return;
    const r = applyBlock(tx.leftLines, tx.rightLines, op, dir);
    setHistory((h) => [...h, edits]);
    keepView.current = true;
    setEdits([dir === "toLeft" ? r.left : edits[0], dir === "toRight" ? r.right : edits[1]]);
  };
  const undoEdit = () => {
    if (!history.length) return;
    keepView.current = true;
    setEdits(history[history.length - 1]);
    setHistory((h) => h.slice(0, -1));
  };
  const saveEdits = async () => {
    if (!dirty) return;
    try {
      let undoable = true;
      for (const i of [0, 1] as const) {
        const e = edits[i], s = src[i];
        if (e && s.kind === "file") undoable = (await api.saveText(s.path, joinLines(e, meta.current[i].crlf, meta.current[i].finalNewline))) && undoable;
      }
      if (undoable) A.notify("Saved. The previous version is in the trash; Undo restores it.", "success", { label: "Undo", run: () => A.undo() });
      else A.notify("Saved. The trash isn't available here, so the previous version can't be restored.", "warn");
      setHistory([]);
      keepView.current = true;
      setEdits([null, null]);
      A.refreshUndo();
      refreshAll();
    } catch (e) { A.fail(e); }
  };

  const copyPatch = async () => {
    try {
      await api.clipSetText(await api.diffPatch(eff(0), eff(1)));
      A.notify("Patch copied (unified diff)", "success");
    } catch (e) { A.fail(e); }
  };
  const savePatch = async () => {
    const { save } = await import("@tauri-apps/plugin-dialog");
    const dir = srcPath(src[1]) ? parent(srcPath(src[1])) : null;
    const file = `${srcName(src[0])}.patch`;
    const dest = await save({ defaultPath: dir ? join(dir, file) : file, filters: [{ name: "Patch", extensions: ["patch", "diff"] }] });
    if (!dest) return;
    api.diffSavePatch(src[0], src[1], dest).then(() => A.notify(`Saved ${basename(dest)}`, "success")).catch(A.fail);
  };
  const toggle = (k: keyof DiffOptions) => setOpts((o) => ({ ...o, [k]: !o[k] }));
  const swap = () => {
    setSrc(([x, y]) => [y, x]);
    setEdits(([x, y]) => [y, x]);
    setHistory((h) => h.map(([x, y]) => [y, x]));
    meta.current = [meta.current[1], meta.current[0]];
  };
  // Opened from the folder comparison: Esc goes back there.
  const close = async () => {
    if (dirty && !(await A.confirm("Discard unsaved changes?", "Blocks you copied between the files have not been saved.", "Discard"))) return;
    A.openOverlay(back ?? null);
  };

  const onKey = (e: React.KeyboardEvent) => {
    e.stopPropagation();
    if ((e.target as HTMLElement).tagName === "INPUT") return;
    const el = bodyRef.current;
    const k = e.key;
    const by = (n: number) => { if (el) el.scrollTop += n; };
    if (primaryMod(e) && k === "s") saveEdits();
    else if (primaryMod(e) && k === "z") undoEdit();
    // Other Cmd chords are the system's (Cmd+Q, Cmd+C…): not the letter commands below
    // (Cmd+S swapped the sides, Cmd+Q closed the view instead of quitting).
    else if (e.metaKey) return;
    else if (e.altKey && k === "ArrowRight") applyBlock_("toRight");
    else if (e.altKey && k === "ArrowLeft") applyBlock_("toLeft");
    else if (k === "Escape" || k === "q") close();
    else if (k === "n" || k === "F8" || (k === "]" && !e.ctrlKey) || (e.altKey && k === "ArrowDown")) next();
    else if (k === "N" || (k === "F8" && e.shiftKey) || k === "[" || (e.altKey && k === "ArrowUp")) prev();
    else if (k === "j" || k === "ArrowDown") by(rowH);
    else if (k === "k" || k === "ArrowUp") by(-rowH);
    else if (k === "PageDown" || k === " " || (e.ctrlKey && k === "d")) by((el?.clientHeight ?? 400) * (k === "d" ? 0.5 : 0.9));
    else if (k === "PageUp" || (e.ctrlKey && k === "u")) by(-(el?.clientHeight ?? 400) * (k === "u" ? 0.5 : 0.9));
    else if (k === "g" || k === "Home") { if (el) el.scrollTop = 0; }
    else if (k === "G" || k === "End") { if (el) el.scrollTop = el.scrollHeight; }
    else if (k === "u") setMode((m) => (m === "split" ? "unified" : "split"));
    else if (k === "w") toggle("ignoreWhitespace");
    else if (k === "i") toggle("ignoreCase");
    else if (k === "e") toggle("ignoreEol");
    else if (k === "m") toggle("minimal");
    else if (k === "z") setFold((f) => !f);
    else if (k === "l") setWrap((v) => !v);
    else if (k === "s") swap();
    else if (k === "r") run();
    else if (k === "y") copyPatch();
    else return;
    e.preventDefault();
  };

  // ── row rendering ──
  const leftLines = tx?.leftLines ?? [];
  const rightLines = tx?.rightLines ?? [];
  const maxLen = useMemo(() => Math.max(1, ...leftLines.map((l) => l.length), ...rightLines.map((l) => l.length)), [leftLines, rightLines]);
  const digits = String(Math.max(leftLines.length, rightLines.length)).length + 1;
  const numW = `${digits}ch`;
  // Columns available for code when wrapping: the cell minus the line number
  // (0.9em font), the sign (1.6ch), padding and the divider.
  const cols = useMemo(() => {
    const numPx = digits * 0.9 * charW;
    const avail = mode === "split" ? bodyW / 2 - numPx - 1.6 * charW - 13 : bodyW - 2 * numPx - 1.6 * charW - 12;
    return Math.max(10, Math.floor(avail / charW));
  }, [digits, charW, bodyW, mode]);
  const off = useMemo(() => {
    if (!wrap) return offsets(rows.map(() => rowH));
    return offsets(rows.map((r) => {
      if (r.kind === "fold") return rowH;
      if (mode === "split") {
        const s = r as Extract<SplitRow, { kind: "line" }>;
        const n = Math.max(s.l ? wrappedLines(leftLines[s.l.n] ?? "", cols) : 1, s.r ? wrappedLines(rightLines[s.r.n] ?? "", cols) : 1);
        return n * rowH;
      }
      const u = r as Extract<UnifiedRow, { kind: "line" }>;
      const line = u.side === "a" ? leftLines[u.an!] : rightLines[u.bn!];
      return wrappedLines(line ?? "", cols) * rowH;
    }));
  }, [rows, wrap, rowH, cols, mode, leftLines, rightLines]);
  const first = Math.max(0, rowAt(off, scrollTop) - 20);
  const last = Math.min(rows.length, rowAt(off, scrollTop + viewH) + 21);
  const rowStyle = (i: number) => ({ top: off[i], height: off[i + 1] - off[i] });

  // Scroll to a line of one side (the other end of a move).
  const jumpTo = (side: "l" | "r", n: number) => {
    const i = rows.findIndex((r) => r.kind === "line" && (mode === "split"
      ? (side === "l" ? (r as Extract<SplitRow, { kind: "line" }>).l?.n === n : (r as Extract<SplitRow, { kind: "line" }>).r?.n === n)
      : (side === "l" ? (r as Extract<UnifiedRow, { kind: "line" }>).side === "a" && (r as Extract<UnifiedRow, { kind: "line" }>).an === n
        : (r as Extract<UnifiedRow, { kind: "line" }>).side === "b" && (r as Extract<UnifiedRow, { kind: "line" }>).bn === n)));
    const el = bodyRef.current;
    if (i >= 0 && el) el.scrollTop = Math.max(0, off[i] - el.clientHeight / 3);
  };
  /** The sign of a moved line: ⇅, click goes to where it moved to / came from. */
  const moveSign = (mv: number, side: "l" | "r", n: number) => {
    const to = moveTarget(tx!.moves, mv, side, n);
    const tip = side === "l" ? `Moved to line ${to + 1} (click to go there)` : `Moved from line ${to + 1} (click to go there)`;
    return <span className="dv-sign mv" title={tip} onClick={(e) => { e.stopPropagation(); jumpTo(side === "l" ? "r" : "l", to); }}>⇅</span>;
  };

  const cell = (c: Cell | undefined, lines: string[], side: "l" | "r") => {
    const sx = c ? hl[side === "l" ? 0 : 1]?.lines[c.n] : undefined;
    if (!c) return <div className={`dv-cell empty ${side}`}><span className="dv-num" style={{ width: numW }} /><span className="dv-code" /></div>;
    return (
      <div className={`dv-cell ${c.t} ${side} ${c.mv != null ? "mv" : ""}`}>
        <span className="dv-num" style={{ width: numW }}>{c.n + 1}</span>
        {c.mv != null ? moveSign(c.mv, side, c.n) : <span className="dv-sign">{c.t === "del" || (c.t === "chg" && side === "l") ? "−" : c.t === "ins" || (c.t === "chg" && side === "r") ? "+" : ""}</span>}
        <span className="dv-code"><Text text={lines[c.n] ?? ""} spans={c.spans} sx={sx} kind={c.t} /></span>
      </div>
    );
  };
  const foldRow = (r: Extract<SplitRow | UnifiedRow, { kind: "fold" }>, i: number) => (
    <button key={i} className="dv-fold" style={rowStyle(i)} onClick={() => setExpanded((s) => new Set(s).add(r.id))}>
      <Icon name="more" size={14} /> {r.count} unchanged line{r.count === 1 ? "" : "s"} · click to show
    </button>
  );

  const renderRow = (r: SplitRow | UnifiedRow, i: number) => {
    if (r.kind === "fold") return foldRow(r, i);
    const isCur = cur >= 0 && blockOfRow.get(i) === cur;
    const style = { ...rowStyle(i), lineHeight: `${rowH}px` };
    if (mode === "split") {
      const s = r as Extract<SplitRow, { kind: "line" }>;
      const k = blockStart.get(i);
      return (
        <div key={i} className={`dv-row split ${isCur ? "cur" : ""}`} style={style}>
          {cell(s.l, leftLines, "l")}{cell(s.r, rightLines, "r")}
          {k != null && (editable(0) || editable(1)) && (
            <span className="dv-merge">
              {editable(0) && <button title="Copy this change to the left (Alt+←)" onClick={() => { setCur(k); applyBlock_("toLeft", k); }}>‹</button>}
              {editable(1) && <button title="Copy this change to the right (Alt+→)" onClick={() => { setCur(k); applyBlock_("toRight", k); }}>›</button>}
            </span>
          )}
        </div>
      );
    }
    const u = r as Extract<UnifiedRow, { kind: "line" }>;
    const lines = u.side === "a" ? leftLines : rightLines;
    const n = u.side === "a" ? u.an! : u.bn!;
    return (
      <div key={i} className={`dv-row unified ${u.t} ${u.mv != null ? "mv" : ""} ${isCur ? "cur" : ""}`} style={style}>
        <span className="dv-num" style={{ width: numW }}>{u.an != null ? u.an + 1 : ""}</span>
        <span className="dv-num" style={{ width: numW }}>{u.bn != null ? u.bn + 1 : ""}</span>
        {u.mv != null ? moveSign(u.mv, u.side === "a" ? "l" : "r", n) : <span className="dv-sign">{u.t === "del" ? "−" : u.t === "ins" ? "+" : ""}</span>}
        <span className="dv-code"><Text text={lines[n] ?? ""} spans={u.spans} sx={hl[u.side === "a" ? 0 : 1]?.lines[n]} kind={u.t} /></span>
      </div>
    );
  };

  const changes = blocks.length;
  const identical = !!tx && tx.added === 0 && tx.removed === 0;
  const anyIgnore = opts.ignoreWhitespace || opts.ignoreCase;
  const total = off[rows.length] ?? 0;

  const fileHead = (s: DiffSource, side: "l" | "r") => (
    <div className={`dv-file ${side}`} title={srcPath(s) || srcName(s)}>
      {s.kind === "text" ? <Icon name="paste" size={15} /> : <FileIcon entry={{ isDir: false, ext: srcName(s).split(".").pop() ?? "", name: srcName(s) }} size={15} />}
      <b>{srcName(s)}</b>
      {edits[side === "l" ? 0 : 1] && <span className="dv-dirty" title="Changed here, not saved yet">● modified</span>}
      {s.kind === "gitHead" && <span className="dv-tag">HEAD</span>}
      {/* LRM marks keep the leading "/" in place: the path is right-to-left only so it ellipsizes from the left. */}
      <span className="dv-path">{srcPath(s) && "\u200e" + tildify(parent(srcPath(s)) ?? "", home) + "\u200e"}</span>
    </div>
  );

  return (
    <div className="backdrop glass center" onMouseDown={close}>
      <div className="diffv" ref={rootRef} tabIndex={-1} onKeyDown={onKey} onMouseDown={(e) => e.stopPropagation()}>
        <header className="dv-head">
          <div className="dv-files">
            {fileHead(src[0], "l")}
            <button className="icon-btn dv-swap" title="Swap sides (s)" onClick={swap}><Icon name="compare" size={15} /></button>
            {fileHead(src[1], "r")}
            <button className="icon-btn" title="Close (Esc)" onClick={close}><Icon name="close" size={15} /></button>
          </div>
          <div className="dv-bar">
            {tx && !tableMode && (
              <span className="dv-stat" title={`${(tx.ratio * 100).toFixed(1)} % similar`}>
                <b className="add">+{tx.added}</b> <b className="del">−{tx.removed}</b>
                <span className="dv-ratio"><i style={{ width: `${tx.ratio * 100}%` }} /></span>
                <span>{Math.round(tx.ratio * 100)}% similar</span>
                {tx.moves.length > 0 && (
                  <span className="dv-moved" title="Lines removed in one place and added unchanged in another (indentation aside)">
                    ⇅ {tx.moves.reduce((n, m) => n + m.len, 0)} moved
                  </span>
                )}
              </span>
            )}
            {/* Text-only controls: hidden for binary files, images and tables. */}
            {(tx || !res) && !tableMode && (
              <>
            <div className="seg" role="tablist">
              <button className={mode === "split" ? "on" : ""} title="Side by side (u)" onClick={() => setMode("split")}>Side by side</button>
              <button className={mode === "unified" ? "on" : ""} title="Unified, like git diff (u)" onClick={() => setMode("unified")}>Unified</button>
            </div>
            <div className="dv-toggles">
              <button className={`tgl ${opts.ignoreWhitespace ? "on" : ""}`} title="Ignore whitespace (w)" onClick={() => toggle("ignoreWhitespace")}>␣</button>
              <button className={`tgl ${opts.ignoreCase ? "on" : ""}`} title="Ignore case (i)" onClick={() => toggle("ignoreCase")}>Aa</button>
              <button className={`tgl ${opts.ignoreEol ? "on" : ""}`} title="Ignore line endings CRLF/LF (e)" onClick={() => toggle("ignoreEol")}>⏎</button>
              <button className={`tgl ${opts.minimal ? "on" : ""}`} title="Minimal diff: fewest changed lines, often harder to read (m)" onClick={() => toggle("minimal")}>M</button>
              <button className={`tgl ${fold ? "on" : ""}`} title="Fold unchanged lines (z)" onClick={() => setFold((f) => !f)}>⋯</button>
              <button className={`tgl ${wrap ? "on" : ""}`} title="Wrap long lines (l)" onClick={() => setWrap((v) => !v)}>↩</button>
            </div>
              </>
            )}
            <span className="spacer" />
            {tx && changes > 0 && !tableMode && (
              <div className="dv-nav">
                <button className="icon-btn" title="Previous change (N, [, Shift+F8)" onClick={prev}><Icon name="up" size={15} /></button>
                <span className="dv-count">{cur >= 0 ? cur + 1 : "–"} / {changes}</span>
                <button className="icon-btn" title="Next change (n, ], F8)" onClick={next}><Icon name="down" size={15} /></button>
              </div>
            )}
            {bothTables && !tableView && <button className="dc-btn" title="Compare as a table (columns and rows matched)" onClick={() => setTableView(true)}>Table view</button>}
            {dirty && <button className="dc-btn" title="Undo the last copied block (Ctrl+Z)" onClick={undoEdit} disabled={!history.length}>Undo</button>}
            {dirty && <button className="dc-btn primary" title="Save the changed file(s) (Ctrl+S)" onClick={saveEdits}>Save</button>}
            {tx && !tableMode && <button className="icon-btn" title="Copy unified patch (y)" onClick={copyPatch}><Icon name="copy" size={15} /></button>}
            {tx && !tableMode && <button className="icon-btn" title="Save patch…" onClick={savePatch}><Icon name="download" size={15} /></button>}
            <button className="icon-btn" title="Reload (r)" onClick={run}><Icon name="refresh" size={15} /></button>
          </div>
        </header>

        {err && <div className="dv-msg err"><Icon name="close" size={16} /> {err}</div>}
        {loading && !res && <div className="dv-msg">Comparing…</div>}

        {tableMode && src[0].kind === "file" && src[1].kind === "file" && (
          <TableDiff left={src[0].path} right={src[1].path} onText={csvOnly ? () => setTableView(false) : undefined} />
        )}

        {!tableMode && res?.binary && src[0].kind === "file" && src[1].kind === "file" && isImage(src[0].path) && isImage(src[1].path) && (
          <>
            <div className={`dv-msg ${res.binary.identical ? "same" : ""}`}>
              <Icon name={res.binary.identical ? "check" : "compare"} size={16} />
              {res.binary.identical ? "Files are identical, byte for byte" : `Files differ (${formatSize(res.left.size)} vs ${formatSize(res.right.size)})`}
            </div>
            <ImageCompare left={src[0].path} right={src[1].path} />
          </>
        )}

        {/* Two binary files: the bytes side by side, differences marked (D-04). */}
        {!tableMode && res?.binary && src[0].kind === "file" && src[1].kind === "file" && !(isImage(src[0].path) && isImage(src[1].path)) && (
          <HexDiff left={src[0].path} right={src[1].path} leftLabel={res.left.label} rightLabel={res.right.label} />
        )}

        {!tableMode && res?.binary && !(src[0].kind === "file" && src[1].kind === "file") && (
          <div className="dv-binary">
            <div className={`dv-verdict ${res.binary.identical ? "same" : "diff"}`}>
              <Icon name={res.binary.identical ? "check" : "compare"} size={28} />
              <div>
                <b>{res.binary.identical ? "Files are identical" : "Files differ"}</b>
                <span>{res.binary.identical ? `Byte for byte (${res.binary.reason})` : res.binary.firstDifference != null ? `First difference at byte ${res.binary.firstDifference.toLocaleString()} (0x${res.binary.firstDifference.toString(16).toUpperCase()}) · ${res.binary.reason}` : res.binary.reason}</span>
              </div>
            </div>
            <div className="dv-sizes">
              <div><span>{res.left.label}</span><b>{formatSize(res.left.size)}</b></div>
              <div><span>{res.right.label}</span><b>{formatSize(res.right.size)}</b></div>
            </div>
          </div>
        )}

        {!tableMode && tx && identical && (
          <div className="dv-msg same"><Icon name="check" size={16} /> No differences{anyIgnore ? " (ignoring " + [opts.ignoreWhitespace && "whitespace", opts.ignoreCase && "case"].filter(Boolean).join(" and ") + ")" : ""}
            {res && res.left.crlf !== res.right.crlf && " · line endings differ (CRLF vs LF)"}
            {res && res.left.finalNewline !== res.right.finalNewline && " · final newline differs"}
          </div>
        )}

        {!tableMode && tx && (
          <div className="dv-main">
            {mode === "split" && (
              <div className="dv-colheads">
                <span>{res!.left.label}{res!.left.crlf ? " · CRLF" : ""}{hl[0] ? ` · ${hl[0].language}` : ""}</span>
                <span>{res!.right.label}{res!.right.crlf ? " · CRLF" : ""}{hl[1] ? ` · ${hl[1].language}` : ""}</span>
              </div>
            )}
            <div className="dv-scroll">
              <div className="dv-body" ref={bodyRef} onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
                style={{ ["--dv-chars" as string]: `${maxLen + 2}ch` }}>
                <span className="dv-probe" aria-hidden><span ref={probeRef}>{"0".repeat(100)}</span></span>
                <div className={`dv-canvas ${mode} ${wrap ? "wrap" : ""}`} style={{ height: total }}>
                  {rows.slice(first, last).map((r, k) => renderRow(r, first + k))}
                </div>
              </div>
              <div className="dv-ruler" title="Changes overview: click to jump"
                onMouseDown={(e) => {
                  const rr = e.currentTarget.getBoundingClientRect();
                  const el = bodyRef.current;
                  if (el) el.scrollTop = ((e.clientY - rr.top) / rr.height) * total - el.clientHeight / 2;
                }}>
                {rows.length > 0 && blocks.map((ri, k) => {
                  const r = rows[ri];
                  let n = 1;
                  while (ri + n < rows.length && blockOfRow.get(ri + n) === k) n++;
                  const t = r.kind === "line" ? ("t" in r ? r.t : r.l && r.r ? "chg" : r.l ? "del" : "ins") : "chg";
                  return <i key={k} className={`m ${t} ${k === cur ? "cur" : ""}`} style={{ top: `${(off[ri] / Math.max(total, 1)) * 100}%`, height: `max(3px, ${((off[ri + n] - off[ri]) / Math.max(total, 1)) * 100}%)` }} />;
                })}
                <b className="view" style={{ top: `${(scrollTop / Math.max(total, 1)) * 100}%`, height: `${Math.min(100, (viewH / Math.max(total, 1)) * 100)}%` }} />
              </div>
            </div>
          </div>
        )}
        {/* Two binary files (hex view): only its own keys. */}
        {!tableMode && res?.binary && src[0].kind === "file" && src[1].kind === "file" && !(isImage(src[0].path) && isImage(src[1].path)) ? (
          <footer className="dv-foot">
            <span><kbd>n</kbd>/<kbd>N</kbd> or <kbd>F8</kbd> next/prev difference</span>
            <span><kbd>j</kbd>/<kbd>k</kbd> scroll</span>
            <span><kbd>g</kbd>/<kbd>G</kbd> start/end</span>
            <span><kbd>Esc</kbd> close</span>
          </footer>
        ) : (
        <footer className="dv-foot">
          <span><kbd>n</kbd>/<kbd>N</kbd> next/prev change</span>
          <span><kbd>u</kbd> view</span>
          <span><kbd>w</kbd> whitespace</span>
          <span><kbd>z</kbd> fold</span>
          <span><kbd>l</kbd> wrap</span>
          <span><kbd>s</kbd> swap</span>
          {(editable(0) || editable(1)) && <span><kbd>Alt</kbd>+<kbd>←</kbd>/<kbd>→</kbd> copy change · <kbd>Ctrl</kbd>+<kbd>S</kbd> save</span>}
          <span><kbd>y</kbd> copy patch</span>
          <span><kbd>Esc</kbd> close</span>
        </footer>
        )}
      </div>
    </div>
  );
}
