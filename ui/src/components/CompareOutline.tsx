import { useEffect, useMemo, useRef, useState } from "react";
import type { DiffOp, OutlineNode } from "../api/types";
import { tt } from "../i18n";
import { changeCount, idsWithChanges, pressKey, rowLabel, visibleRows, type OutlineKey, type OutlineState } from "../core/outline";

// Vim keys next to the arrows, as everywhere in the comparison window.
const VIM: Record<string, OutlineKey> = { j: "ArrowDown", k: "ArrowUp", h: "ArrowLeft", l: "ArrowRight", g: "Home", G: "End" };
const KEYS = new Set<string>(["ArrowDown", "ArrowUp", "ArrowLeft", "ArrowRight", "Home", "End", "Enter", " "]);

interface Props {
  nodes: OutlineNode[];
  /** The list was cut short: a very large file lists only its first nodes. */
  truncated?: boolean;
  ops: DiffOp[];
  /** The side whose text the outline was made from. */
  side: "a" | "b";
  /** A row was chosen (Enter, Space or a click): go to its first line (1-based). */
  onJump: (line: number) => void;
  onClose: () => void;
  /** Shows the outline of the other side (the `O` key of the window). */
  onSwitchSide?: () => void;
}

/**
 * The structural outline beside the comparison: a tree of modules, classes and functions, each with the number of
 * changes inside it. Keys follow the ARIA tree pattern (arrows, Home, End, Enter), with j k h l g G next to them;
 * the tree is one tab stop and names the active row through `aria-activedescendant`. Esc closes the pane.
 */
export function CompareOutline({ nodes, truncated, ops, side, onJump, onClose, onSwitchSide }: Props) {
  const [state, setState] = useState<OutlineState>(() => ({ active: nodes.length ? "0" : null, expanded: idsWithChanges(nodes, ops, side) }));
  const ref = useRef<HTMLDivElement>(null);
  const rows = useMemo(() => visibleRows(nodes, state.expanded), [nodes, state.expanded]);
  useEffect(() => { ref.current?.focus(); }, []);
  useEffect(() => { document.getElementById(`ol-${state.active}`)?.scrollIntoView({ block: "nearest" }); }, [state.active]);

  const press = (key: OutlineKey) => {
    const r = pressKey(nodes, state, key);
    setState(r.state);
    if (r.activate) {
      const row = rows.find((x) => x.id === r.state.active);
      if (row) onJump(row.node.span.line);
    }
  };

  return (
    <div className="dv-outline" onKeyDown={(e) => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(); } }}>
      <div className="dv-outline-head"><b>{tt("Outline")} · {side === "a" ? tt("left") : tt("right")}</b>{onSwitchSide && <button className="icon-btn" aria-label={tt("Outline of the other side (O)")} title={tt("Outline of the other side (O)")} onClick={onSwitchSide}>⇄</button>}<button className="icon-btn" aria-label={tt("Close outline (Esc)")} title={tt("Close outline (Esc)")} onClick={onClose}>×</button></div>
      <div
        ref={ref}
        className="dv-outline-tree"
        role="tree"
        tabIndex={0}
        aria-label={tt("Outline")}
        aria-activedescendant={state.active ? `ol-${state.active}` : undefined}
        onKeyDown={(e) => {
          if (e.ctrlKey || e.metaKey || e.altKey) return;
          const key = KEYS.has(e.key) ? (e.key as OutlineKey) : VIM[e.key];
          if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(); return; }
          if (!key) return;
          e.preventDefault();
          e.stopPropagation();
          press(key);
        }}
      >
      {truncated && <p className="dv-outline-empty" role="status">{tt("The file is very large: only the first part is listed.")}</p>}
      {rows.length === 0 && <p className="dv-outline-empty">{tt("No modules, classes or functions.")}</p>}
      {rows.map((r) => {
        const n = changeCount(r.node, ops, side);
        return (
          <div
            key={r.id}
            id={`ol-${r.id}`}
            role="treeitem"
            aria-level={r.depth + 1}
            aria-setsize={r.siblings}
            aria-posinset={r.position}
            aria-expanded={r.hasChildren ? r.open : undefined}
            aria-selected={r.id === state.active}
            aria-label={rowLabel(r.node, n)}
            className={`dv-outline-row ${r.id === state.active ? "on" : ""} ${n > 0 ? "chg" : ""}`}
            style={{ paddingLeft: 8 + r.depth * 14 }}
            onClick={() => { setState((s) => ({ ...s, active: r.id })); onJump(r.node.span.line); }}
          >
            <span className="dv-outline-twist" aria-hidden>{r.hasChildren ? (r.open ? "▾" : "▸") : ""}</span>
            <span className="dv-outline-kind" aria-hidden>{r.node.kind}</span>
            <span className="dv-outline-name">{r.node.name ?? <i>{tt("anonymous")}</i>}</span>
            {n > 0 && <span className="dv-outline-count" aria-hidden>{n}</span>}
          </div>
        );
      })}
      </div>
    </div>
  );
}
