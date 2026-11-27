import { useEffect, useRef, useState } from "react";
import type { SemanticChange, SemanticChangeKind } from "../api/types";
import { tt } from "../i18n";

const KIND_LABEL: Record<SemanticChangeKind, string> = {
  rename: "Renamed",
  signature: "Signature",
  moved: "Moved",
  behavior: "Behavior",
  added: "Added",
  removed: "Removed",
};

interface Props {
  changes: SemanticChange[];
  /** Whether Vim motion keys (j, k, g, G, h, l) are enabled. */
  vim: boolean;
  /** Jump to a 1-based line on the left (`"l"`) or right (`"r"`) side. */
  onJump: (side: "l" | "r", line: number) => void;
  onClose: () => void;
}

export type SemanticKeyAction =
  | { type: "move"; index: number }
  | { type: "jump"; side: "l" | "r"; line: number }
  | { type: "close" }
  | null;

/** Pure keyboard dispatch for `SemanticDetails`, covering arrow keys and optional Vim motions. */
export function resolveSemanticKey(
  changes: readonly SemanticChange[],
  active: number,
  key: string,
  vim: boolean,
  isListTarget: boolean,
): SemanticKeyAction {
  if (key === "Escape") return { type: "close" };
  if (!changes.length) return null;
  const clamped = Math.max(0, Math.min(active, changes.length - 1));
  const cur = changes[clamped];
  if (key === "ArrowDown" || (vim && key === "j")) return { type: "move", index: Math.min(changes.length - 1, clamped + 1) };
  if (key === "ArrowUp" || (vim && key === "k")) return { type: "move", index: Math.max(0, clamped - 1) };
  if (key === "Home" || (vim && key === "g")) return { type: "move", index: 0 };
  if (key === "End" || (vim && key === "G")) return { type: "move", index: changes.length - 1 };
  if ((key === "ArrowLeft" || (vim && key === "h")) && cur?.left) return { type: "jump", side: "l", line: cur.left.line };
  if ((key === "ArrowRight" || (vim && key === "l")) && cur?.right) return { type: "jump", side: "r", line: cur.right.line };
  if ((key === "Enter" || key === " ") && isListTarget && cur) {
    if (cur.right) return { type: "jump", side: "r", line: cur.right.line };
    if (cur.left) return { type: "jump", side: "l", line: cur.left.line };
  }
  return null;
}

/**
 * Details pane for syntax-tree comparison changes: lists every classified item or member change
 * with buttons and keyboard shortcuts to jump to the left and right lines.
 */
export function SemanticDetails({ changes, vim, onJump, onClose }: Props) {
  const [active, setActive] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  const clamped = changes.length ? Math.min(active, changes.length - 1) : -1;

  useEffect(() => { listRef.current?.focus(); }, []);
  useEffect(() => {
    if (clamped >= 0) {
      document.getElementById(`sem-${clamped}`)?.scrollIntoView({ block: "nearest" });
    }
  }, [clamped]);

  const jumpPrimary = (c: SemanticChange) => {
    if (c.right) onJump("r", c.right.line);
    else if (c.left) onJump("l", c.left.line);
  };

  const onListKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable)) return;
    const action = resolveSemanticKey(changes, clamped, e.key, vim, e.target === listRef.current);
    if (!action) return;
    e.preventDefault();
    e.stopPropagation();
    if (action.type === "close") onClose();
    else if (action.type === "move") setActive(action.index);
    else onJump(action.side, action.line);
  };

  return (
    <aside
      className="dv-outline dv-semantic"
      aria-label={tt("Syntax changes")}
      onKeyDown={(e) => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(); } }}
    >
      <div className="dv-outline-head">
        <b>{tt("Syntax changes")} ({changes.length})</b>
        <button className="icon-btn" aria-label={tt("Close syntax changes (Esc)")} title={tt("Close syntax changes (Esc)")} onClick={onClose}>×</button>
      </div>
      <div
        ref={listRef}
        className="dv-outline-tree dv-semantic-list"
        role="listbox"
        tabIndex={0}
        aria-label={tt("Syntax changes")}
        aria-activedescendant={clamped >= 0 ? `sem-${clamped}` : undefined}
        onKeyDown={onListKeyDown}
      >
        {changes.length === 0 && <p className="dv-outline-empty">{tt("No syntax changes.")}</p>}
        {changes.map((c, i) => {
          const kindLabel = tt(KIND_LABEL[c.kind]);
          const isCur = i === clamped;
          return (
            <div
              key={`${c.kind}-${c.name ?? "anon"}-${c.left?.start ?? "n"}-${c.right?.start ?? "n"}-${i}`}
              id={`sem-${i}`}
              role="option"
              aria-selected={isCur}
              className={`dv-semantic-row ${isCur ? "on" : ""}`}
              onClick={() => { setActive(i); jumpPrimary(c); }}
            >
              <div className="dv-semantic-main">
                <span className={`dv-semantic-kind kind-${c.kind}`}>{kindLabel}</span>
                <span className="dv-semantic-name" title={c.name ?? undefined}>{c.name ?? <i>{tt("anonymous")}</i>}</span>
              </div>
              <div className="dv-semantic-jumps">
                {c.left && (
                  <button
                    type="button"
                    className="dv-semantic-jump"
                    aria-label={tt("Jump to left line {line}", { line: c.left.line })}
                    title={tt("Jump to left line {line}", { line: c.left.line })}
                    onClick={(e) => { e.stopPropagation(); setActive(i); onJump("l", c.left!.line); }}
                  >
                    {tt("Left")} :{c.left.line}
                  </button>
                )}
                {c.right && (
                  <button
                    type="button"
                    className="dv-semantic-jump"
                    aria-label={tt("Jump to right line {line}", { line: c.right.line })}
                    title={tt("Jump to right line {line}", { line: c.right.line })}
                    onClick={(e) => { e.stopPropagation(); setActive(i); onJump("r", c.right!.line); }}
                  >
                    {tt("Right")} :{c.right.line}
                  </button>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </aside>
  );
}
