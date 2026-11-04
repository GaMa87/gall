import { describe, expect, it } from "vitest";
import type { OpenRule } from "../api/types";
import { findRule, ruleLabel, scopesFor } from "./openRules";

const r = (target: string, dir: string | undefined, name: string): OpenRule => ({ target, dir, name, command: name });

describe("open rules", () => {
  const rules = [
    r("ext:md", undefined, "typora"),
    r("ext:md", "/home/a/Projects", "zed"),
    r("ext:md", "/home/a/Projects/web", "code"),
    r("path:/home/a/Projects/web/NOTES.md", undefined, "vim"),
  ];
  const f = (p: string) => findRule(rules, p)?.name ?? null;
  it("picks the most specific rule, like the backend", () => {
    expect(f("/home/a/notes.md")).toBe("typora");
    expect(f("/home/a/Projects/x/README.MD")).toBe("zed");
    expect(f("/home/a/Projects/web/a/b.md")).toBe("code");
    expect(f("/home/a/Projects/web/NOTES.md")).toBe("vim");
    expect(f("/home/a/Projects-old/a.md")).toBe("typora");
    expect(f("/home/a/x.txt")).toBeNull();
  });
  it("ignores case in Windows paths only", () => {
    const win = [r("ext:md", "C:\\Users\\Ana\\Projects", "zed"), r("path:C:\\Users\\Ana\\todo.md", undefined, "vim")];
    expect(findRule(win, "c:\\users\\ana\\projects\\x\\a.md")?.name).toBe("zed");
    expect(findRule(win, "C:\\USERS\\ana\\TODO.md")?.name).toBe("vim");
    expect(f("/home/a/projects/x/a.md")).toBe("typora");
  });
  it("offers scopes from broad to narrow", () => {
    expect(scopesFor(["/p/repo/docs/a.md"], "/p/repo").map((s) => s.label)).toEqual(
      ["all .md files", ".md in project “repo”", ".md in “docs”", "only “a.md”"]);
    expect(scopesFor(["/p/x/a.md", "/p/x/b.txt"], null)).toEqual([]);
    expect(scopesFor(["/p/x/Makefile"], null).map((s) => s.label)).toEqual(["only “Makefile”"]);
  });
  it("labels rules for the settings list", () => {
    expect(ruleLabel(rules[1], "/home/a")).toBe(".md in ~/Projects");
    expect(ruleLabel(rules[3], "/home/a")).toBe("only ~/Projects/web/NOTES.md");
    expect(ruleLabel(rules[0], "/home/a")).toBe("all .md files");
  });
});
