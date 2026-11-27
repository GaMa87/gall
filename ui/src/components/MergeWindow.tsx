import { memo, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { SyntaxHighlight } from "../api/types";
import { colouredLine } from "./MergeHighlight";
import { MAX_WINDOW_HEIGHT, ROW_HEIGHT, rowCount, rowText, windowHeight, windowRange } from "../core/windowRows";

type RowProps = {
  index: number;
  columns: string[][];
  classes: string[];
  syntax?: (SyntaxHighlight | null)[];
  offsets?: number[];
};

const MergeWindowRow = memo(function MergeWindowRow({ index, columns, classes, syntax, offsets }: RowProps) {
  return (
    <div className="gm-win-row" style={{ top: index * ROW_HEIGHT }}>
      {columns.map((column, k) => {
        const text = rowText(column[index] ?? "");
        const highlight = syntax?.[k];
        return (
          <pre key={k} className={`gm-win-cell ${classes[k] ?? ""}`}>
            {highlight ? colouredLine(text, highlight, (offsets?.[k] ?? 0) + index) : text}
          </pre>
        );
      })}
    </div>
  );
});

// Keep the current window while scrolling within this many lines. The mounted
// tail covers the distance to the next window, so every visible row is present.
const SCROLL_CHUNK = 12 * ROW_HEIGHT;

/**
 * Rows of one or more columns that share the same alignment, rendered as a window: only the rows in view (and a few
 * around them) exist in the document, whatever the number of lines. A column shorter than the others is blank at the
 * bottom; every column scrolls together because they are the cells of the same rows.
 */
export function MergeWindow({ columns, labels, classes, syntax, offsets, vim }: { columns: string[][]; labels: string[]; classes: string[]; syntax?: (SyntaxHighlight | null)[]; offsets?: number[]; vim: boolean }) {
  const count = rowCount(columns);
  // Keep the scroll width stable while most rows are off screen. The grid can
  // also grow to fit the actual glyphs if a font needs more room for a visible row.
  const widths = useMemo(() => columns.map((column) => column.reduce((max, line) => {
    let width = 0;
    for (const character of rowText(line)) {
      if (character === "\t") width += 8 - (width % 8);
      else width += character > "\u00ff" ? 2 : 1;
    }
    return Math.max(max, width);
  }, 24)), [columns]);
  const pane = useRef<HTMLDivElement>(null);
  const [top, setTop] = useState(0);
  const [viewHeight, setViewHeight] = useState<number>();
  const height = windowHeight(count);
  useLayoutEffect(() => {
    const element = pane.current!;
    const measure = () => setViewHeight(element.clientHeight);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  const { start, end } = windowRange(top, (viewHeight ?? height) + SCROLL_CHUNK, ROW_HEIGHT, count);
  const tracks = widths.map((width) => `minmax(${width}ch, 1fr)`).join(" ");
  const minWidth = `calc(${widths.reduce((a, b) => a + b, 0)}ch + ${(columns.length - 1) * 6}px)`;
  const rows: number[] = [];
  for (let i = start; i < end; i++) rows.push(i);
  return (
    <div
      className="gm-win"
      ref={pane}
      role="group"
      tabIndex={0}
      aria-label={labels.join(" · ")}
      style={{ height, maxHeight: MAX_WINDOW_HEIGHT }}
      onScroll={(e) => setTop(Math.floor(e.currentTarget.scrollTop / SCROLL_CHUNK) * SCROLL_CHUNK)}
      onKeyDown={(event) => {
        if (event.target !== event.currentTarget || event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return;
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
          // Keep native scrolling in this pane rather than stepping conflicts.
          event.stopPropagation();
        } else if (vim && (event.key === "j" || event.key === "k")) {
          event.preventDefault();
          event.stopPropagation();
          event.currentTarget.scrollTop += event.key === "j" ? ROW_HEIGHT : -ROW_HEIGHT;
        }
      }}
    >
      <div className="gm-win-space" style={{ height: count * ROW_HEIGHT, minWidth, "--gm-tracks": tracks } as React.CSSProperties}>
        {rows.map((i) => (
          <MergeWindowRow key={i} index={i} columns={columns} classes={classes} syntax={syntax} offsets={offsets} />
        ))}
      </div>
    </div>
  );
}
