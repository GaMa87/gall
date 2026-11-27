import { expect, it } from "vitest";
import { patchFiles, searchStashes, stashName, type StashInfo } from "./gitStash";

const stash = (n: number, message: string) => ({ id: `${n}`.repeat(40), selector: `stash@{${n}}`, message });

it("reads the branch and name of a stash", () => {
  expect(stashName("On main: login: half done")).toEqual({ branch: "main", name: "login: half done", named: true });
  expect(stashName("WIP on feature/x: 1a2b3c4 Fix the thing")).toEqual({ branch: "feature/x", name: "Fix the thing", named: false });
  expect(stashName("On (no branch): x")).toEqual({ branch: null, name: "x", named: true });
  expect(stashName("something else")).toEqual({ branch: null, name: "something else", named: true });
});
it("finds stashes by name and by file", () => {
  const stashes = [stash(0, "On main: colours"), stash(1, "On main: login form"), stash(2, "WIP on dev: 1a2b3c4 other")];
  const infos = new Map<string, StashInfo>([[stashes[2].id, { id: stashes[2].id, time: 1, files: ["src/login.ts", "README.md"] }]]);
  expect(searchStashes(stashes, infos, "").map((row) => row.stash)).toEqual(stashes);
  expect(searchStashes(stashes, infos, "colours").map((row) => row.stash)).toEqual([stashes[0]]);
  const found = searchStashes(stashes, infos, "login");
  expect(found.map((row) => row.stash).sort((a, b) => a.selector.localeCompare(b.selector))).toEqual([stashes[1], stashes[2]]);
  expect(found.find((row) => row.stash === stashes[2])?.file).toBe("src/login.ts");
  expect(found.find((row) => row.stash === stashes[1])?.file).toBeNull();
  expect(searchStashes(stashes, infos, "zzzzqq")).toEqual([]);
});
it("lists the files of a patch", () => {
  const patch = ["diff --git a/a.txt b/a.txt", "index 1..2 100644", "--- a/a.txt", "+++ b/a.txt", "@@ -1,2 +1,2 @@", "-- old", "+++ new", " same", "diff --git a/gone.txt b/gone.txt", "deleted file mode 100644", "--- a/gone.txt", "+++ /dev/null", "@@ -1 +0,0 @@", "-x", "diff --git a/old b/new name", "similarity index 100%", "rename from old", "rename to new name", "diff --git a/p.png b/p.png", "Binary files a/p.png and b/p.png differ", ""];
  expect(patchFiles(patch)).toEqual([
    { path: "a.txt", added: 1, deleted: 1, line: 0 }, { path: "gone.txt", added: 0, deleted: 1, line: 8 },
    { path: "new name", added: 0, deleted: 0, line: 14 }, { path: "p.png", added: 0, deleted: 0, line: 18 },
  ]);
  expect(patchFiles([""])).toEqual([]);
});
