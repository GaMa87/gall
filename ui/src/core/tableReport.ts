// CSV report of table changes (export from the table compare view). Pure, unit-tested.
import type { SheetDiff } from "../api/types";

const csvCell = (s: string) => (/[",\n;]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s);

/** Report of every change: one line per changed cell, added or removed row. */
export function changeReport(s: SheetDiff): string {
  const lines = [["sheet", "change", s.key ?? "row", "column", "left", "right"].join(",")];
  const keyCol = s.key ? s.columns.findIndex((c) => c.name === s.key) : -1;
  const val = (rows: string[][], i: number | null, c: number | null) => (i == null || c == null ? "" : rows[i]?.[c] ?? "");
  for (const r of s.rows) {
    if (r.status === "same") continue;
    const id = keyCol >= 0 ? (r.left != null ? val(s.leftRows, r.left, s.columns[keyCol].left) : val(s.rightRows, r.right, s.columns[keyCol].right)) : String((r.right ?? r.left ?? 0) + 1);
    if (r.status === "changed") {
      for (const k of r.cells) lines.push([s.name, "changed", id, s.columns[k].name, val(s.leftRows, r.left, s.columns[k].left), val(s.rightRows, r.right, s.columns[k].right)].map(csvCell).join(","));
    } else {
      const rows = r.status === "added" ? s.rightRows : s.leftRows;
      const i = r.status === "added" ? r.right : r.left;
      const text = s.columns.map((c) => val(rows, i, r.status === "added" ? c.right : c.left)).join(" | ");
      lines.push([s.name, r.status, id, "", r.status === "removed" ? text : "", r.status === "added" ? text : ""].map(csvCell).join(","));
    }
  }
  return lines.join("\n") + "\n";
}
