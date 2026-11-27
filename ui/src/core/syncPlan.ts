// Plans a folder sync from a comparison: which paths to copy which way and what
// to move to the trash. Pure (unit-tested); the backend only executes the ops.
import type { DirCmpResult, DirItem, SyncCheck, SyncOp } from "../api/types";
import { joinRel } from "./path";

export type Direction = "toRight" | "toLeft";
/** update: copy missing + newer · mirror: make the target identical (trash extras) · selected: the given rows. */
export type SyncMode = "update" | "mirror" | "selected";
export type RowAction = "copyRight" | "copyLeft" | "trashLeft" | "trashRight";

export interface SyncPlan {
  dir: Direction;
  mode: SyncMode;
  ops: SyncOp[];
  /** The files of the plan as the comparison saw them: checked again before anything is written, so a plan whose files changed is refused. */
  checks: SyncCheck[];
  actions: Map<string, RowAction>;
  copies: number;
  replaces: number;
  trashes: number;
  bytes: number;
}

// `rel` is always `/`-separated (from the backend); real paths use the root's separator.
const join = joinRel;
const isUnder = (rel: string, anc: string) => rel.startsWith(anc + "/");

/** The path of a compare row on one side as it is on disk (a decomposed name on macOS differs from `rel`). */
export function sidePath(root: string, it: Pick<DirItem, "rel" | "leftRel" | "rightRel">, side: "left" | "right"): string {
  return join(root, (side === "left" ? it.leftRel : it.rightRel) ?? it.rel);
}

type SyncSnapshot = Pick<DirCmpResult, "items" | "truncated" | "incompleteReasons">;

export function planSync(snapshot: SyncSnapshot, left: string, right: string, dir: Direction, mode: SyncMode, selected?: Set<string>): SyncPlan {
  const toRight = dir === "toRight";
  const srcOnly = toRight ? "leftOnly" : "rightOnly";
  const dstOnly = toRight ? "rightOnly" : "leftOnly";
  const srcNewer = toRight ? "left" : "right";
  const [srcRoot, dstRoot] = toRight ? [left, right] : [right, left];
  const plan: SyncPlan = { dir, mode, ops: [], checks: [], actions: new Map(), copies: 0, replaces: 0, trashes: 0, bytes: 0 };
  if (syncBlocked(mode, snapshot)) return plan;
  // Rows in scope: everything, or the selected rows and their descendants.
  const sel = mode === "selected" ? [...(selected ?? [])] : null;
  const inScope = (rel: string) => !sel || sel.some((s) => rel === s || isUnder(rel, s));
  let covered: string | null = null; // rel of the last whole-tree op; skip its descendants
  const byRel = new Map(snapshot.items.map((row) => [row.rel, row] as const));
  /**
   * Where a path that exists only on the source goes: under the nearest folder above it that exists on the destination,
   * in that folder's real spelling. The folders are paired by their normalized names, so a source `café/new.txt` (composed)
   * next to a destination `café/` (decomposed) lands inside that folder and not in a second, composed one.
   */
  const placeUnderDestination = (rel: string, srcRel: string): string => {
    for (let p = rel.lastIndexOf("/"); p > 0; p = rel.lastIndexOf("/", p - 1)) {
      const above = byRel.get(rel.slice(0, p));
      if (!above || !(toRight ? above.right : above.left)) continue;
      const aboveSrc = (toRight ? above.leftRel : above.rightRel) ?? above.rel;
      const aboveDst = (toRight ? above.rightRel : above.leftRel) ?? above.rel;
      // The rest of the path is the source's own spelling below that folder.
      return srcRel.startsWith(`${aboveSrc}/`) ? `${aboveDst}/${srcRel.slice(aboveSrc.length + 1)}` : srcRel;
    }
    return srcRel;
  };

  for (const it of snapshot.items) {
    if (covered && isUnder(it.rel, covered)) continue;
    covered = null;
    if (!inScope(it.rel)) continue;
    // A folder that holds something the filters left out is no whole-tree operation: copying it would take the left-out
    // files along and trashing it would trash them. Its listed children are acted on one by one (a copy makes the
    // missing folders), and the folder and what was left out stay.
    if (it.isDir && it.hasExcluded && (it.status === srcOnly || it.status === dstOnly || (it.status === "different" && it.reason === "type"))) continue;
    const src = toRight ? it.left : it.right;
    // The real spelling of the name on each side (a decomposed name on macOS differs from `rel`).
    const srcRel = (toRight ? it.leftRel : it.rightRel) ?? it.rel;
    const dstExists = !!(toRight ? it.right : it.left);
    const dstRel = (toRight ? it.rightRel : it.leftRel) ?? (dstExists ? it.rel : placeUnderDestination(it.rel, srcRel));
    // Files are checked by size and time; a folder has no stamp of its own that means anything.
    const stamp = (meta: typeof it.left) => (meta && !meta.isDir ? { size: meta.size, mtime: meta.mtime } : null);
    const copy = (replace: boolean) => {
      plan.ops.push({ op: "copy", src: join(srcRoot, srcRel), dest: join(dstRoot, dstRel) });
      // A file source is checked by size and time, a folder source must still be a folder.
      plan.checks.push(it.isDir ? { path: join(srcRoot, srcRel), stamp: null, dir: true } : { path: join(srcRoot, srcRel), stamp: stamp(src) });
      const dstMeta = toRight ? it.right : it.left;
      // A destination that was not there must still not be there (the copy would replace what appeared); a file that was
      // is checked like the source. A folder that is replaced (a type change) is not replaced at all: see below.
      if (!dstMeta) plan.checks.push({ path: join(dstRoot, dstRel), stamp: null });
      else if (!dstMeta.isDir && !it.isDir) plan.checks.push({ path: join(dstRoot, dstRel), stamp: stamp(dstMeta) });
      plan.actions.set(it.rel, toRight ? "copyRight" : "copyLeft");
      plan.copies++;
      if (replace) plan.replaces++;
      plan.bytes += src?.size ?? 0;
      if (it.isDir) covered = it.rel;
    };
    if (it.status === srcOnly) copy(false);
    else if (it.status === dstOnly) {
      if (mode === "mirror") {
        plan.ops.push({ op: "trash", path: join(dstRoot, dstRel) });
        plan.checks.push(it.isDir ? { path: join(dstRoot, dstRel), stamp: null, dir: true } : { path: join(dstRoot, dstRel), stamp: stamp(toRight ? it.right : it.left) });
        plan.actions.set(it.rel, toRight ? "trashRight" : "trashLeft");
        plan.trashes++;
        if (it.isDir) covered = it.rel;
      }
    } else if (it.status === "different") {
      // Bytes that could not be read are no difference: such a file is never replaced.
      if (it.reason === "unreadable") continue;
      if (it.isDir && it.reason !== "type") continue; // children decide
      // A folder is never replaced by a file: all it holds would go to the trash without being shown. The row stays as a
      // difference to settle by hand.
      if (it.reason === "type" && (toRight ? it.right : it.left)?.isDir) continue;
      if (mode === "update" && it.newer !== srcNewer) continue;
      copy(true);
    }
  }
  return plan;
}

/** Failed reads or ambiguous paths can make even selected copies target the wrong file. */
export function syncBlocked(mode: SyncMode, snapshot: Omit<SyncSnapshot, "items">): string | null {
  if (snapshot.incompleteReasons.length) return "The folder scan is incomplete. Sync is unavailable.";
  if (snapshot.truncated && mode !== "selected") {
    return "The comparison is incomplete, so Update and Mirror are off. Compare a subfolder instead.";
  }
  return null;
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
