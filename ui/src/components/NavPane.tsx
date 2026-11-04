// Navigation pane: Miller columns (N-01…N-05) or breadcrumbs (N-04).
import { useEffect, useMemo, useRef, useState } from "react";
import type { FileEntry } from "../api/types";
import { fitColumnWidths } from "../core/columns";
import { quickFilterMatches } from "../core/filter";
import { ancestors, basename, isInside } from "../core/path";
import { naturalCmp } from "../core/sort";
import * as A from "../state/actions";
import { dropInto, onDragOverTarget, startDrag } from "../state/dnd";
import { useDirList } from "../state/hooks";
import { useStore } from "../state/store";
import { FileIcon, Icon } from "./Icon";

/** Breadcrumbs: every segment is clickable and a drop target. */
export function Breadcrumbs({ b, path }: { b: number; path: string }) {
  const chain = ancestors(path);
  const [drop, setDrop] = useState<string | null>(null);
  // A drive root (C:\, /) alone would only repeat the tab and the folder header: then the
  // bar keeps just its icon (it stays, so the list does not move up and down).
  const shown = chain.length > 1 ? chain : [];
  return (
    <div className="bcrumbs">
      <button className="bc-icon" title={`${path}\nEdit path (Ctrl+L)`} onClick={() => A.openOverlay({ type: "pathEdit", b })}>
        <Icon name="crumbs" size={14} />
      </button>
      {shown.map((p, i) => (
        <span key={p} style={{ display: "contents" }}>
          {i > 0 && <span className="sep">›</span>}
          <span
            className={`seg ${i === chain.length - 1 ? "last" : ""} ${drop === p ? "drop" : ""}`}
            title={p}
            onClick={() => A.navigate(b, p)}
            onMouseDown={(e) => {
              if (e.button === 1) {
                e.preventDefault();
                A.sessionCmd({ op: "newTab", b, path: p, activate: false });
              }
            }}
            onDragOver={(e) => onDragOverTarget(e, p) && setDrop(p)}
            onDragLeave={() => setDrop(null)}
            onDrop={(e) => {
              setDrop(null);
              dropInto(e, p);
            }}
          >
            {basename(p)}
          </span>
        </span>
      ))}
      <span className="grow" title="Edit path (\\)" onClick={() => A.openOverlay({ type: "pathEdit", b })} />
    </div>
  );
}

interface ColProps {
  b: number;
  dir: string;
  /** Child of `dir` that is on the current path (highlighted). */
  onPath: string | null;
  width: number;
  isCurrent: boolean;
  onHover?: (hovering: boolean) => void;
}

/** One Miller column listing the sub-folders of `dir`. */
function MillerColumn({ b, dir, onPath, width, isCurrent, onHover }: ColProps) {
  const settings = useStore((s) => s.settings);
  const { entries } = useDirList(dir, settings, true);
  const [filter, setFilter] = useState("");
  const [drop, setDrop] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const sorted = useMemo(
    () => entries.filter((e) => quickFilterMatches(e.name, filter)).sort((x, y) => naturalCmp(x.name, y.name)),
    [entries, filter],
  );

  useEffect(() => {
    if (!onPath || !listRef.current) return;
    const el = listRef.current.querySelector<HTMLElement>(`[data-p="${CSS.escape(onPath)}"]`);
    el?.scrollIntoView({ block: "nearest" });
  }, [onPath, sorted.length]);

  const item = (e: FileEntry) => (
    <div
      key={e.path}
      data-p={e.path}
      className={`mcol-item ${e.path === onPath ? "on-path" : ""} ${drop === e.path ? "drop" : ""}`}
      title={e.name}
      draggable
      onDragStart={(ev) => startDrag(ev, [e.path])}
      onClick={() => {
        A.focusBrowser(b);
        A.navigate(b, e.path);
      }}
      onMouseDown={(ev) => {
        if (ev.button === 1) {
          ev.preventDefault();
          if (ev.altKey) A.openInOther(b, e.path, false);
          else A.sessionCmd({ op: "newTab", b, path: e.path, activate: false });
        }
      }}
      onContextMenu={(ev) => {
        ev.preventDefault();
        A.showMenu(ev.clientX, ev.clientY, [
          { label: "Open", run: () => A.navigate(b, e.path) },
          { label: "Open in new tab", run: () => A.sessionCmd({ op: "newTab", b, path: e.path }) },
          { label: A.hasOtherTarget(b) ? "Open in other browser" : "Open in new tab", run: () => A.openInOther(b, e.path) },
          { separator: true },
          { label: "Copy path", run: () => A.copyPaths([], e.path) },
          { label: "Move to trash", icon: "trash", run: () => A.trashPaths([e.path]) },
        ]);
      }}
      onDragOver={(ev) => onDragOverTarget(ev, e.path) && setDrop(e.path)}
      onDragLeave={() => setDrop(null)}
      onDrop={(ev) => {
        setDrop(null);
        dropInto(ev, e.path);
      }}
    >
      <FileIcon entry={e} size={14} open={e.path === onPath} />
      <span className="label">{e.name}</span>
    </div>
  );

  return (
    <div
      className={`mcol ${isCurrent ? "current-col" : ""}`}
      style={{ flex: `0 0 ${width}px`, width }}
      onMouseEnter={() => onHover?.(true)}
      onMouseLeave={() => onHover?.(false)}
      tabIndex={-1}
      onKeyDown={(e) => {
        if (e.key.length === 1 && /[\p{L}\p{N}.]/u.test(e.key) && !e.ctrlKey && !e.altKey) {
          setFilter((f) => f + e.key);
          e.stopPropagation();
        } else if (e.key === "Escape" && filter) {
          setFilter("");
          e.stopPropagation();
        }
      }}
    >
      <div
        className="mcol-head"
        title={dir}
        onClick={() => A.navigate(b, dir)}
        onDragOver={(e) => onDragOverTarget(e, dir)}
        onDrop={(e) => dropInto(e, dir)}
      >
        {basename(dir)}
      </div>
      {filter && (
        <div className="mcol-filter">
          <Icon name="search" size={12} />
          {filter}
          <button className="icon-btn" onClick={() => setFilter("")}>×</button>
        </div>
      )}
      <div className="mcol-list" ref={listRef}>{sorted.map(item)}</div>
    </div>
  );
}

/** Drives column: leftmost column in the navigation pane (N-02). */
function DrivesColumn({ b, path, width }: { b: number; path: string; width: number }) {
  const drives = useStore((s) => s.drives);
  const current = drives.filter((d) => isInside(path, d.mount)).sort((x, y) => y.mount.length - x.mount.length)[0];
  return (
    <div className="mcol drives-col" style={{ flex: `0 0 ${width}px`, width }}>
      <div className="mcol-head" style={{ fontSize: "0.95em" }}><Icon name="drive" size={14} /></div>
      <div className="mcol-list">
        {drives.map((d) => (
          <div key={d.mount} className={`mcol-item ${current?.mount === d.mount ? "on-path" : ""}`} title={`${d.label || d.short} — ${d.mount}`}
            onClick={() => A.navigate(b, d.mount)}
            onDragOver={(e) => onDragOverTarget(e, d.mount)} onDrop={(e) => dropInto(e, d.mount)}>
            <span className="label">{d.short}</span>
            {width > 60 && d.total > 0 && !d.spaceShownBy && <small>{Math.round((1 - d.available / d.total) * 100)}%</small>}
          </div>
        ))}
      </div>
    </div>
  );
}

/**
 * Miller columns for `path`. Ancestor columns compress to fit; a hovered
 * column expands after a delay (configurable, N-03).
 */
export function MillerColumns({ b, path, width, includeCurrent, trailing }: {
  b: number;
  path: string;
  width: number;
  /** Show a column listing the current folder's sub-folders (Standard layout). */
  includeCurrent: boolean;
  /** Element rendered after the columns (Columns layout: the folder pane). */
  trailing?: React.ReactNode;
}) {
  const settings = useStore((s) => s.settings)!;
  const home = useStore((s) => s.info?.home ?? "");
  const [hover, setHover] = useState<number | null>(null);
  const timer = useRef<number | null>(null);
  const full = ancestors(path);
  // Finder-style: inside the home folder, start at home (drives column still reaches /).
  const homeIdx = settings.columnsFromHome && home && home !== "/" && isInside(path, home) ? full.indexOf(home) : -1;
  const chain = homeIdx > 0 ? full.slice(homeIdx) : full;
  const cols = includeCurrent ? chain : chain.slice(0, -1);
  const drivesW = 64;
  const trailingMin = trailing ? Math.max(320, width * 0.45) : 0;
  // Each column draws a 1px right border (drives column included).
  const avail = Math.max(0, width - drivesW - trailingMin - (cols.length + 1));
  const minW = settings.columnMinWidth;
  const maxW = settings.columnMaxWidth;
  const hovered = hover !== null && hover < cols.length && settings.columnHoverExpand ? hover : null;
  const widths = fitColumnWidths(cols.length, avail, minW, maxW, hovered);
  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    // Keep the newest columns visible.
    if (scrollRef.current) scrollRef.current.scrollLeft = scrollRef.current.scrollWidth;
  }, [path, width]);

  return (
    <div className="nav-pane" ref={scrollRef} style={{ overflowX: "auto", flex: 1, borderBottom: trailing ? "none" : undefined }}>
      <DrivesColumn b={b} path={path} width={drivesW} />
      {cols.map((dir, i) => (
        <MillerColumn
          key={dir}
          b={b}
          dir={dir}
          onPath={chain[i + 1] ?? null}
          width={widths[i]}
          isCurrent={i === cols.length - 1 && includeCurrent}
          onHover={(h) => {
            if (timer.current) clearTimeout(timer.current);
            if (!settings.columnHoverExpand) return;
            if (h) timer.current = window.setTimeout(() => setHover(i), settings.columnExpandDelayMs);
            else setHover(null);
          }}
        />
      ))}
      {trailing}
    </div>
  );
}
