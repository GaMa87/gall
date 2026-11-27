// Pure document block comparison for `.docx` and `.pdf`: aligns positioned blocks, detects
// layout changes (heading-level change, moved blocks across pages/sections, table row
// insertions/deletions/cell edits, block-kind changes), and formats position markers.
import type { DocBlock, DocExtract } from "../api/types";

export const DOC_EXT = new Set(["docx", "pdf"]);

export function docExt(path: string): string {
  const name = path.split(/[\\/]/).pop() ?? path;
  const dot = name.lastIndexOf(".");
  return dot >= 0 ? name.slice(dot + 1).toLowerCase() : "";
}

export function isSameDocumentKind(left: string, right: string): boolean {
  const a = docExt(left);
  const b = docExt(right);
  return DOC_EXT.has(a) && a === b;
}

export type DocLayoutChange =
  | "headingLevel"
  | "kindChanged"
  | "movedFrom"
  | "movedTo"
  | "tableRowAdded"
  | "tableRowRemoved"
  | "tableRowChanged";

export type DocRowStatus = "same" | "added" | "removed" | "changed" | "moved";

export interface DocCompareRow {
  index: number;
  status: DocRowStatus;
  left: DocBlock | null;
  right: DocBlock | null;
  layoutChange: DocLayoutChange | null;
  layoutDetail: string | null;
  moveId: number | null;
  /** Row index of the matching move partner (populated for `movedFrom` / `movedTo`). */
  movePartnerIndex: number | null;
  /** 0-based cell indices that changed in a `tableRow` comparison. */
  changedCells: number[];
}

export interface DocCompareResult {
  format: string;
  rows: DocCompareRow[];
  counts: {
    same: number;
    added: number;
    removed: number;
    changed: number;
    moved: number;
    headingChanges: number;
    tableRowChanges: number;
  };
}

/** Human-readable position badge for a document block: `p. 2 · #4` (PDF) or `§2 · #4` (DOCX). */
export function formatBlockPosition(block: DocBlock, format: string): string {
  const prefix = format === "pdf" ? `p. ${block.page}` : `§${block.page}`;
  if (block.kind === "tableRow" && block.table != null && block.tableRow != null) {
    return `${prefix} · #${block.block} · tbl ${block.table} r ${block.tableRow}`;
  }
  return `${prefix} · #${block.block}`;
}

export function formatBlockGeometry(block: DocBlock): string | null {
  const g = block.geometry;
  if (!g) return null;
  return `x=${g.x.toFixed(0)} pt, y=${g.y.toFixed(0)} pt (${g.width.toFixed(0)}×${g.height.toFixed(0)} pt)`;
}

function normText(s: string): string {
  return s.trim().replace(/\s+/g, " ");
}

function blockMatchKey(b: DocBlock): string {
  return normText(b.text);
}

/** Word-level Jaccard similarity in `[0, 1]` used to pair modified blocks within a diff hunk. */
function textSimilarity(a: string, b: string): number {
  const wa = new Set(normText(a).toLowerCase().split(/\s+/).filter(Boolean));
  const wb = new Set(normText(b).toLowerCase().split(/\s+/).filter(Boolean));
  if (wa.size === 0 && wb.size === 0) return 1;
  if (wa.size === 0 || wb.size === 0) return 0;
  let inter = 0;
  for (const w of wa) if (wb.has(w)) inter++;
  return inter / (wa.size + wb.size - inter);
}

/** Similarity for table rows: matching first (key) cell or shared cell values pair modified rows cleanly. */
function blockPairSimilarity(a: DocBlock, b: DocBlock): number {
  if (a.kind === "tableRow" && b.kind === "tableRow" && a.cells?.length && b.cells?.length) {
    const firstA = normText(a.cells[0] ?? "").toLowerCase();
    const firstB = normText(b.cells[0] ?? "").toLowerCase();
    const maxLen = Math.max(a.cells.length, b.cells.length);
    let sameCells = 0;
    for (let i = 0; i < Math.min(a.cells.length, b.cells.length); i++) {
      if (normText(a.cells[i] ?? "").toLowerCase() === normText(b.cells[i] ?? "").toLowerCase()) {
        sameCells++;
      }
    }
    const ratio = sameCells / Math.max(1, maxLen);
    if (firstA && firstA === firstB) return Math.max(0.65, ratio);
    return ratio;
  }
  return textSimilarity(a.text, b.text);
}

function diffTableCells(leftCells: string[] | null | undefined, rightCells: string[] | null | undefined): number[] {
  const a = leftCells ?? [];
  const b = rightCells ?? [];
  const n = Math.max(a.length, b.length);
  const changed: number[] = [];
  for (let i = 0; i < n; i++) {
    if (normText(a[i] ?? "") !== normText(b[i] ?? "")) changed.push(i);
  }
  return changed;
}

/**
 * Simple patience-inspired / Myers-free bounded LCS on block text keys:
 * fast for typical documents (up to several thousand blocks) and falls back to anchor matching
 * for huge documents so comparison never stalls the UI thread.
 */
function alignBlocks(left: DocBlock[], right: DocBlock[]): Array<[number | null, number | null]> {
  const n = left.length;
  const m = right.length;
  // Trim common prefix and suffix first (fast path for localized edits in huge docs).
  let p = 0;
  while (p < n && p < m && blockMatchKey(left[p]) === blockMatchKey(right[p])) p++;
  let s = 0;
  while (s < n - p && s < m - p && blockMatchKey(left[n - 1 - s]) === blockMatchKey(right[m - 1 - s])) s++;

  const pairs: Array<[number | null, number | null]> = [];
  for (let i = 0; i < p; i++) pairs.push([i, i]);

  const lm = n - p - s;
  const rm = m - p - s;
  if (lm === 0) {
    for (let j = 0; j < rm; j++) pairs.push([null, p + j]);
  } else if (rm === 0) {
    for (let i = 0; i < lm; i++) pairs.push([p + i, null]);
  } else if (lm * rm <= 4_000_000) {
    // Standard O(lm * rm) LCS DP on the differing interior slice.
    const cols = rm + 1;
    const dp = new Int32Array((lm + 1) * cols);
    for (let i = lm - 1; i >= 0; i--) {
      const lk = blockMatchKey(left[p + i]);
      const rowOff = i * cols;
      const nextOff = (i + 1) * cols;
      for (let j = rm - 1; j >= 0; j--) {
        if (lk === blockMatchKey(right[p + j])) {
          dp[rowOff + j] = dp[nextOff + j + 1] + 1;
        } else {
          const down = dp[nextOff + j];
          const rightVal = dp[rowOff + j + 1];
          dp[rowOff + j] = down >= rightVal ? down : rightVal;
        }
      }
    }
    let i = 0, j = 0;
    while (i < lm && j < rm) {
      if (blockMatchKey(left[p + i]) === blockMatchKey(right[p + j])) {
        pairs.push([p + i, p + j]);
        i++;
        j++;
      } else if (dp[(i + 1) * cols + j] >= dp[i * cols + j + 1]) {
        pairs.push([p + i, null]);
        i++;
      } else {
        pairs.push([null, p + j]);
        j++;
      }
    }
    while (i < lm) { pairs.push([p + i, null]); i++; }
    while (j < rm) { pairs.push([null, p + j]); j++; }
  } else {
    // Large-document fallback: greedy forward lookup within a bounded window.
    let i = 0, j = 0;
    const rightIndex = new Map<string, number[]>();
    for (let r = 0; r < rm; r++) {
      const k = blockMatchKey(right[p + r]);
      const list = rightIndex.get(k);
      if (list) list.push(r);
      else rightIndex.set(k, [r]);
    }
    while (i < lm && j < rm) {
      const lk = blockMatchKey(left[p + i]);
      if (lk === blockMatchKey(right[p + j])) {
        pairs.push([p + i, p + j]);
        i++;
        j++;
      } else {
        const hit = rightIndex.get(lk)?.find((idx) => idx > j && idx - j <= 64);
        if (hit != null) {
          while (j < hit) { pairs.push([null, p + j]); j++; }
          pairs.push([p + i, p + j]);
          i++;
          j++;
        } else {
          pairs.push([p + i, null]);
          i++;
        }
      }
    }
    while (i < lm) { pairs.push([p + i, null]); i++; }
    while (j < rm) { pairs.push([null, p + j]); j++; }
  }

  for (let k = 0; k < s; k++) pairs.push([n - s + k, m - s + k]);
  return pairs;
}

/**
 * Compares two extracted documents block-by-block, preserving page/section and table positions
 * and flagging layout changes (heading-level shifts, moved blocks, table-row insertions/edits).
 */
export function compareDocumentBlocks(leftDoc: DocExtract, rightDoc: DocExtract): DocCompareResult {
  const format = leftDoc.format || rightDoc.format || "docx";
  const leftBlocks = (leftDoc.items ?? []).filter((b) => b.kind !== "sectionBreak");
  const rightBlocks = (rightDoc.items ?? []).filter((b) => b.kind !== "sectionBreak");
  const raw = alignBlocks(leftBlocks, rightBlocks);

  // Step 1: Collapse adjacent removed + added runs into paired modifications when they are
  // similar in text (or matching table rows), leaving unmatched deletions/insertions for move detection.
  const rows: DocCompareRow[] = [];
  let idx = 0;
  while (idx < raw.length) {
    const [li, ri] = raw[idx];
    if (li != null && ri != null) {
      const lb = leftBlocks[li];
      const rb = rightBlocks[ri];
      let status: DocRowStatus = "same";
      let layoutChange: DocLayoutChange | null = null;
      let layoutDetail: string | null = null;
      let changedCells: number[] = [];

      if (lb.kind === "heading" && rb.kind === "heading" && lb.headingLevel !== rb.headingLevel) {
        status = "changed";
        layoutChange = "headingLevel";
        layoutDetail = `H${lb.headingLevel ?? 1} → H${rb.headingLevel ?? 1}`;
      } else if (lb.kind !== rb.kind) {
        status = "changed";
        layoutChange = "kindChanged";
        layoutDetail = `${lb.kind} → ${rb.kind}`;
      } else if (lb.kind === "tableRow" && rb.kind === "tableRow") {
        changedCells = diffTableCells(lb.cells, rb.cells);
        if (changedCells.length > 0) {
          status = "changed";
          layoutChange = "tableRowChanged";
          layoutDetail = `${changedCells.length} cell${changedCells.length === 1 ? "" : "s"}`;
        }
      }
      rows.push({
        index: rows.length,
        status,
        left: lb,
        right: rb,
        layoutChange,
        layoutDetail,
        moveId: null,
        movePartnerIndex: null,
        changedCells,
      });
      idx++;
      continue;
    }

    // Collect contiguous unmatched hunk of removals and additions.
    const removed: DocBlock[] = [];
    const added: DocBlock[] = [];
    while (idx < raw.length && (raw[idx][0] == null || raw[idx][1] == null)) {
      const [a, b] = raw[idx];
      if (a != null) removed.push(leftBlocks[a]);
      if (b != null) added.push(rightBlocks[b]);
      idx++;
    }

    // Pair similar blocks within the same hunk as in-place edits; keep dissimilar ones separate
    // so cross-document move detection can match them later.
    const usedAdded = new Set<number>();
    const pairedRemoved = new Set<number>();
    const hunkPairs: Array<[DocBlock | null, DocBlock | null]> = [];
    for (let r = 0; r < removed.length; r++) {
      let bestA = -1;
      let bestSim = 0.45;
      for (let a = 0; a < added.length; a++) {
        if (usedAdded.has(a)) continue;
        // Prefer pairing same kind (especially tableRow with tableRow).
        if ((removed[r].kind === "tableRow") !== (added[a].kind === "tableRow")) continue;
        const sim = blockPairSimilarity(removed[r], added[a]);
        if (sim >= bestSim) {
          bestSim = sim;
          bestA = a;
        }
      }
      if (bestA >= 0) {
        pairedRemoved.add(r);
        usedAdded.add(bestA);
        hunkPairs.push([removed[r], added[bestA]]);
      } else {
        hunkPairs.push([removed[r], null]);
      }
    }
    for (let a = 0; a < added.length; a++) {
      if (!usedAdded.has(a)) hunkPairs.push([null, added[a]]);
    }

    for (const [lb, rb] of hunkPairs) {
      if (lb && rb) {
        const isTable = lb.kind === "tableRow" && rb.kind === "tableRow";
        const changedCells = isTable ? diffTableCells(lb.cells, rb.cells) : [];
        let layoutChange: DocLayoutChange | null = null;
        let layoutDetail: string | null = null;
        if (isTable) {
          layoutChange = "tableRowChanged";
          layoutDetail = `${changedCells.length} cell${changedCells.length === 1 ? "" : "s"}`;
        } else if (lb.kind === "heading" && rb.kind === "heading" && lb.headingLevel !== rb.headingLevel) {
          layoutChange = "headingLevel";
          layoutDetail = `H${lb.headingLevel ?? 1} → H${rb.headingLevel ?? 1}`;
        } else if (lb.kind !== rb.kind) {
          layoutChange = "kindChanged";
          layoutDetail = `${lb.kind} → ${rb.kind}`;
        }
        rows.push({
          index: rows.length,
          status: "changed",
          left: lb,
          right: rb,
          layoutChange,
          layoutDetail,
          moveId: null,
          movePartnerIndex: null,
          changedCells,
        });
      } else if (lb) {
        rows.push({
          index: rows.length,
          status: "removed",
          left: lb,
          right: null,
          layoutChange: lb.kind === "tableRow" ? "tableRowRemoved" : null,
          layoutDetail: lb.kind === "tableRow" && lb.tableRow != null ? `row ${lb.tableRow}` : null,
          moveId: null,
          movePartnerIndex: null,
          changedCells: [],
        });
      } else if (rb) {
        rows.push({
          index: rows.length,
          status: "added",
          left: null,
          right: rb,
          layoutChange: rb.kind === "tableRow" ? "tableRowAdded" : null,
          layoutDetail: rb.kind === "tableRow" && rb.tableRow != null ? `row ${rb.tableRow}` : null,
          moveId: null,
          movePartnerIndex: null,
          changedCells: [],
        });
      }
    }
  }

  // Step 2: Detect moved blocks (removed in one place and added with identical normalized text
  // elsewhere, minimum 6 non-space chars to avoid matching trivial single tokens).
  const removedByText = new Map<string, number[]>();
  const addedByText = new Map<string, number[]>();
  for (let i = 0; i < rows.length; i++) {
    const r = rows[i];
    if (r.status === "removed" && r.left) {
      const k = blockMatchKey(r.left);
      if (k.length >= 6) {
        const list = removedByText.get(k);
        if (list) list.push(i);
        else removedByText.set(k, [i]);
      }
    } else if (r.status === "added" && r.right) {
      const k = blockMatchKey(r.right);
      if (k.length >= 6) {
        const list = addedByText.get(k);
        if (list) list.push(i);
        else addedByText.set(k, [i]);
      }
    }
  }

  let nextMoveId = 1;
  for (const [key, remIndices] of removedByText) {
    const addIndices = addedByText.get(key);
    if (!addIndices || remIndices.length !== 1 || addIndices.length !== 1) continue;
    const fromIdx = remIndices[0];
    const toIdx = addIndices[0];
    const fromRow = rows[fromIdx];
    const toRow = rows[toIdx];
    if (!fromRow.left || !toRow.right) continue;
    const id = nextMoveId++;
    const fromPos = formatBlockPosition(fromRow.left, format);
    const toPos = formatBlockPosition(toRow.right, format);
    fromRow.status = "moved";
    fromRow.layoutChange = "movedFrom";
    fromRow.layoutDetail = `→ ${toPos}`;
    fromRow.moveId = id;
    fromRow.movePartnerIndex = toIdx;

    toRow.status = "moved";
    toRow.layoutChange = "movedTo";
    toRow.layoutDetail = `← ${fromPos}`;
    toRow.moveId = id;
    toRow.movePartnerIndex = fromIdx;
  }

  const counts = {
    same: 0,
    added: 0,
    removed: 0,
    changed: 0,
    moved: nextMoveId - 1,
    headingChanges: 0,
    tableRowChanges: 0,
  };
  for (const r of rows) {
    if (r.status === "same") counts.same++;
    else if (r.status === "added") counts.added++;
    else if (r.status === "removed") counts.removed++;
    else if (r.status === "changed") counts.changed++;
    if (r.layoutChange === "headingLevel") counts.headingChanges++;
    if (
      r.layoutChange === "tableRowAdded"
      || r.layoutChange === "tableRowRemoved"
      || r.layoutChange === "tableRowChanged"
    ) {
      counts.tableRowChanges++;
    }
  }

  return { format, rows, counts };
}
