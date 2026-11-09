import { expect, it } from "vitest";
import { edgePath, layoutHistory } from "./gitGraph";
it("routes a merge through distinct lanes and rejoins its shared ancestor", () => {
  const rows = layoutHistory("merge\0left right\0main\0Merge\0User\u00002026-01-01\nleft\0base\0\0Left\0User\u00002026-01-01\nright\0base\0\0Right\0User\u00002026-01-01\nbase\0\0\0Base\0User\u00002026-01-01");
  expect(rows[0].edges.map((e) => e.to)).toEqual([0, 1]);
  expect(rows[2].lane).toBe(1);
  expect(rows[2].edges.map((e) => [e.from, e.to])).toEqual([[0, 0], [1, 1]]);
  expect(rows[3].edges.map((e) => [e.from, e.to])).toEqual([[1, 0]]);
});
it("handles an empty history", () => { expect(layoutHistory("")).toEqual([]); });
it("keeps a continuing branch color when another lane ends", () => {
  const line = (id: string, parents = "") => [id, parents, "", id, "User", "2026-01-01"].join("\0");
  const rows = layoutHistory([line("merge", "end chain"), line("end"), line("chain", "base"), line("base")].join("\n"));
  expect(rows[2].lane).toBe(1);
  expect(rows[2].color).toBe(rows[0].edges[1].color);
  expect(rows[3].color).toBe(rows[2].color);
});
it("marks branch tips so no line is drawn above a commit without a child", () => {
  const line = (id: string, parents = "") => [id, parents, "", id, "User", "2026-01-01"].join("\0");
  const rows = layoutHistory([line("tipA", "base"), line("tipB", "base"), line("base")].join("\n"));
  expect(rows.map((r) => r.tip)).toEqual([true, true, false]);
});
it("draws lane changes as one horizontal run with rounded corners and no diagonals", () => {
  expect(edgePath({ from: 1, to: 1 }, 0)).toBe("M 36 0 V 36");
  expect(edgePath({ from: 0, to: 0 }, 0)).toBe("M 18 18 V 36");
  expect(edgePath({ from: 0, to: 2 }, 0)).toBe("M 18 18 H 38 Q 54 18 54 34 V 36");
  expect(edgePath({ from: 2, to: 0 }, 0)).toBe("M 54 0 V 2 Q 54 18 38 18 H 18");
});
it("changes lanes only at a commit: leaving it or joining it", () => {
  const line = (id: string, parents = "") => [id, parents, "", id, "User", "2026-01-01"].join("\0");
  const rows = layoutHistory([line("a", "x y"), line("x", "z"), line("y", "z"), line("z", "w"), line("w")].join("\n"));
  for (const row of rows) for (const edge of row.edges) if (edge.from !== edge.to) expect(edge.from === row.lane || edge.to === row.lane).toBe(true);
});
it("keeps a branch on its own lane and color all the way into its first parent", () => {
  const line = (id: string, parents = "") => [id, parents, "", id, "User", "2026-01-01"].join("\0");
  const rows = layoutHistory([line("main1", "base"), line("topic2", "topic1"), line("topic1", "base"), line("base")].join("\n"));
  const join = rows[3].edges.find((e) => e.from !== e.to)!;
  expect(rows[1].lane).toBe(1);
  expect(rows[2].lane).toBe(1);
  expect(rows[2].edges.find((e) => e.from === 1)).toMatchObject({ to: 1, color: rows[2].color });
  expect(join).toMatchObject({ from: 1, to: 0, color: rows[2].color });
  expect(rows[3].color).toBe(rows[0].color);
  expect(join.color).not.toBe(rows[3].color);
});
it("puts short-lived branches next to the main line and long parallel ones further right", () => {
  const line = (id: string, parents = "") => [id, parents, "", id, "User", "2026-01-01"].join("\0");
  const rows = layoutHistory([line("m1", "m2"), line("long1", "long2"), line("short1", "m3"), line("m2", "m3"), line("long2", "long3"), line("m3", "m4"), line("long3", "long4"), line("m4")].join("\n"));
  expect(rows[0].lane).toBe(0);
  expect(rows[2].lane).toBe(1);
  expect(rows[1].lane).toBeGreaterThan(rows[2].lane);
  expect(rows[4].lane).toBe(rows[1].lane);
  expect(rows[6].lane).toBe(rows[1].lane);
});
it("keeps two long parallel branches to the right of every short one", () => {
  const line = (id: string, parents = "") => [id, parents, "", id, "User", "2026-01-01"].join("\0");
  const history = [line("m1", "m2"), line("pa1", "pa2"), line("pb1", "pb2"), line("m2", "m3"), line("s1", "m4"), line("pa2", "pa3"), line("pb2", "pb3"), line("m3", "m4"), line("m4", "m5"), line("pa3", "pa4"), line("pb3", "pb4"), line("m5")].join("\n");
  const rows = layoutHistory(history);
  const lane = (id: string) => rows.find((r) => r.id === id)!.lane;
  expect(lane("s1")).toBeLessThan(lane("pa1"));
  expect(lane("s1")).toBeLessThan(lane("pb1"));
});
it("puts branches merged into the main line right next to it, before unmerged branches", () => {
  const line = (id: string, parents = "") => [id, parents, "", id, "User", "2026-01-01"].join("\0");
  const rows = layoutHistory([line("merge", "m2 side"), line("tip", "m2"), line("side", "m2"), line("m2")].join("\n"));
  const lane = (id: string) => rows.find((r) => r.id === id)!.lane;
  expect(lane("merge")).toBe(0);
  expect(lane("side")).toBe(1);
  expect(lane("tip")).toBe(2);
});
it("treats a long branch with a merge as a secondary main line: its merged branch sits directly right of it", () => {
  const line = (id: string, parents = "") => [id, parents, "", id, "User", "2026-01-01"].join("\0");
  const rows = layoutHistory([line("m1", "m2"), line("s1", "m3"), line("L1", "L2 x1"), line("m2", "m3"), line("x1", "L3"), line("L2", "L3"), line("m3", "m4"), line("L3"), line("m4")].join("\n"));
  const lane = (id: string) => rows.find((r) => r.id === id)!.lane;
  expect(lane("s1")).toBeLessThan(lane("L1"));
  expect(lane("x1")).toBe(lane("L1") + 1);
});
