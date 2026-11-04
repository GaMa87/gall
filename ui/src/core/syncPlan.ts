// Plans a folder sync from a comparison: which paths to copy which way and what
// to move to the trash. Pure (unit-tested); the backend only executes the ops.
import type { DirItem, SyncOp } from "../api/types";
import { joinRel } from "./path";

export type Direction = "toRight" | "toLeft";
/** update: copy missing + newer · mirror: make the target identical (trash extras) · selected: the given rows. */
export type SyncMode = "update" | "mirror" | "selected";
export type RowAction = "copyRight" | "copyLeft" | "trashLeft" | "trashRight";

export interface SyncPlan {
  dir: Direction;
  mode: SyncMode;
  ops: SyncOp[];
  actions: Map<string, RowAction>;
  copies: number;
  replaces: number;
  trashes: number;
  bytes: number;
}

// `rel` is always `/`-separated (from the backend); real paths use the root's separator.
const join = joinRel;
const isUnder = (rel: string, anc: string) => rel.startsWith(anc + "/");

export function planSync(items: DirItem[], left: string, right: string, dir: Direction, mode: SyncMode, selected?: Set<string>): SyncPlan {
  const toRight = dir === "toRight";
  const srcOnly = toRight ? "leftOnly" : "rightOnly";
  const dstOnly = toRight ? "rightOnly" : "leftOnly";
  const srcNewer = toRight ? "left" : "right";
  const [srcRoot, dstRoot] = toRight ? [left, right] : [right, left];
  const plan: SyncPlan = { dir, mode, ops: [], actions: new Map(), copies: 0, replaces: 0, trashes: 0, bytes: 0 };
  // Rows in scope: everything, or the selected rows and their descendants.
  const sel = mode === "selected" ? [...(selected ?? [])] : null;
  const inScope = (rel: string) => !sel || sel.some((s) => rel === s || isUnder(rel, s));
  let covered: string | null = null; // rel of the last whole-tree op; skip its descendants

  for (const it of items) {
    if (covered && isUnder(it.rel, covered)) continue;
    covered = null;
    if (!inScope(it.rel)) continue;
    const src = toRight ? it.left : it.right;
    const copy = (replace: boolean) => {
      plan.ops.push({ op: "copy", src: join(srcRoot, it.rel), dest: join(dstRoot, it.rel) });
      plan.actions.set(it.rel, toRight ? "copyRight" : "copyLeft");
      plan.copies++;
      if (replace) plan.replaces++;
      plan.bytes += src?.size ?? 0;
      if (it.isDir) covered = it.rel;
    };
    if (it.status === srcOnly) copy(false);
    else if (it.status === dstOnly) {
      if (mode === "mirror") {
        plan.ops.push({ op: "trash", path: join(dstRoot, it.rel) });
        plan.actions.set(it.rel, toRight ? "trashRight" : "trashLeft");
        plan.trashes++;
        if (it.isDir) covered = it.rel;
      }
    } else if (it.status === "different") {
      if (it.isDir && it.reason !== "type") continue; // children decide
      if (mode === "update" && it.newer !== srcNewer) continue;
      copy(true);
    }
  }
  return plan;
}

export function describePlan(p: SyncPlan): string {
  const arrow = p.dir === "toRight" ? "→" : "←";
  const parts = [`copy ${p.copies}${p.replaces ? ` (replace ${p.replaces})` : ""}`];
  if (p.trashes) parts.push(`move ${p.trashes} to trash`);
  return `${arrow} ${parts.join(", ")}`;
}

/** Rows to show: status filter + text filter; folders stay when something below matches. O(n · depth). */
export function visibleRows(items: DirItem[], show: (it: DirItem) => boolean, collapsed: Set<string>): DirItem[] {
  const keep = new Array<boolean>(items.length).fill(false);
  const index = new Map<string, number>();
  items.forEach((it, i) => index.set(it.rel, i));
  for (let i = 0; i < items.length; i++) {
    if (keep[i] || !show(items[i])) continue;
    keep[i] = true;
    // Keep every ancestor folder; stop at the first one already kept.
    for (let rel = items[i].rel, p = rel.lastIndexOf("/"); p > 0; p = rel.lastIndexOf("/")) {
      rel = rel.slice(0, p);
      const j = index.get(rel);
      if (j === undefined || keep[j]) break;
      keep[j] = true;
    }
  }
  const out: DirItem[] = [];
  let hideUnder: string | null = null;
  items.forEach((it, i) => {
    if (hideUnder && isUnder(it.rel, hideUnder)) return;
    hideUnder = null;
    if (!keep[i]) return;
    out.push(it);
    if (it.isDir && collapsed.has(it.rel)) hideUnder = it.rel;
  });
  return out;
}
