import { describe, expect, it } from "vitest";
import { alwaysPicks, gitPickers, hasCandidate, paletteCommands, pickCandidates, rankCandidates, rankCommands, recentCommands, rememberCommand, sensibleCandidates, type CandidateSource } from "./gitPalette";
import { gitCommandList, gitShownKey } from "./gitCommands";
import { gitActionEnabled, type GitReference } from "./gitRepository";

const ref = (kind: GitReference["kind"], name: string, id: string): GitReference => ({ name, fullName: `refs/${kind}/${name}`, kind, id, upstream: null, ahead: 0, behind: 0, upstreamGone: false });
const source: CandidateSource = {
  refs: [ref("branch", "main", "a".repeat(40)), ref("branch", "feature/login", "b".repeat(40)), ref("remote", "origin/HEAD", "a".repeat(40)), ref("remote", "origin/main", "a".repeat(40)), ref("tag", "v1.0", "c".repeat(40))],
  stashes: [{ id: "d".repeat(40), selector: "stash@{0}", message: "On main: wip invoice" }],
  remotes: [{ name: "origin", fetchUrl: "https://example.com/r.git", pushUrl: "" }],
  conflicts: ["work.txt"],
  commits: [{ id: "e1".padEnd(40, "0"), subject: "Fix the login form", author: "Ana" }, { id: "f2".padEnd(40, "0"), subject: "Add tests", author: "Bor" }],
};
const rank = (query: string, more: Partial<Parameters<typeof rankCommands>[2]> = {}) => rankCommands(paletteCommands("history"), query, { stateOf: () => "run", recent: [], current: "history", translate: (text) => text, ...more });

describe("commands", () => {
  it("lists every Git action once, with its key", () => {
    const listed = paletteCommands("history");
    const ids = listed.map((item) => item.command.id);
    expect(new Set(ids).size).toBe(ids.length);
    const actions = new Set(listed.map((item) => item.command.action));
    for (const { scope, commands } of gitCommandList()) if (scope !== "search") for (const command of commands) if (command.action !== "palette") expect(actions.has(command.action), command.id).toBe(true);
    const reword = listed.find((item) => item.command.action === "reword")!;
    expect(gitShownKey(reword.command, true)).toBe("r");
  });
  it("takes an action of several panels from the panel in use", () => {
    expect(paletteCommands("branches").find((item) => item.command.action === "checkout")?.scope).toBe("branches");
    expect(paletteCommands("history").find((item) => item.command.action === "checkout")).toMatchObject({ scope: "history", command: { label: "Checkout" } });
  });
  it("keeps the same key of two panels apart", () => {
    expect(paletteCommands("history").filter((item) => item.command.action === "panel:n").map((item) => item.scope)).toEqual(["worktrees", "tools"]);
  });
  it("can ask for the target of every action that needs a selection", () => {
    const nothing = { commit: false, head: true, stash: false, remote: false, conflict: false, branch: true, remotes: 1, operation: null, conflicts: 0 };
    for (const { command } of paletteCommands("history")) if (!gitActionEnabled(command.action, nothing)) expect(gitPickers[command.action], command.action).toBeDefined();
    expect(alwaysPicks("checkout")).toBe(true);
  });
});

describe("ranking", () => {
  it("prefers the better match and finds panel names", () => {
    expect(rank("reword")[0].command.action).toBe("reword");
    expect(rank("cherry pick current")[0].command.action).toBe("cherryPick");
    expect(rank("cherry pick another")[0].command.action).toBe("cherryPickTo");
    expect(rank("drop stash")[0].command.action).toBe("stashDrop");
    expect(rank("qqqq")).toEqual([]);
  });
  it("finds the English name in another language", () => {
    expect(rank("reword", { translate: (text) => (text === "Reword commit…" ? "Spremeni sporočilo commita …" : `x${text.length}`) })[0].command.action).toBe("reword");
  });
  it("puts recent commands first and the impossible ones last", () => {
    const merge = paletteCommands("history").find((item) => item.command.action === "merge")!;
    expect(rank("", { recent: [merge.command.id] })[0].command.action).toBe("merge");
    expect(rank("")[0].scope).toBe("history");
    const ranked = rank("reword", { stateOf: (item) => (item.command.action === "reword" ? "off" : "run") });
    expect(ranked[ranked.length - 1].command.action).toBe("reword");
  });
  it("a match beats recent use", () => {
    const fetch = paletteCommands("history").find((item) => item.command.action === "fetch")!;
    expect(rank("reword commit", { recent: [fetch.command.id] })[0].command.action).toBe("reword");
  });
  it("remembers the last commands without repeats", () => {
    for (let i = 0; i < 20; i++) rememberCommand(`c${i}`);
    rememberCommand("c15");
    expect(recentCommands()[0]).toBe("c15");
    expect(recentCommands().length).toBe(12);
    expect(new Set(recentCommands()).size).toBe(12);
  });
});

describe("targets", () => {
  it("leaves out targets that make no sense for a cherry-pick", () => {
    const current = { branch: "main", head: "a".repeat(40) };
    expect(sensibleCandidates("cherryPick", pickCandidates(gitPickers.cherryPick, source), current).map((item) => item.label)).toEqual(["feature/login", "v1.0", "Fix the login form", "Add tests"]);
    expect(sensibleCandidates("cherryPickTo", pickCandidates(gitPickers.cherryPickTo, source), current).map((item) => item.label)).toEqual(["feature/login"]);
    expect(sensibleCandidates("merge", pickCandidates(gitPickers.merge, source), current)).toHaveLength(6);
  });
  it("offers branches, remote branches and tags before commits", () => {
    expect(pickCandidates(gitPickers.checkout, source).map((item) => item.label)).toEqual(["main", "feature/login", "origin/main", "v1.0", "Fix the login form", "Add tests"]);
    expect(pickCandidates(["stash"], source)[0].stash?.selector).toBe("stash@{0}");
    expect(pickCandidates(["conflict"], source)[0].file).toBe("work.txt");
    expect(pickCandidates(["remoteRepository"], source)[0].remote?.name).toBe("origin");
  });
  it("matches names, subjects, authors and hashes", () => {
    const all = pickCandidates(gitPickers.checkout, source);
    expect(rankCandidates(all, "flog")[0].label).toBe("feature/login");
    expect(rankCandidates(all, "f200")[0].label).toBe("Add tests");
    expect(rankCandidates(all, "ana")[0].label).toBe("Fix the login form");
    expect(rankCandidates(all, "")).toHaveLength(6);
  });
  it("stays fast and short with thousands of commits", () => {
    const commits = Array.from({ length: 5000 }, (_, i) => ({ id: String(i).padStart(40, "0"), subject: `commit number ${i}`, author: "A" }));
    const all = pickCandidates(["commit"], { ...source, commits });
    const started = performance.now();
    expect(rankCandidates(all, "number 49")).toHaveLength(200);
    expect(performance.now() - started).toBeLessThan(250);
  });
  it("hasCandidate matches sensibleCandidates for every picker action", () => {
    const current = { branch: "main", head: "a".repeat(40) };
    const headOnly: CandidateSource = {
      refs: [ref("branch", "main", "a".repeat(40)), ref("remote", "origin/main", "a".repeat(40)), ref("remote", "origin/HEAD", "a".repeat(40))],
      stashes: [],
      remotes: [],
      conflicts: [],
      commits: [{ id: "a".repeat(40), subject: "head", author: "A" }],
    };
    const empty: CandidateSource = { refs: [], stashes: [], remotes: [], conflicts: [], commits: [] };
    for (const candidateSource of [source, headOnly, empty]) {
      for (const action of Object.keys(gitPickers)) {
        const expected = sensibleCandidates(action, pickCandidates(gitPickers[action], candidateSource), current).length > 0;
        expect(hasCandidate(action, candidateSource, current), action).toBe(expected);
      }
      expect(hasCandidate("unknownAction", candidateSource, current)).toBe(false);
    }
  });
});
