// Hex comparison of two binary files (D-04): offset, 16 bytes as hex and as text for each
// side, differing bytes marked; next / previous difference; an overview ruler. The backend
// streams both files once for the differing ranges; the bytes shown are read in blocks as
// they scroll into view, so files of any size open at once.
import { memo, useEffect, useMemo, useRef, useState } from "react";
import { tt } from "../i18n";
import { api } from "../api/ipc";
import type { HexDiffResult } from "../api/types";
import { formatSize } from "../core/format";
import { HEX_BLOCK, HEX_PER_ROW, pruneBlockCache, visibleBlockKeys } from "../core/hexBlocks";
import { parseOffset, rowOfOffset } from "../core/hexOffset";
import { useStore } from "../state/store";
import { Icon } from "./Icon";

const PER_ROW = HEX_PER_ROW;
const BLOCK = HEX_BLOCK;
const LOADING_HEX = "·· ·· ·· ·· ·· ·· ·· ··  ·· ·· ·· ·· ·· ·· ·· ··";
const LOADING_TXT = "                ";

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

const HEX_TABLE: string[] = Array.from({ length: 256 }, (_, b) => b.toString(16).toUpperCase().padStart(2, "0"));
const TXT_TABLE: string[] = Array.from({ length: 256 }, (_, b) => (b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : "·"));

function formatPlainHex(bytes: Uint8Array): string {
  let out = "";
  for (let i = 0; i < PER_ROW; i++) {
    if (i > 0) out += i === 8 ? "  " : " ";
    out += HEX_TABLE[bytes[i]];
  }
  return out;
}

function formatPlainTxt(bytes: Uint8Array): string {
  let out = "";
  for (let i = 0; i < PER_ROW; i++) out += TXT_TABLE[bytes[i]];
  return out;
}

function renderSide(
  from: number,
  sz: number,
  otherSize: number,
  mine: Uint8Array | null,
  other: Uint8Array | null,
  changed: boolean,
): React.ReactNode {
  const full = from + PER_ROW <= sz;
  if (!mine) {
    if (full) {
      return <><span className="hx-hex">{LOADING_HEX}</span><span className="hx-txt">{LOADING_TXT}</span></>;
    }
    let hx = "";
    for (let i = 0; i < PER_ROW; i++) {
      if (i > 0) hx += i === 8 ? "  " : " ";
      hx += from + i < sz ? "··" : "  ";
    }
    return <><span className="hx-hex">{hx}</span><span className="hx-txt">{LOADING_TXT}</span></>;
  }
  if (!changed && full && from + PER_ROW <= otherSize) {
    return <><span className="hx-hex">{formatPlainHex(mine)}</span><span className="hx-txt">{formatPlainTxt(mine)}</span></>;
  }
  const hexNodes: React.ReactNode[] = [];
  const txtNodes: React.ReactNode[] = [];
  let plainHex = "";
  let plainTxt = "";
  const flushHex = () => { if (plainHex) { hexNodes.push(plainHex); plainHex = ""; } };
  const flushTxt = () => { if (plainTxt) { txtNodes.push(plainTxt); plainTxt = ""; } };
  for (let i = 0; i < PER_ROW; i++) {
    const sep = i === 0 ? "" : i === 8 ? "  " : " ";
    const off = from + i;
    if (off >= sz) {
      plainHex += sep + "  ";
      plainTxt += " ";
      continue;
    }
    const v = mine[i];
    const diff = off >= otherSize || (other != null && other[i] !== v);
    if (diff) {
      plainHex += sep;
      flushHex();
      hexNodes.push(<span key={i} className="hx-b diff">{HEX_TABLE[v]}</span>);
      flushTxt();
      txtNodes.push(<span key={i} className="hx-c diff">{TXT_TABLE[v]}</span>);
    } else {
      plainHex += sep + HEX_TABLE[v];
      plainTxt += TXT_TABLE[v];
    }
  }
  flushHex();
  flushTxt();
  return <><span className="hx-hex">{hexNodes}</span><span className="hx-txt">{txtNodes}</span></>;
}

const HexRow = memo(function HexRow({
  from,
  offW,
  top,
  rowH,
  changed,
  leftSize,
  rightSize,
  leftBlock,
  rightBlock,
  blkOff,
}: {
  from: number;
  offW: number;
  top: number;
  rowH: number;
  changed: boolean;
  leftSize: number;
  rightSize: number;
  leftBlock: Uint8Array | null;
  rightBlock: Uint8Array | null;
  blkOff: number;
}) {
  const a = leftBlock && blkOff < leftBlock.length ? leftBlock.subarray(blkOff, Math.min(leftBlock.length, blkOff + PER_ROW)) : null;
  const b = rightBlock && blkOff < rightBlock.length ? rightBlock.subarray(blkOff, Math.min(rightBlock.length, blkOff + PER_ROW)) : null;
  return (
    <div className={`hx-row ${changed ? "chg" : ""}`} style={{ top, height: rowH }}>
      <span className="hx-off">{from.toString(16).toUpperCase().padStart(offW, "0")}</span>
      <span className="hx-side l">{renderSide(from, leftSize, rightSize, a, b, changed)}</span>
      <span className="hx-side r">{renderSide(from, rightSize, leftSize, b, a, changed)}</span>
    </div>
  );
});

export function HexDiff({ left, right, leftLabel, rightLabel }: { left: string; right: string; leftLabel: string; rightLabel: string }) {
  const fontSize = useStore((s) => s.settings?.fontSize ?? 13);
  const rowH = Math.round(fontSize * 1.55);
  const [d, setD] = useState<HexDiffResult | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewH, setViewH] = useState(400);
  const [cur, setCur] = useState(-1);
  // The offset typed into the bar, and why it was refused.
  const [offsetText, setOffsetText] = useState(""), [offsetError, setOffsetError] = useState("");
  const [, setTick] = useState(0);
  const body = useRef<HTMLDivElement>(null);
  const blocks = useRef(new Map<string, Uint8Array>());
  const inFlight = useRef(new Set<string>());
  const visibleKeys = useRef(new Set<string>());
  const pairGen = useRef(0);
  const stats = useRef({ started: 0, applied: 0, dropped: 0 });
  const tickRaf = useRef(0);
  const scheduleTick = () => {
    if (!tickRaf.current) {
      tickRaf.current = requestAnimationFrame(() => {
        tickRaf.current = 0;
        setTick((n) => n + 1);
      });
    }
  };

  useEffect(() => () => {
    if (tickRaf.current) cancelAnimationFrame(tickRaf.current);
  }, []);

  useEffect(() => {
    pairGen.current += 1;
    setD(null);
    setErr(null);
    setCur(-1);
    setScrollTop(0);
    blocks.current.clear();
    inFlight.current.clear();
    visibleKeys.current.clear();
    stats.current = { started: 0, applied: 0, dropped: 0 };
    api.hexDiff(left, right).then((r) => r && setD(r)).catch((e) => setErr(String(e)));
    return () => { api.hexDiffCancel().catch(() => {}); };
  }, [left, right]);

  useEffect(() => {
    const el = body.current;
    if (!el) return;
    el.focus();
    setViewH(el.clientHeight || 400);
    const ro = new ResizeObserver(() => setViewH(el.clientHeight || 400));
    ro.observe(el);
    return () => ro.disconnect();
  }, [d != null]); // eslint-disable-line react-hooks/exhaustive-deps

  const size = d ? Math.max(d.leftSize, d.rightSize) : 0;
  const rows = Math.ceil(size / PER_ROW);
  // Browsers cap layout height (WebKitGTK LayoutUnit saturates around 33.5M device px, which
  // is ~22M CSS px at 1.5x scale). Keep the virtual canvas <= 10M CSS px and map the scrollbar
  // linearly to logical rows while keeping rendered rows spaced at full `rowH` pixels apart.
  const MAX_PX = 10_000_000;
  const scale = rows * rowH > MAX_PX ? (rows * rowH) / MAX_PX : 1;
  const total = (rows * rowH) / scale;
  const scrollChunk = 12 * rowH;
  const maxScroll = Math.max(0, total - viewH);
  const maxBaseRow = Math.max(0, rows - Math.floor(viewH / rowH));
  const exactRow = scale === 1
    ? scrollTop / rowH
    : maxScroll > 0 ? (scrollTop / maxScroll) * maxBaseRow : 0;
  const baseRow = Math.min(maxBaseRow, Math.max(0, Math.floor(exactRow)));
  const fracPx = (exactRow - baseRow) * rowH;
  const firstRow = Math.max(0, baseRow - 2);
  const shown = scale === 1
    ? Math.ceil((viewH + scrollChunk) / rowH) + 4
    : Math.ceil(viewH / rowH) + 4;
  const lastRow = Math.min(rows, firstRow + shown);
  const offW = Math.max(8, size.toString(16).length);
  const topOf = (r: number) => (scale === 1 ? r * rowH : scrollTop - fracPx + (r - baseRow) * rowH);
  const scrollForRow = (row: number) => {
    const h = body.current?.clientHeight ?? viewH;
    if (scale === 1) return Math.max(0, row * rowH - h / 3);
    const maxS = Math.max(0, total - h);
    const maxBase = Math.max(1, rows - Math.floor(h / rowH));
    const targetBase = Math.max(0, Math.min(maxBase, row - Math.floor(h / (3 * rowH))));
    return (targetBase / maxBase) * maxS;
  };

  // Request only the blocks currently in view; drop stale completions when the user has
  // already scrolled past them or opened another pair, and keep the cache bounded.
  useEffect(() => {
    if (!d) return;
    const needed = visibleBlockKeys(firstRow, lastRow, d.leftSize, d.rightSize);
    const keep = new Set(needed.map((n) => n.key));
    visibleKeys.current = keep;
    pruneBlockCache(blocks.current, keep);
    const gen = pairGen.current;
    for (const { side, block, key } of needed) {
      const existing = blocks.current.get(key);
      if (existing) {
        // Touch LRU order.
        blocks.current.delete(key);
        blocks.current.set(key, existing);
        continue;
      }
      if (inFlight.current.has(key)) continue;
      inFlight.current.add(key);
      stats.current.started += 1;
      const path = side ? right : left;
      api.readBytes(path, block * BLOCK, BLOCK)
        .then((b) => {
          inFlight.current.delete(key);
          if (gen !== pairGen.current || !visibleKeys.current.has(key)) {
            stats.current.dropped += 1;
            return;
          }
          blocks.current.set(key, b instanceof Uint8Array ? b : new Uint8Array(b));
          pruneBlockCache(blocks.current, visibleKeys.current);
          stats.current.applied += 1;
          scheduleTick();
        })
        .catch(() => {
          inFlight.current.delete(key);
        });
    }
  }, [d, left, right, firstRow, lastRow]);

  const onBodyScroll = (e: React.UIEvent<HTMLDivElement>) => {
    const next = e.currentTarget.scrollTop;
    setScrollTop(scale === 1 ? Math.floor(next / scrollChunk) * scrollChunk : next);
  };

  const goTo = (i: number) => {
    if (!d || !d.ranges.length || !body.current) return;
    const k = (i + d.ranges.length) % d.ranges.length;
    setCur(k);
    const row = Math.floor(d.ranges[k][0] / PER_ROW);
    body.current.scrollTop = scrollForRow(row);
  };
  /** Brings a typed offset (decimal, 0x hex or end) into view. Both sides share the row, so one scroll positions both. */
  const goToOffset = () => {
    const found = parseOffset(offsetText, size);
    if (!found.ok) { setOffsetError(found.reason); return; }
    setOffsetError("");
    const el = body.current;
    if (!el) return;
    el.scrollTop = scrollForRow(rowOfOffset(found.offset, size, PER_ROW));
    el.focus();
  };
  const step = (dir: 1 | -1) => {
    if (!d?.ranges.length) return;
    const topOffset = baseRow * PER_ROW;
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
  if (!d) return <div className="dv-msg">{tt("Comparing bytes…")}</div>;

  const row = (r: number, slot: number) => {
    const from = r * PER_ROW, to = from + PER_ROW;
    const blk = Math.floor(from / BLOCK);
    const blkOff = from - blk * BLOCK;
    const leftBlock = blocks.current.get(`0:${blk}`) ?? null;
    const rightBlock = blocks.current.get(`1:${blk}`) ?? null;
    const k = rangeAt(d.ranges, from);
    const changed = k < d.ranges.length && d.ranges[k][0] < to;
    return (
      <HexRow
        key={scale > 1 ? slot : r}
        from={from}
        offW={offW}
        top={topOf(r)}
        rowH={rowH}
        changed={changed}
        leftSize={d.leftSize}
        rightSize={d.rightSize}
        leftBlock={leftBlock}
        rightBlock={rightBlock}
        blkOff={blkOff}
      />
    );
  };

  return (
    <div
      className="hx"
      data-read-started={stats.current.started}
      data-read-applied={stats.current.applied}
      data-read-dropped={stats.current.dropped}
      data-cached-blocks={blocks.current.size}
    >
      <div className="hx-bar">
        <span className={`hx-verdict ${d.differing ? "diff" : "same"}`}>
          <Icon name={d.differing ? "compare" : "check"} size={15} />
          {d.differing
            ? <>{d.differing.toLocaleString()} byte{d.differing === 1 ? "" : "s"} differ in {d.ranges.length.toLocaleString()}{d.truncated ? "+" : ""} place{d.ranges.length === 1 ? "" : "s"}</>
            : "Identical, byte for byte"}
        </span>
        <span className="hx-sizes" title={tt("Bytes are compared at the same offset in both files, not by content: an insertion shifts every byte after it.")}>{leftLabel} {formatSize(d.leftSize)} · {rightLabel} {formatSize(d.rightSize)}</span>
        {d.leftSize !== d.rightSize && <span className="hx-align" role="note">{tt("Different sizes: compared at the same offsets, so an insertion shows every later byte as different. {count} bytes past the shorter file have no counterpart.", { count: Math.abs(d.leftSize - d.rightSize).toLocaleString() })}</span>}
        <form className="hx-goto" onSubmit={(e) => { e.preventDefault(); goToOffset(); }}>
          <input type="text" value={offsetText} spellCheck={false} autoComplete="off" inputMode="text" size={12}
            aria-label={tt("Go to offset: a number, 0x and hex digits, or end")} aria-invalid={offsetError ? true : undefined} aria-describedby={offsetError ? "hx-offset-error" : undefined}
            placeholder={tt("Offset: 4096, 0x1000, end")} title={tt("Go to offset: a number, 0x and hex digits, or end")}
            onChange={(e) => { setOffsetText(e.target.value); setOffsetError(""); }} />
          <button type="submit" className="icon-btn" title={tt("Go to offset")}>{tt("Go")}</button>
          {offsetError && <span id="hx-offset-error" className="hx-offset-error" role="alert">{tt(offsetError, { size: size.toLocaleString() })}</span>}
        </form>
        {d.ranges.length > 0 && (
          <span className="dv-nav">
            <button className="icon-btn" title={tt("Previous difference (N, Shift+F8)")} onClick={() => step(-1)}><Icon name="chevronUp" size={14} /></button>
            <span className="dv-count">{cur >= 0 ? (cur + 1).toLocaleString() : "–"} / {d.ranges.length.toLocaleString()}{d.truncated ? "+" : ""}</span>
            <button className="icon-btn" title={tt("Next difference (n, F8)")} onClick={() => step(1)}><Icon name="chevronDown" size={14} /></button>
          </span>
        )}
      </div>
      <div className="hx-heads">
        <span className="hx-off">{tt("Offset")}</span>
        <span className="hx-side l">{leftLabel}</span>
        <span className="hx-side r">{rightLabel}</span>
      </div>
      <div className="dv-scroll">
        <div className="hx-body" ref={body} tabIndex={0} onScroll={onBodyScroll}
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
            {Array.from({ length: Math.max(0, lastRow - firstRow) }, (_, i) => row(firstRow + i, i))}
          </div>
        </div>
        <div className="dv-ruler" title={tt("Differences: click to jump")}
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
