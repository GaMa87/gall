// Hex comparison of two binary files (D-04): offset, 16 bytes as hex and as text for each
// side, differing bytes marked; next / previous difference; an overview ruler. The backend
// streams both files once for the differing ranges; the bytes shown are read in blocks as
// they scroll into view, so files of any size open at once.
import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api/ipc";
import type { HexDiffResult } from "../api/types";
import { formatSize } from "../core/format";
import { useStore } from "../state/store";
import { Icon } from "./Icon";

const PER_ROW = 16;
const BLOCK = 16 * 1024;

/** First range whose end is past `offset` (ranges ascending). */
function rangeAt(ranges: [number, number][], offset: number): number {
  let lo = 0, hi = ranges.length;
  while (lo < hi) {
    const m = (lo + hi) >> 1;
    if (ranges[m][1] <= offset) lo = m + 1;
    else hi = m;
  }
  return lo;
}

const hex2 = (b: number) => b.toString(16).toUpperCase().padStart(2, "0");
const printable = (b: number) => (b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : "·");

export function HexDiff({ left, right, leftLabel, rightLabel }: { left: string; right: string; leftLabel: string; rightLabel: string }) {
  const fontSize = useStore((s) => s.settings?.fontSize ?? 13);
  const rowH = Math.round(fontSize * 1.55);
  const [d, setD] = useState<HexDiffResult | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewH, setViewH] = useState(400);
  const [cur, setCur] = useState(-1);
  const [, setTick] = useState(0);
  const body = useRef<HTMLDivElement>(null);
  const blocks = useRef(new Map<string, Uint8Array | null>()); // null = loading

  useEffect(() => {
    setD(null);
    setErr(null);
    blocks.current.clear();
    api.hexDiff(left, right).then((r) => r && setD(r)).catch((e) => setErr(String(e)));
    return () => { api.hexDiffCancel().catch(() => {}); };
  }, [left, right]);

  useEffect(() => {
    const el = body.current;
    if (!el) return;
    el.focus();
    const ro = new ResizeObserver(() => setViewH(el.clientHeight));
    ro.observe(el);
    return () => ro.disconnect();
  }, [d != null]); // eslint-disable-line react-hooks/exhaustive-deps

  const size = d ? Math.max(d.leftSize, d.rightSize) : 0;
  const rows = Math.ceil(size / PER_ROW);
  // Browsers cap an element's height (~33 M px): very long files scroll in scaled steps.
  const MAX_PX = 30_000_000;
  const scale = rows * rowH > MAX_PX ? (rows * rowH) / MAX_PX : 1;
  const total = (rows * rowH) / scale;
  const firstRow = Math.max(0, Math.floor((scrollTop * scale) / rowH) - 2);
  const shown = Math.ceil(viewH / rowH) + 4;
  const lastRow = Math.min(rows, firstRow + shown);
  const offW = Math.max(8, size.toString(16).length);

  /** Bytes of one side for [from, to), or null while a block is still loading. */
  const bytes = (side: 0 | 1, from: number, to: number): Uint8Array | null => {
    const path = side ? right : left;
    const out = new Uint8Array(to - from);
    for (let blk = Math.floor(from / BLOCK); blk * BLOCK < to; blk++) {
      const key = `${side}:${blk}`;
      if (!blocks.current.has(key)) {
        blocks.current.set(key, null);
        api.readBytes(path, blk * BLOCK, BLOCK)
          .then((b) => { blocks.current.set(key, new Uint8Array(b)); setTick((n) => n + 1); })
          .catch(() => blocks.current.delete(key));
      }
      const data = blocks.current.get(key);
      if (!data) return null;
      const s = Math.max(from, blk * BLOCK), e = Math.min(to, blk * BLOCK + data.length);
      if (e > s) out.set(data.subarray(s - blk * BLOCK, e - blk * BLOCK), s - from);
    }
    return out;
  };

  const goTo = (i: number) => {
    if (!d || !d.ranges.length || !body.current) return;
    const k = (i + d.ranges.length) % d.ranges.length;
    setCur(k);
    const row = Math.floor(d.ranges[k][0] / PER_ROW);
    body.current.scrollTop = (row * rowH) / scale - body.current.clientHeight / 3;
  };
  const step = (dir: 1 | -1) => {
    if (!d?.ranges.length) return;
    const topOffset = Math.floor((scrollTop * scale) / rowH) * PER_ROW;
    if (cur >= 0) return goTo(cur + dir);
    const k = rangeAt(d.ranges, topOffset);
    goTo(dir > 0 ? k : k - 1);
  };
  // Open at the first difference.
  useEffect(() => { if (d?.ranges.length) setTimeout(() => goTo(0), 0); }, [d]); // eslint-disable-line react-hooks/exhaustive-deps

  const marks = useMemo(() => {
    if (!d || !size) return [];
    // At most ~1500 marks: neighbours closer than a pixel row would overlap anyway.
    const stepN = Math.max(1, Math.ceil(d.ranges.length / 1500));
    return d.ranges.filter((_, i) => i % stepN === 0).map(([s, e]) => ({ top: (s / size) * 100, h: ((e - s) / size) * 100 }));
  }, [d, size]);

  if (err) return <div className="dv-msg err"><Icon name="close" size={16} /> {err}</div>;
  if (!d) return <div className="dv-msg">Comparing bytes…</div>;

  const row = (r: number) => {
    const from = r * PER_ROW, to = from + PER_ROW;
    const a = bytes(0, from, to), b = bytes(1, from, to);
    const cells = (side: 0 | 1) => {
      const mine = side ? b : a, other = side ? a : b;
      const sz = side ? d.rightSize : d.leftSize;
      const hex: React.ReactNode[] = [], txt: React.ReactNode[] = [];
      for (let i = 0; i < PER_ROW; i++) {
        const off = from + i;
        if (off >= sz) { hex.push(<span key={i} className="hx-b none">{"  "}</span>); txt.push(<span key={i} className="hx-c none"> </span>); continue; }
        if (!mine) { hex.push(<span key={i} className="hx-b">··</span>); txt.push(<span key={i} className="hx-c"> </span>); continue; }
        const v = mine[i];
        const otherSize = side ? d.leftSize : d.rightSize;
        const diff = off >= otherSize || (other != null && other[i] !== v);
        hex.push(<span key={i} className={`hx-b ${diff ? "diff" : ""} ${i === 7 ? "mid" : ""}`}>{hex2(v)}</span>);
        txt.push(<span key={i} className={`hx-c ${diff ? "diff" : ""}`}>{printable(v)}</span>);
      }
      return <><span className="hx-hex">{hex}</span><span className="hx-txt">{txt}</span></>;
    };
    const k = rangeAt(d.ranges, from);
    const changed = k < d.ranges.length && d.ranges[k][0] < to;
    return (
      <div key={r} className={`hx-row ${changed ? "chg" : ""}`} style={{ top: (r * rowH) / scale, height: rowH }}>
        <span className="hx-off">{from.toString(16).toUpperCase().padStart(offW, "0")}</span>
        <span className="hx-side l">{cells(0)}</span>
        <span className="hx-side r">{cells(1)}</span>
      </div>
    );
  };

  return (
    <div className="hx">
      <div className="hx-bar">
        <span className={`hx-verdict ${d.differing ? "diff" : "same"}`}>
          <Icon name={d.differing ? "compare" : "check"} size={15} />
          {d.differing
            ? <>{d.differing.toLocaleString()} byte{d.differing === 1 ? "" : "s"} differ in {d.ranges.length.toLocaleString()}{d.truncated ? "+" : ""} place{d.ranges.length === 1 ? "" : "s"}</>
            : "Identical, byte for byte"}
        </span>
        <span className="hx-sizes" title="Sizes">{leftLabel} {formatSize(d.leftSize)} · {rightLabel} {formatSize(d.rightSize)}</span>
        {d.ranges.length > 0 && (
          <span className="dv-nav">
            <button className="icon-btn" title="Previous difference (N, Shift+F8)" onClick={() => step(-1)}><Icon name="chevronUp" size={14} /></button>
            <span className="dv-count">{cur >= 0 ? (cur + 1).toLocaleString() : "–"} / {d.ranges.length.toLocaleString()}{d.truncated ? "+" : ""}</span>
            <button className="icon-btn" title="Next difference (n, F8)" onClick={() => step(1)}><Icon name="chevronDown" size={14} /></button>
          </span>
        )}
      </div>
      <div className="hx-heads">
        <span className="hx-off">Offset</span>
        <span className="hx-side l">{leftLabel}</span>
        <span className="hx-side r">{rightLabel}</span>
      </div>
      <div className="dv-scroll">
        <div className="hx-body" ref={body} tabIndex={0} onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
          onKeyDown={(e) => {
            const el = body.current;
            if (!el || e.ctrlKey || e.altKey || e.metaKey) return;
            const k = e.key;
            if (k === "n" || (k === "F8" && !e.shiftKey)) step(1);
            else if (k === "N" || (k === "F8" && e.shiftKey)) step(-1);
            else if (k === "j" || k === "ArrowDown") el.scrollTop += rowH;
            else if (k === "k" || k === "ArrowUp") el.scrollTop -= rowH;
            else if (k === "PageDown" || k === " ") el.scrollTop += el.clientHeight * 0.9;
            else if (k === "PageUp") el.scrollTop -= el.clientHeight * 0.9;
            else if (k === "Home" || k === "g") el.scrollTop = 0;
            else if (k === "End" || k === "G") el.scrollTop = total;
            else return; // Esc, q and the rest belong to the comparison window
            e.preventDefault();
            e.stopPropagation();
          }}>
          <div className="hx-canvas" style={{ height: total }}>
            {Array.from({ length: Math.max(0, lastRow - firstRow) }, (_, i) => row(firstRow + i))}
          </div>
        </div>
        <div className="dv-ruler" title="Differences: click to jump"
          onMouseDown={(e) => {
            const rr = e.currentTarget.getBoundingClientRect();
            const el = body.current;
            if (el) el.scrollTop = ((e.clientY - rr.top) / rr.height) * total - el.clientHeight / 2;
          }}>
          {marks.map((m, i) => <i key={i} className="m chg" style={{ top: `${m.top}%`, height: `max(2px, ${m.h}%)` }} />)}
          <b className="view" style={{ top: `${(scrollTop / Math.max(total, 1)) * 100}%`, height: `${Math.min(100, (viewH / Math.max(total, 1)) * 100)}%` }} />
        </div>
      </div>
    </div>
  );
}
