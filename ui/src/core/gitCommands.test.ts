import { describe, expect, it } from "vitest";
import { GIT_RESERVED, GIT_SCOPES, contextHints, gitCommandForKey, gitCommandList, gitCommands, gitDefaultKey, gitHints, gitKeyCap, gitKeyConflicts, gitKeyMeanings, gitKeyOf, gitPanelKey, isGitChord, parseGitKey, type GitContext } from "./gitCommands";
import { buildKeymap } from "./keymap";

const event = (key: string, more: Partial<{ ctrlKey: boolean; metaKey: boolean; altKey: boolean; shiftKey: boolean; code: string }> = {}) => ({ key, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...more });

describe("registry", () => {
  it("has no two commands on one default key within a panel", () => {
    expect(gitKeyConflicts()).toEqual([]);
  });
  it("gives every command a name of its own", () => {
    const ids = gitCommandList().flatMap(({ commands }) => commands.map((command) => command.id));
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids.every((id) => /^git\.[a-z]+\.\S+$/.test(id))).toBe(true);
  });
  // The keys follow lazygit, where a panel gives a letter its own meaning. A key that is not listed here means the same everywhere;
  // a new difference fails, so it is a decision and not an accident.
  // `D` shows the change of each revision of a file in the commit details and means another thing in another panel; `[` and
  // `]` step the revisions there and the hunks in the diff of the changes and are listed once (a decision of WN-143).
  // `A` amends in both panels that have it, as in lazygit: the last commit in Files, the selected commit in Commits (a decision of WN-126).
  it("lists every key that means different things in different panels", () => {
    expect([...gitKeyMeanings().keys()].sort()).toEqual([" ", "/", "A", "D", "Enter", "P", "R", "a", "b", "c", "d", "e", "f", "g", "i", "n", "p", "r", "s", "t", "u", "v", "w", "y"]);
    for (const key of ["W", "N", "M", "m", "x", "U", "Ctrl+r", "o", "Y", "h", "Ctrl+e", "Ctrl+j", "Ctrl+k", "z", "H", "C", "V", "T"]) expect(gitKeyMeanings().has(key), key).toBe(false);
  });
  it("puts destructive commands of every panel on d or behind a capital", () => {
    for (const scope of GIT_SCOPES) for (const command of gitCommands(scope)) if (command.danger && !command.defaults.some((key) => key.startsWith("Ctrl+"))) expect(command.defaults[0], `${scope} ${command.action}`).toMatch(/^(?:d|[A-Z])$/);
    expect(gitCommandForKey("branches", "Z")).toMatchObject({ action: "discardWip", danger: true });
  });
  it("advertises only commands that exist", () => {
    for (const context of Object.keys(contextHints) as GitContext[]) for (const [key] of contextHints[context]) if (!GIT_RESERVED.includes(key)) expect(gitCommandForKey(context, key), `${context} ${key}`).toBeDefined();
  });
});

describe("pressed and written keys", () => {
  it("names keys by what they type", () => {
    expect(gitKeyOf(event("W", { shiftKey: true }))).toBe("W");
    expect(gitKeyOf(event("?", { shiftKey: true }))).toBe("?");
    expect(gitKeyOf(event("J", { ctrlKey: true, shiftKey: true }))).toBe("Ctrl+Shift+j");
    expect(gitKeyOf(event("e", { metaKey: true }))).toBe("Ctrl+e");
    expect(gitKeyOf(event("F6", { shiftKey: true }))).toBe("Shift+F6");
    expect(gitKeyOf(event("ArrowDown", { altKey: true }))).toBe("Alt+ArrowDown");
    // AltGr on Windows arrives as Ctrl+Alt and types a character.
    expect(gitKeyOf(event("[", { ctrlKey: true, altKey: true, code: "KeyF" }))).toBe("[");
  });
  it("reads what is written in Settings as the same names", () => {
    expect(parseGitKey("Space")).toBe(" ");
    expect(parseGitKey("ctrl + J")).toBe("Ctrl+j");
    expect(parseGitKey("Cmd+Shift+g")).toBe("Ctrl+Shift+g");
    expect(parseGitKey("Shift+w")).toBe("W");
    expect(parseGitKey("f6")).toBe("F6");
    expect(parseGitKey("+")).toBe("+");
    expect(parseGitKey("Ctrl++")).toBe("Ctrl++");
    expect(["Ctrl+g", "F6", "Shift+F2", "Alt+x"].every(isGitChord)).toBe(true);
    expect(["g", "W", " ", "+", "Enter"].some(isGitChord)).toBe(false);
  });
});

describe("remapping", () => {
  const shortcuts = { "git.history.reword": "z, Ctrl+G", "git.common.fetch": "F", "git.diff.x": "o" };
  it("runs the command on the new keys only", () => {
    expect(gitCommandForKey("history", "z", shortcuts)?.action).toBe("reword");
    expect(gitCommandForKey("history", "Ctrl+g", shortcuts)?.action).toBe("reword");
    expect(gitCommandForKey("history", "r", shortcuts)).toBeUndefined();
    // Another panel keeps its own meaning of the key.
    expect(gitCommandForKey("branches", "r", shortcuts)?.action).toBe("rebase");
  });
  it("changes a shared command in every panel, and gives it back where its old key was taken", () => {
    expect(gitCommandForKey("history", "f", shortcuts)?.action).toBe("quick:fixup");
    for (const scope of ["history", "branches", "details"] as GitContext[]) expect(gitCommandForKey(scope, "F", shortcuts)?.action).toBe("fetch");
  });
  it("lets a chosen key win over a default and leaves that command without a key", () => {
    const own = { "git.history.reword": "s" };
    expect(gitCommandForKey("history", "s", own)?.action).toBe("reword");
    expect(gitCommands("history", own).find((c) => c.action === "quick:squash")?.key).toBe("");
    expect(gitHints("history", own).map((hint) => hint.label)).not.toContain("squash");
    expect(gitHints("history", own)).toContainEqual({ key: "s", label: "reword" });
  });
  it("never takes the navigation keys", () => {
    for (const key of GIT_RESERVED) expect(gitCommandForKey("history", key, { "git.history.reword": key })?.action).not.toBe("reword");
  });
  it("translates for panels that know only default keys", () => {
    expect(gitDefaultKey("diff", "o", shortcuts)).toBe("x");
    expect(gitDefaultKey("diff", "x", shortcuts)).toBe("");
    expect(gitDefaultKey("diff", "j", shortcuts)).toBe("j");
    expect(gitDefaultKey("diff", "Enter", shortcuts)).toBe("Enter");
    expect(gitPanelKey(["diff", "files"], event("a"), {}, true)).toEqual({ key: "a", on: true });
    expect(gitPanelKey(["diff", "files"], event("x"), shortcuts, true)).toEqual({ key: "x", on: true });
  });
  it("keeps letters for typing without Vim keys but lets a chosen chord run", () => {
    expect(gitPanelKey(["files"], event("d"), {}, false)).toEqual({ key: "d", on: false });
    expect(gitPanelKey(["merge"], event("p"), {}, true)).toEqual({ key: "p", on: true });
    expect(gitPanelKey(["merge"], event("p"), {}, false)).toEqual({ key: "p", on: false });
    expect(gitPanelKey(["merge"], event("g"), { "git.merge.p": "g" }, true)).toEqual({ key: "p", on: true });
    expect(gitCommandForKey("merge", "p")?.label).toBe("Propose a resolution (local model)…");
    expect(gitPanelKey(["files"], event("d"), { "git.files.d": "Ctrl+D" }, false)).toEqual({ key: "", on: false });
    expect(gitPanelKey(["files"], event("d", { ctrlKey: true }), { "git.files.d": "Ctrl+D" }, false)).toEqual({ key: "d", on: true });
    expect(gitKeyCap("files", "d", {}, false)).toBe("");
    expect(gitKeyCap("files", "d", { "git.files.d": "Ctrl+D" }, false)).toBe("Ctrl+d");
    expect(gitKeyCap("files", " ", {}, true)).toBe("Space");
    expect(gitKeyCap("history", "r", shortcuts, true)).toBe("z");
  });
  it("opens the palette with its chord without Vim keys, but not with its letter", () => {
    expect(gitPanelKey(["history"], event("P", { ctrlKey: true, shiftKey: true }), {}, false)).toMatchObject({ on: true });
    expect(gitPanelKey(["history"], event(":", { shiftKey: true }), {}, false).on).toBe(false);
    expect(gitCommandForKey("history", "Ctrl+Shift+p")?.action).toBe("palette");
  });
  it("leaves the file list alone", () => {
    expect(buildKeymap(shortcuts, "vim")).toEqual(buildKeymap({}, "vim"));
  });
});
