import { expect, it } from "vitest";
import { edgePath, layoutHistory, keepListed, oldestFirst, rangeIds, toggleId } from "./gitGraph";
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
  expect(edgePath({ from: 1, to: 1 }, 0)).toBe("M 36 0 V 28");
  expect(edgePath({ from: 0, to: 0 }, 0)).toBe("M 18 14 V 28");
  expect(edgePath({ from: 0, to: 2 }, 0)).toBe("M 18 14 H 42 Q 54 14 54 26 V 28");
  expect(edgePath({ from: 2, to: 0 }, 0)).toBe("M 54 0 V 2 Q 54 14 42 14 H 18");
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

it("chooses several commits by toggling, by range and after a refresh", () => {
  const rows = ["e", "d", "c", "b", "a"].map((id) => ({ id }));
  expect(rangeIds(rows, "d", "b")).toEqual(["d", "c", "b"]);
  expect(rangeIds(rows, "b", "d")).toEqual(["d", "c", "b"]);
  expect(rangeIds(rows, "b", "gone")).toEqual([]);
  expect([...toggleId(new Set(["a"]), "b")]).toEqual(["a", "b"]);
  expect([...toggleId(new Set(["a", "b"]), "a")]).toEqual(["b"]);
  // Applied from the oldest, whatever the order of choosing.
  expect(oldestFirst(rows, new Set(["d", "a", "c"]))).toEqual(["a", "c", "d"]);
  const picked = new Set(["d", "b"]);
  expect(keepListed(picked, rows)).toBe(picked);
  expect([...keepListed(picked, rows.filter((row) => row.id !== "d"))]).toEqual(["b"]);
});
it("lays a stash row out as a tip of its own with one edge to its base, and keeps a commit of the same object separate", () => {
  const line = (id: string, parents = "", refs = "", stash = "") => [id, parents, refs, id, "User", "2026-01-01", ...(stash ? [stash] : [])].join("\0");
  const rows = layoutHistory([line("tip", "base", "HEAD -> main"), line("s0", "base", "", "stash@{0}"), line("s1", "base", "", "stash@{1}"), line("base")].join("\n"));
  expect(rows.map((row) => row.stash)).toEqual([undefined, "stash@{0}", "stash@{1}", undefined]);
  expect(rows.map((row) => row.key)).toEqual(["tip", "stash:stash@{0}", "stash:stash@{1}", "base"]);
  // Each stash row is a tip (nothing above it) with a single edge going to its base.
  expect(rows[1].tip && rows[2].tip).toBe(true);
  expect(rows[1].edges.filter((e) => e.from === rows[1].lane)).toHaveLength(1);
  expect(rows[3].edges.length).toBeGreaterThan(0);
  // The same object as a branch tip and as a stash: both rows exist and neither takes the other's lane.
  const twin = layoutHistory([line("same", "base", "other"), line("same", "base", "", "stash@{0}"), line("base")].join("\n"));
  expect(twin.map((row) => row.key)).toEqual(["same", "stash:stash@{0}", "base"]);
  expect(twin[0].tip && twin[1].tip).toBe(true);
  expect(twin[1].lane).not.toBe(twin[0].lane);
});

it("reads the selector of a stash row only from the seventh field", () => {
  const rows = layoutHistory(["a\0\0\0stash@{0} in a subject\0User\u00002026-01-01", "b\0\0\0s\0User\u00002026-01-01\0stash@{3}"].join("\n"));
  expect(rows[0].stash).toBeUndefined();
  expect(rows[0].subject).toBe("stash@{0} in a subject");
  expect(rows[1].stash).toBe("stash@{3}");
});

import { inFocus, reachableFrom } from "./gitGraph";

const focusRow = (id: string, parents: string, stash = "") => [id, parents, "", id, "User", "2026-01-01", ...(stash ? [stash] : [])].join("\0");

it("reaches the tip, its ancestors and both sides of a merge, and nothing else", () => {
  const rows = layoutHistory([focusRow("m", "a b"), focusRow("x", "base"), focusRow("a", "base"), focusRow("b", "base"), focusRow("base", "")].join("\n"));
  expect([...reachableFrom(rows, "m")].sort()).toEqual(["a", "b", "base", "m"]);
  expect([...reachableFrom(rows, "x")].sort()).toEqual(["base", "x"]);
  expect([...reachableFrom(rows, "missing")]).toEqual([]);
});

it("stops at a parent that is not loaded and keeps stash rows out of every ancestry", () => {
  const rows = layoutHistory([focusRow("tip", "mid"), focusRow("s0", "base", "stash@{0}"), focusRow("mid", "gone")].join("\n"));
  expect([...reachableFrom(rows, "tip")].sort()).toEqual(["mid", "tip"]);
  // A stash whose object is also a branch tip: only the ordinary row counts.
  const twin = layoutHistory([focusRow("same", "base"), focusRow("same", "base", "stash@{0}"), focusRow("base", "")].join("\n"));
  expect([...reachableFrom(twin, "same")].sort()).toEqual(["base", "same"]);
  expect(inFocus(new Set(["same"]), twin[1].key)).toBe(false);
  expect(inFocus(new Set(["same"]), twin[0].key)).toBe(true);
});

it("gives each edge the row it belongs to, so a bright route can pass through a dim row", () => {
  // tip -> p1 -> p2 on the first-parent line; `other` is a second tip on p2 that the focused tip does not reach.
  const rows = layoutHistory([focusRow("tip", "p1"), focusRow("other", "p2"), focusRow("p1", "p2"), focusRow("p2", "")].join("\n"));
  const reached = reachableFrom(rows, "tip");
  expect([...reached].sort()).toEqual(["p1", "p2", "tip"]);
  // Row 1 is `other`: dim. The lane of tip -> p1 passes through it and belongs to `tip`: bright.
  expect(inFocus(reached, rows[1].key)).toBe(false);
  const passing = rows[1].edges.find((e) => e.owner === "tip");
  expect(passing).toBeDefined();
  expect(inFocus(reached, passing!.owner)).toBe(true);
  // The edge that leaves `other` is its own and dim.
  const leaving = rows[1].edges.find((e) => e.owner === "other");
  expect(leaving).toBeDefined();
  expect(inFocus(reached, leaving!.owner)).toBe(false);
  // At p2 the lane that arrives from p1 is the line above the node (bright); the lane of `other` joins it (dim).
  expect(rows[0].incoming).toBeUndefined();
  expect(rows[2].incoming).toBe("tip");
  expect(rows[3].incoming).toBe("p1");
  expect(inFocus(reached, rows[3].incoming!)).toBe(true);
  expect(rows[3].edges.map((e) => [e.owner, inFocus(reached, e.owner)])).toEqual([["other", false]]);
});

it("keeps the owner of a lane that a merge brought in, along its whole length", () => {
  // m merges a and b; x is a tip that only shares the base. Everything m reaches is bright, only x's lanes are dim.
  const rows = layoutHistory([focusRow("m", "a b"), focusRow("a", "base"), focusRow("b", "base"), focusRow("x", "base"), focusRow("base", "")].join("\n"));
  const reached = reachableFrom(rows, "m");
  expect([...reached].sort()).toEqual(["a", "b", "base", "m"]);
  for (const row of rows) {
    expect(inFocus(reached, row.key)).toBe(row.key !== "x");
    for (const edge of row.edges) expect(inFocus(reached, edge.owner), `${row.key}: ${edge.owner}`).toBe(edge.owner !== "x");
    if (row.incoming) expect(inFocus(reached, row.incoming), `${row.key} incoming`).toBe(row.incoming !== "x");
  }
  // The lane of b (the merge's second parent) belongs to m where it passes the row of a, and arrives at b from m.
  expect(rows[1].edges.some((e) => e.owner === "m")).toBe(true);
  expect(rows[2].incoming).toBe("m");
});
