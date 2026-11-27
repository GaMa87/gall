import { expect, it } from "vitest";
import { rankCommits, touchedHunks, type AbsorbHunk } from "../core/gitAbsorb";

const hunk = (index: number, file: string, oldStart: number, oldCount: number, newStart: number, newCount: number, movable = true): AbsorbHunk => ({ index, file, oldStart, oldCount, newStart, newCount, lines: [], target: null, certain: false, reason: null, movable });
const hunks = [hunk(0, "a", 1, 1, 1, 2), hunk(1, "a", 10, 2, 11, 0), hunk(2, "b", 1, 1, 1, 1), hunk(3, "a", 30, 0, 29, 3), hunk(4, "a", 50, 1, 50, 1, false)];
const found = (file: string, range: Parameters<typeof touchedHunks>[2]) => touchedHunks(hunks, file, range).map((h) => h.index);

it("finds the staged changes that the chosen lines touch", () => {
  expect(found("a", null)).toEqual([0, 1, 3]);
  expect(found("a", { start: 2, end: 2, side: "new" })).toEqual([0]);
  expect(found("a", { start: 1, end: 30, side: "new" })).toEqual([0, 3]);
  expect(found("a", { start: 11, end: 11, side: "old" })).toEqual([1]);
  expect(found("b", { start: 5, end: 9, side: "new" })).toEqual([]);
  expect(found("c", null)).toEqual([]);
});
it("ranks the commits to choose from", () => {
  const stack = [{ id: "abc1", subject: "Login form" }, { id: "def2", subject: "Colours" }, { id: "fed3", subject: "Login test" }];
  expect(rankCommits(stack, " ")).toEqual(stack);
  expect(rankCommits(stack, "def")[0]).toBe(stack[1]);
  expect(rankCommits(stack, "login").map((c) => c.id).sort()).toEqual(["abc1", "fed3"]);
  expect(rankCommits(stack, "qqq")).toEqual([]);
});
