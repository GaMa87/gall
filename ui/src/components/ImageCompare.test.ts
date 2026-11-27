import { expect, it } from "vitest";
import { changeSides, commitSides } from "../core/gitImages";

const file = (status: string, original: string | null = null) => ({ path: "new.png", original, status });
it("compares a commit with its base", () => {
  expect(commitSides(file("M"), "c", "b")).toEqual([{ kind: "commit", id: "b", file: "new.png" }, { kind: "commit", id: "c", file: "new.png" }]);
  expect(commitSides(file("R100", "old.png"), "c", "b")[0]).toEqual({ kind: "commit", id: "b", file: "old.png" });
  expect(commitSides(file("A"), "c", "b")[0]).toEqual({ kind: "none" });
  expect(commitSides(file("M"), "c", null)[0]).toEqual({ kind: "none" });
  expect(commitSides(file("D"), "c", "b")[1]).toEqual({ kind: "none" });
});
it("compares HEAD, the index and the working file", () => {
  const change = (index: string, worktree: string, staged: boolean, original: string | null = null) => changeSides({ path: "a.png", original, index, worktree, staged });
  expect(change("M", " ", true)).toEqual([{ kind: "head", file: "a.png" }, { kind: "index", file: "a.png" }]);
  expect(change("A", " ", true)[0]).toEqual({ kind: "none" });
  expect(change("R", " ", true, "old.png")[0]).toEqual({ kind: "head", file: "old.png" });
  expect(change("D", " ", true)[1]).toEqual({ kind: "none" });
  expect(change("M", "M", false)).toEqual([{ kind: "index", file: "a.png" }, { kind: "work", file: "a.png" }]);
  expect(change("?", "?", false)[0]).toEqual({ kind: "none" });
  expect(change(" ", "D", false)[1]).toEqual({ kind: "none" });
});
