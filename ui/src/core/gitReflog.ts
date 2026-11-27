import { journalKinds, type JournalView } from "./gitJournal";

/** See gitreflog.rs. */
export type ReflogEntry = { reference: string; index: number; old: string; id: string; time: number; action: string; message: string };
/** A line of the list: a step of Git, or an operation of Gall, whose `id` is the state before it. */
export type ReflogRow = { key: string; id: string; time: number; reference: string; action: string; message: string; gall: boolean };

/** Git's steps and the operations of Gall in one list, newest first. */
export function reflogRows(entries: ReflogEntry[], journal: JournalView | null): ReflogRow[] {
  const rows: ReflogRow[] = entries.map((e) => ({ key: `${e.reference}@{${e.index}}`, id: e.id, time: e.time, reference: e.reference, action: e.action, message: e.message, gall: false }));
  for (const e of journal?.entries ?? []) if (e.before.id) rows.push({ key: `gall ${e.id}`, id: e.before.id, time: e.time, reference: e.before.reference?.replace(/^refs\/heads\//, "") ?? "HEAD", action: journalKinds[e.kind], message: e.label, gall: true });
  // An operation of Gall stands above the steps of Git it was made of.
  return rows.map((row, i) => ({ row, i })).sort((a, b) => b.row.time - a.row.time || Number(b.row.gall) - Number(a.row.gall) || a.i - b.i).map(({ row }) => row);
}

/** Rows that hold every word of the text, in any column. */
export function searchReflog(rows: ReflogRow[], query: string): ReflogRow[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return rows;
  return rows.filter((row) => {
    const text = `${row.reference} ${row.action} ${row.message} ${row.id} ${row.key}`.toLowerCase();
    return words.every((word) => text.includes(word));
  });
}
