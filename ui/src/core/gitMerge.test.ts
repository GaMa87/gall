import { expect, it } from "vitest";
import { composeResult, mergeThreeWay } from "./gitMerge";

it("merges one-sided changes automatically and reports overlapping edits as conflicts", () => {
  const chunks = mergeThreeWay("a\nb\nc\nd\ne\n", "a\nB\nc\nd\ne\n", "a\nb\nc\nd\nE\n");
  expect(chunks.some((chunk) => chunk.conflict)).toBe(false);
  expect(composeResult(chunks, {}).text).toBe("a\nB\nc\nd\nE\n");
  const clash = mergeThreeWay("a\nb\nc\n", "a\nours\nc\n", "a\ntheirs\nc\n");
  expect(clash.filter((chunk) => chunk.conflict)).toHaveLength(1);
  expect(composeResult(clash, {}).unresolved).toBe(1);
  expect(composeResult(clash, {}).text).toContain("<<<<<<< ours");
});

it("builds the live result from per-conflict choices", () => {
  const chunks = mergeThreeWay("a\nb\nc\nd\ne\n", "a\nO1\nc\nO2\ne\n", "a\nT1\nc\nT2\ne\n");
  const conflicts = chunks.map((chunk, index) => (chunk.conflict ? index : -1)).filter((index) => index >= 0);
  expect(conflicts).toHaveLength(2);
  const [first, second] = conflicts;
  expect(composeResult(chunks, { [first]: "ours", [second]: "theirs" })).toEqual({ text: "a\nO1\nc\nT2\ne\n", unresolved: 0 });
  expect(composeResult(chunks, { [first]: "both", [second]: "base" }).text).toBe("a\nO1\nT1\nc\nd\ne\n");
});

it("treats identical changes, added files and missing trailing newlines safely", () => {
  expect(composeResult(mergeThreeWay("a\n", "x\n", "x\n"), {}).text).toBe("x\n");
  const added = mergeThreeWay("", "one\n", "two\n");
  expect(composeResult(added, { 0: "both" }).text).toBe("one\ntwo\n");
  const noNewline = mergeThreeWay("a\nb", "a\nours", "a\ntheirs");
  expect(composeResult(noNewline, { 1: "both" }).text).toBe("a\nours\ntheirs");
});
