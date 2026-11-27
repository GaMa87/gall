import { expect, it } from "vitest";
import type { JournalView } from "./gitJournal";
import { reflogRows, searchReflog, type ReflogEntry } from "./gitReflog";

const entry = (index: number, time: number, action: string, message: string, reference = "HEAD"): ReflogEntry => ({ reference, index, old: "", id: `${index}`.repeat(40), time, action, message });
const journal: JournalView = { entries: [
  { id: 1, kind: "rebase", label: "main", time: 20, before: { reference: "refs/heads/dev", id: "e".repeat(40) }, after: { reference: "refs/heads/dev", id: "f".repeat(40) }, snapshot: null },
  { id: 2, kind: "commit", label: "first", time: 30, before: { reference: "refs/heads/dev", id: "" }, after: { reference: "refs/heads/dev", id: "a".repeat(40) }, snapshot: null },
], applied: 2, current: null, undoBlocked: null, redoBlocked: null };
const entries = [entry(0, 20, "rebase (finish)", "returning to refs/heads/dev"), entry(1, 20, "rebase (pick)", "Login form"), entry(2, 10, "commit", "Colours", "feature/x")];

it("merges the operations of Gall above their steps", () => {
  const rows = reflogRows(entries, journal);
  // The first commit of a repository has no state before it.
  expect(rows.map((row) => row.key)).toEqual(["gall 1", "HEAD@{0}", "HEAD@{1}", "feature/x@{2}"]);
  expect(rows[0]).toMatchObject({ id: "e".repeat(40), reference: "dev", action: "Rebase", message: "main", gall: true });
  expect(reflogRows(entries, null).length).toBe(3);
});
it("finds rows by all words in any column", () => {
  const rows = reflogRows(entries, journal), keys = (query: string) => searchReflog(rows, query).map((row) => row.key);
  expect(keys("")).toHaveLength(4);
  expect(keys("rebase login")).toEqual(["HEAD@{1}"]);
  expect(keys("feature/x colours")).toEqual(["feature/x@{2}"]);
  expect(keys("eeeeee")).toEqual(["gall 1"]);
  expect(keys("head@{0}")).toEqual(["HEAD@{0}"]);
  expect(keys("nothing")).toEqual([]);
});
