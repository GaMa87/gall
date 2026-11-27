import { expect, it } from "vitest";
import { BlameCache, BlameLines, diffLineNumbers, uncommitted, type BlameCommit } from "./gitBlame";

const commit = (id: string): BlameCommit => ({ id, author: "A", time: 1, subject: id, previous: null });

it("blame lines fill in as parts arrive", () => {
  const blame = new BlameLines();
  blame.add({ commits: [commit("b")], ranges: [{ commit: "b", line: 3, count: 2, original: 7, file: "f" }] });
  expect([0, 1, 2, 3].map((i) => blame.lines[i]?.commit.id)).toEqual([undefined, undefined, "b", "b"]);
  expect(blame.lines[3]?.original).toBe(8);
  blame.add({ commits: [commit("a")], ranges: [{ commit: "a", line: 1, count: 2, original: 1, file: "old" }, { commit: "b", line: 5, count: 1, original: 9, file: "f" }] });
  expect(blame.lines.map((line) => line?.commit.id)).toEqual(["a", "a", "b", "b", "b"]);
  expect(blame.lines[0]?.file).toBe("old");
});
it("a blame of an unknown commit is left out", () => {
  const blame = new BlameLines();
  blame.add({ commits: [], ranges: [{ commit: "x", line: 1, count: 1, original: 1, file: "f" }] });
  expect(blame.lines).toEqual([]);
  expect(uncommitted(commit("0".repeat(40)))).toBe(true);
  expect(uncommitted(commit("0a"))).toBe(false);
});
it("blame of a diff belongs to the lines after the change", () => {
  expect(diffLineNumbers(["diff --git a/f b/f", "--- a/f", "+++ b/f", "@@ -1,3 +5,3 @@ x", " a", "-b", "+B", "\\ No newline at end of file", "@@ -20 +30 @@", "+z", ""])).toEqual([null, null, null, null, 4, null, 5, null, null, 29, null]);
});
it("the blame cache drops the least recently used", () => {
  const cache = new BlameCache<number>(2);
  cache.set("a", 1); cache.set("b", 2); cache.get("a"); cache.set("c", 3);
  expect([cache.get("a"), cache.get("b"), cache.get("c")]).toEqual([1, undefined, 3]);
});
