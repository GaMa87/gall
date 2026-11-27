import { expect, it } from "vitest";
import { amendMessage, autosquash, autosquashTarget, combinedMessage, execProblem, listWindow, sameOrder, squashRange } from "../core/gitRebase";

const commit = (id: string, subject: string, message = subject) => ({ id, subject, message });

it("finds the commit a fixup, squash or amend subject points at, however often prefixed", () => {
  expect(autosquashTarget("fixup! add alpha")).toEqual({ kind: "fixup", target: "add alpha" });
  expect(autosquashTarget("squash! add alpha")).toEqual({ kind: "squash", target: "add alpha" });
  expect(autosquashTarget("fixup! fixup! squash! add alpha")).toEqual({ kind: "fixup", target: "add alpha" });
  expect(autosquashTarget("amend! add alpha")?.kind).toBe("amend");
  expect(autosquashTarget("add alpha")).toBeNull();
  expect(autosquashTarget("fixup! ")).toBeNull();
});

it("moves fixup and squash commits behind their targets and leaves the rest in place", () => {
  const commits = [commit("a1", "add alpha"), commit("b2", "add beta"), commit("c3", "fixup! add alpha"), commit("d4", "squash! add beta"), commit("e5", "fixup! fixup! add alpha"), commit("f6", "unrelated")];
  const found = autosquash(commits)!;
  expect(found.order).toEqual(["a1", "c3", "e5", "b2", "d4", "f6"]);
  expect(found.actions).toEqual({ c3: "fixup", d4: "squash", e5: "fixup" });
  expect(found.count).toBe(3);
  expect(sameOrder(commits, found.order)).toBe(false);
});

it("folds an amend! commit and rewords its target with the latest one", () => {
  const found = autosquash([commit("a1", "add alpha"), commit("b2", "amend! add alpha"), commit("c3", "other"), commit("d4", "amend! add alpha")])!;
  expect(found.order).toEqual(["a1", "b2", "d4", "c3"]);
  expect(found.actions).toEqual({ b2: "fixup", d4: "fixup" });
  expect(found.rewords).toEqual({ a1: "d4" });
  expect(amendMessage("amend! add alpha\n\nAdd alpha at last\n\nbody\n")).toBe("Add alpha at last\n\nbody");
  expect(amendMessage("amend! add alpha")).toBe("");
});

it("draws only a part of a long list and leaves room for the rest", () => {
  expect(listWindow(200, 5000, 800, 80)).toEqual({ first: 0, last: 200, above: 0, below: 0 });
  expect(listWindow(5000, 0, 800, 80)).toEqual({ first: 0, last: 70, above: 0, below: 4930 * 80 });
  const middle = listWindow(5000, 160_000, 800, 80);
  expect(middle).toEqual({ first: 1970, last: 2040, above: 1970 * 80, below: 2960 * 80 });
  expect(listWindow(5000, 999_999, 800, 80).last).toBe(5000);
});

it("offers nothing without a target, and matches a commit ID prefix as Git does", () => {
  expect(autosquash([commit("a1", "add alpha"), commit("b2", "fixup! something else")])).toBeNull();
  expect(autosquash([commit("a1", "add alpha"), commit("b2", "plain")])).toBeNull();
  // A fixup that already follows its target changes only its action.
  const next = autosquash([commit("abc123", "add alpha"), commit("d4", "fixup! abc12")])!;
  expect(next.order).toEqual(["abc123", "d4"]);
  expect(next.actions).toEqual({ d4: "fixup" });
  expect(sameOrder([commit("abc123", "x"), commit("d4", "y")], next.order)).toBe(true);
  // A target must be an earlier commit: a fixup of a later commit stays.
  expect(autosquash([commit("a1", "fixup! add beta"), commit("b2", "add beta")])).toBeNull();
});

it("squashes one run of commits into the oldest and puts the message on the last", () => {
  const commits = [commit("a1", "one"), commit("b2", "two", "two\n\nbody"), commit("c3", "three"), commit("d4", "four")];
  const plan = squashRange(commits, new Set(["b2", "c3", "d4"]), "combined")!;
  expect(plan.first).toBe("b2");
  expect(plan.entries.map((entry) => `${entry.id}:${entry.action}`)).toEqual(["a1:pick", "b2:pick", "c3:squash", "d4:squash"]);
  expect(plan.entries.map((entry) => entry.message)).toEqual(["", "", "", "combined"]);
  expect(combinedMessage(commits, new Set(["b2", "c3"]))).toBe("two\n\nbody\n\nthree");
});

it("refuses a squash of commits that are not one run or not in the plan", () => {
  const commits = [commit("a1", "one"), commit("b2", "two"), commit("c3", "three")];
  expect(squashRange(commits, new Set(["a1", "c3"]), "x")).toBeNull();
  expect(squashRange(commits, new Set(["a1"]), "x")).toBeNull();
  expect(squashRange(commits, new Set(["a1", "zz"]), "x")).toBeNull();
});

it("accepts one-line exec commands only", () => {
  expect(execProblem("cargo test")).toBeNull();
  expect(execProblem("")).not.toBeNull();
  expect(execProblem("   ")).not.toBeNull();
  expect(execProblem("make\nrm -rf x")).not.toBeNull();
  expect(execProblem("make\rx")).not.toBeNull();
  expect(execProblem("a".repeat(5000))).not.toBeNull();
});
