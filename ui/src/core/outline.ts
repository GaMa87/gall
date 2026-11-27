// Logic of the structural outline beside the comparison: the flat list of visible rows of the tree, keyboard
// movement (the keys of an ARIA tree), the link from a node to the changes that lie inside it, and the rules of
// presentation-only suppression of generated files. Pure, so it is unit-tested; the view only renders.
import type { DiffOp, GeneratedReason, OutlineNode, SemanticVerdict } from "../api/types";
import { tt } from "../i18n";

/** A visible row of the outline: the node, its path id ("0.2.1"), depth and whether it can open. */
export interface OutlineRow { id: string; node: OutlineNode; depth: number; hasChildren: boolean; open: boolean; /** Position among its siblings (1-based) and their number, for `aria-posinset` and `aria-setsize`. */ position: number; siblings: number }

/** The rows that are visible when the nodes whose ids are in `expanded` are open. */
export function visibleRows(nodes: OutlineNode[], expanded: Set<string>): OutlineRow[] {
  const rows: OutlineRow[] = [];
  const walk = (list: OutlineNode[], prefix: string, depth: number) => {
    list.forEach((node, i) => {
      const id = prefix === "" ? String(i) : `${prefix}.${i}`;
      const hasChildren = node.children.length > 0;
      const open = hasChildren && expanded.has(id);
      rows.push({ id, node, depth, hasChildren, open, position: i + 1, siblings: list.length });
      if (open) walk(node.children, id, depth + 1);
    });
  };
  walk(nodes, "", 0);
  return rows;
}

/** The ids of every node that has children (to open everything at once). */
export function parentIds(nodes: OutlineNode[]): string[] {
  const ids: string[] = [];
  const walk = (list: OutlineNode[], prefix: string) => list.forEach((node, i) => {
    const id = prefix === "" ? String(i) : `${prefix}.${i}`;
    if (node.children.length) { ids.push(id); walk(node.children, id); }
  });
  walk(nodes, "");
  return ids;
}

export interface OutlineState { active: string | null; expanded: Set<string> }
export type OutlineKey = "ArrowDown" | "ArrowUp" | "ArrowRight" | "ArrowLeft" | "Home" | "End" | "Enter" | " ";

/**
 * One key of the ARIA tree pattern: Down and Up move, Right opens a closed node or goes to its first child, Left
 * closes an open node or goes to its parent, Home and End go to the ends. `activate` is set for Enter and Space
 * (the view then jumps to the node).
 */
export function pressKey(nodes: OutlineNode[], state: OutlineState, key: OutlineKey): { state: OutlineState; activate: boolean } {
  const rows = visibleRows(nodes, state.expanded);
  if (!rows.length) return { state, activate: false };
  const at = Math.max(0, rows.findIndex((r) => r.id === state.active));
  const row = rows[at];
  const go = (i: number): OutlineState => ({ ...state, active: rows[Math.min(rows.length - 1, Math.max(0, i))].id });
  switch (key) {
    case "ArrowDown": return { state: go(at + 1), activate: false };
    case "ArrowUp": return { state: go(at - 1), activate: false };
    case "Home": return { state: go(0), activate: false };
    case "End": return { state: go(rows.length - 1), activate: false };
    case "ArrowRight":
      if (row.hasChildren && !row.open) return { state: { ...state, active: row.id, expanded: new Set(state.expanded).add(row.id) }, activate: false };
      if (row.hasChildren) return { state: go(at + 1), activate: false };
      return { state, activate: false };
    case "ArrowLeft": {
      if (row.open) { const e = new Set(state.expanded); e.delete(row.id); return { state: { ...state, active: row.id, expanded: e }, activate: false }; }
      const parent = row.id.includes(".") ? row.id.slice(0, row.id.lastIndexOf(".")) : null;
      return { state: parent ? { ...state, active: parent } : { ...state, active: row.id }, activate: false };
    }
    case "Enter":
    case " ":
      return { state: { ...state, active: row.id }, activate: true };
  }
}

/** Indices (in reading order) of the changed blocks that touch a node, on one side of the comparison. */
export function blocksIn(node: OutlineNode, ops: DiffOp[], side: "a" | "b"): number[] {
  const first = node.span.line - 1, end = node.endLine; // 0-based lines [first, end)
  const out: number[] = [];
  let block = 0;
  for (const op of ops) {
    if (op.tag === "equal" || op.cosmetic) continue;
    const start = side === "a" ? op.a : op.b, len = side === "a" ? op.aLen : op.bLen;
    // An insertion or deletion has no lines on one side: it lies at the position it sits at.
    if (len === 0 ? start > first && start < end : start < end && start + len > first) out.push(block);
    block++;
  }
  return out;
}

/** The number of changed blocks in a node, counting each block once even when its children also hold it. */
export function changeCount(node: OutlineNode, ops: DiffOp[], side: "a" | "b"): number {
  return blocksIn(node, ops, side).length;
}

/** The ids of the nodes that hold a change, with their ancestors, so that the outline can open to the changes. */
export function idsWithChanges(nodes: OutlineNode[], ops: DiffOp[], side: "a" | "b"): Set<string> {
  const ids = new Set<string>();
  const walk = (list: OutlineNode[], prefix: string): boolean => {
    let any = false;
    list.forEach((node, i) => {
      const id = prefix === "" ? String(i) : `${prefix}.${i}`;
      const inside = walk(node.children, id) || blocksIn(node, ops, side).length > 0;
      if (inside) { ids.add(id); any = true; }
    });
    return any;
  };
  walk(nodes, "");
  return ids;
}

/** Text of a row for the list and for screen readers: kind, name (or "anonymous") and the number of changes. */
export function rowLabel(node: OutlineNode, changes: number): string {
  const name = node.name ?? tt("anonymous");
  if (changes <= 0) return tt("{kind} {name}", { kind: node.kind, name });
  return tt(changes === 1 ? "{kind} {name}, {count} change" : "{kind} {name}, {count} changes", { kind: node.kind, name, count: changes });
}

/** The reasons for collapsing a generated file, in words. */
export const GENERATED_REASON: Record<GeneratedReason, string> = {
  lockfile: "lockfile",
  artifactPath: "build output folder",
  generatedHeader: "generated header",
  minifiedBundle: "minified bundle",
  gitAttributes: "marked in .gitattributes",
  customPattern: "matching pattern",
};

/** Explains why the outline could not be read, naming the exact side and line on a parse error. */
export function outlineErrorMessage(err: SemanticVerdict): string {
  if (err.verdict === "unknown") {
    if (err.reason.why === "parseError") {
      return tt("Could not read the outline of the {side} side: parse error on line {line}.", {
        side: tt(err.reason.side),
        line: err.reason.line,
      });
    }
    if (err.reason.why === "unsupported") return tt("No outline for this language.");
    if (err.reason.why === "tooLarge") return tt("The outline is not available for large files.");
  }
  return tt("No outline for this file (unsupported language, a parse error or too large).");
}

/**
 * How a key press in `DiffView` behaves while a generated file body is collapsed:
 * - `"notify"` for change / move jumps (`n`, `N`, `F8`, `[`, `]`, `Alt+↑/↓`, `x`, `X`) so nothing moves unseen;
 * - `"ignore"` for direct body scroll keys (`j`, `k`, arrows, `PageUp`/`PageDown`, `Space`, `Ctrl+d/u`, `g`, `G`, `Home`, `End`);
 * - `"allow"` for all non-body actions (`H`, `y`, `p`, `o`, `O`, `S`, `E`, options, `Esc`, etc.).
 */
export function collapsedKeyBehavior(key: string, ctrlKey: boolean, altKey: boolean): "allow" | "notify" | "ignore" {
  if (key === "n" || key === "N" || key === "F8" || key === "]" || key === "[" || key === "x" || key === "X"
    || (altKey && (key === "ArrowDown" || key === "ArrowUp"))) {
    return "notify";
  }
  if (key === "j" || key === "k" || key === "ArrowDown" || key === "ArrowUp"
    || key === "PageDown" || key === "PageUp" || key === " "
    || (ctrlKey && (key === "d" || key === "u"))
    || key === "g" || key === "G" || key === "Home" || key === "End") {
    return "ignore";
  }
  return "allow";
}

/**
 * Whether the comparison body is collapsed: a file classified as generated is, until the user reveals it for this
 * view. The counts of the changes and everything that acts on them (patch, stage, merge, sync) do not depend on it.
 */
export function suppressed(generated: GeneratedReason | null | undefined, revealed: boolean): boolean {
  return !!generated && !revealed;
}

/** A row of the Git changes list as the collapse sees it. */
export interface ChangeRow { path: string; staged: boolean }

/**
 * Which rows of the Git changes list are drawn collapsed: a file classified as generated, in a list (staged or
 * unstaged) whose generated files have not been revealed. Collapsing is presentation only: every row stays in the list,
 * counts include it, and staging, unstaging, discarding and the patch act on it exactly as when it is shown.
 */
export function collapsedRows(rows: ChangeRow[], generated: ReadonlyMap<string, GeneratedReason>, revealed: { staged: boolean; unstaged: boolean }): boolean[] {
  return rows.map((row) => generated.has(row.path) && !(row.staged ? revealed.staged : revealed.unstaged));
}

/** The generated files of one list: how many there are and the distinct reasons. */
export function generatedSummary(rows: ChangeRow[], generated: ReadonlyMap<string, GeneratedReason>, staged: boolean): { count: number; reasons: GeneratedReason[] } {
  const found = rows.filter((row) => row.staged === staged && generated.has(row.path)).map((row) => generated.get(row.path)!);
  return { count: found.length, reasons: [...new Set(found)] };
}

/** The row an arrow key moves to: the next (or previous) one that is drawn; the same row when there is none. */
export function stepVisible(collapsed: boolean[], from: number, direction: 1 | -1): number {
  for (let i = from + direction; i >= 0 && i < collapsed.length; i += direction) if (!collapsed[i]) return i;
  return from;
}
