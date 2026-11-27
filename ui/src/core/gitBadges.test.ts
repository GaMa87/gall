import { describe, expect, it } from "vitest";
import { remoteTagName, badgeRefs, type KnownRef } from "./gitBadges";

const known: KnownRef[] = [
  { name: "main", fullName: "refs/heads/main", kind: "branch" },
  { name: "feature/x", fullName: "refs/heads/feature/x", kind: "branch" },
  { name: "origin/main", fullName: "refs/remotes/origin/main", kind: "remote" },
  { name: "origin/feature/x", fullName: "refs/remotes/origin/feature/x", kind: "remote" },
  { name: "v1", fullName: "refs/tags/v1", kind: "tag" },
];

describe("badgeRefs", () => {
  it("normalizes HEAD -> name to the branch plus a separate HEAD marker", () => {
    const r = badgeRefs(["HEAD -> main", "origin/main"], known);
    expect(r.head).toBe(true);
    expect(r.badges).toEqual([
      { label: "main", fullName: "refs/heads/main", kind: "branch", current: true },
      { label: "origin/main", fullName: "refs/remotes/origin/main", kind: "remote", current: false },
    ]);
  });

  it("a detached HEAD has the marker and no branch", () => {
    const r = badgeRefs(["HEAD"], known);
    expect(r).toEqual({ head: true, badges: [] });
  });

  it("shows each reference once, by full name, however Git spelled it", () => {
    const r = badgeRefs(["main", "heads/main", "refs/heads/main", "origin/main", "refs/remotes/origin/main"], known);
    expect(r.badges.map((b) => b.fullName)).toEqual(["refs/heads/main", "refs/remotes/origin/main"]);
  });

  it("keeps two refs at one tip and a slash in a branch name", () => {
    const r = badgeRefs(["feature/x", "origin/feature/x", "main"], known);
    expect(r.badges.map((b) => [b.label, b.kind])).toEqual([["feature/x", "branch"], ["origin/feature/x", "remote"], ["main", "branch"]]);
  });

  it("keeps a branch and a tag of one name, and a tag only once", () => {
    const r = badgeRefs(["tag: v1", "v1", "tag: v1"], [...known, { name: "v1", fullName: "refs/heads/v1", kind: "branch" }]);
    expect(r.badges.map((b) => [b.label, b.kind])).toEqual([["v1", "tag"], ["v1", "branch"]]);
  });

  it("keeps a remote branch named tags/v1 distinct from an identically named local branch and a tag", () => {
    const refs: KnownRef[] = [
      ...known,
      { name: "origin/tags/v1", fullName: "refs/remotes/origin/tags/v1", kind: "remote" },
      { name: "origin/tags/v1", fullName: "refs/heads/origin/tags/v1", kind: "branch" },
    ];
    const result = badgeRefs(["refs/remotes/origin/tags/v1", "refs/heads/origin/tags/v1", "tag: v1"], refs);
    expect(result.badges.map((badge) => [badge.label, badge.kind])).toEqual([
      ["origin/tags/v1", "remote"], ["origin/tags/v1", "branch"], ["v1", "tag"],
    ]);
  });

  it("preserves a local tag whose name contains a remote tag prefix even without a loaded reference list", () => {
    const refs: KnownRef[] = [...known, { name: "origin/tags/v1", fullName: "refs/tags/origin/tags/v1", kind: "tag" }];
    expect(badgeRefs(["tag: origin/tags/v1", "tag: v1"], refs).badges.map((badge) => badge.label))
      .toEqual(["origin/tags/v1", "v1"]);
    expect(badgeRefs(["tag: origin/tags/v1"], []).badges[0].label).toBe("origin/tags/v1");
  });

  it("names a saved commit and an unknown decoration without inventing a full name", () => {
    const r = badgeRefs(["refs/gall/recovery/abc", "mystery"], known);
    expect(r.badges).toEqual([
      { label: "refs/gall/recovery/abc", kind: "saved", current: false },
      { label: "mystery", kind: "other", current: false },
    ]);
  });

  it("is empty for nothing", () => {
    expect(badgeRefs([], known)).toEqual({ head: false, badges: [] });
    expect(badgeRefs([""], known)).toEqual({ head: false, badges: [] });
  });
});

it("takes a remote name under tags/ for a fetched tag only when that tag exists at the same commit", () => {
  const id = "a".repeat(40);
  const remote = { kind: "remote", name: "origin/tags/v1", id };
  expect(remoteTagName(remote, [{ kind: "tag", name: "v1", id }])).toBe("v1");
  expect(remoteTagName(remote, [])).toBeNull();
  expect(remoteTagName(remote, [{ kind: "tag", name: "v1", id: "b".repeat(40) }])).toBeNull();
  expect(remoteTagName({ kind: "branch", name: "origin/tags/v1", id }, [{ kind: "tag", name: "v1", id }])).toBeNull();
});
