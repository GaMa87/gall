import { expect, it } from "vitest";
import { journalNodes, jumpPreview, refMove, type JournalEntry, type JournalView } from "./gitJournal";

const at = (reference: string | null, id: string) => ({ reference: reference && `refs/heads/${reference}`, id: id.repeat(40).slice(0, 40) });
const entry = (id: number, before: ReturnType<typeof at>, after: ReturnType<typeof at>, kind: JournalEntry["kind"] = "commit"): JournalEntry => ({ id, kind, label: `op ${id}`, time: id, before, after, snapshot: null });
const view = (applied: number): JournalView => ({
  entries: [entry(1, at("main", "a"), at("main", "b")), entry(2, at("main", "b"), at("main", "c"), "amend"), entry(3, at("main", "c"), at("side", "d"), "checkout"), entry(4, at("side", "d"), at(null, "e"), "checkout")],
  applied, current: null, undoBlocked: null, redoBlocked: null,
});

it("shows the timeline newest first with the state before everything at its end", () => {
  expect(journalNodes(view(2)).map((node) => [node.applied, node.state])).toEqual([[4, "undone"], [3, "undone"], [2, "current"], [1, "done"], [0, "done"]]);
  expect(journalNodes(view(0))[4]).toMatchObject({ applied: 0, entry: null, state: "current" });
  expect(journalNodes({ ...view(0), entries: [] })).toEqual([]);
});
it("names the references that move", () => {
  expect(refMove(at("main", "a"), at("main", "b"))).toBe("main aaaaaaaa → bbbbbbbb");
  expect(refMove(at("main", "a"), at("side", "a"))).toBe("main → side");
  expect(refMove(at("main", "a"), at(null, "b"))).toBe("main → HEAD bbbbbbbb");
  expect(refMove(at("main", ""), at("main", "b"))).toBe("main — → bbbbbbbb");
});
it("previews a jump with one move for each branch", () => {
  expect(jumpPreview(view(2), 0)).toEqual({ undo: 2, redo: 0, moves: ["main cccccccc → aaaaaaaa"] });
  expect(jumpPreview(view(4), 1)).toEqual({ undo: 3, redo: 0, moves: ["HEAD eeeeeeee → side", "side → main", "main cccccccc → bbbbbbbb"] });
  expect(jumpPreview(view(0), 3)).toEqual({ undo: 0, redo: 3, moves: ["main aaaaaaaa → cccccccc", "main → side"] });
  expect(jumpPreview(view(2), 2)).toEqual({ undo: 0, redo: 0, moves: [] });
});
