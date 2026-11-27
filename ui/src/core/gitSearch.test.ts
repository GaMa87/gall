import { describe, expect, it } from "vitest";
import { layoutHistory } from "./gitGraph";
import { emptyQuery, hitHistory, historyQuery, nextHit, parseSearch, searchMarks, splitMatches } from "./gitSearch";

const query = (fields: Partial<ReturnType<typeof emptyQuery>>) => ({ ...emptyQuery(), ...fields });

describe("typed search", () => {
  it("is a filter of the loaded commits without a prefix", () => {
    expect(parseSearch("  ")).toEqual({ kind: "none" });
    expect(parseSearch("login form")).toEqual({ kind: "text", text: "login form", query: query({ message: "login form" }) });
    // Also a hash: an ordinary word of hexadecimal letters is looked for as both.
    expect(parseSearch("feed")).toEqual({ kind: "text", text: "feed", query: query({ message: "feed", hash: "feed" }) });
  });
  it("reads prefixes, with spaces in their values", () => {
    expect(parseSearch("a:Ana Novak")).toEqual({ kind: "history", query: query({ author: "Ana Novak" }) });
    expect(parseSearch("a:ana f:src/login.rs s:let x g:fix it")).toEqual({ kind: "history", query: query({ author: "ana", path: "src/login.rs", content: "let x", message: "fix it" }) });
    expect(parseSearch("bugs a:ana")).toEqual({ kind: "history", query: query({ message: "bugs", author: "ana" }) });
    expect(parseSearch("g:one g:two")).toEqual({ kind: "history", query: query({ message: "one two" }) });
  });
  it("takes a prefix only at the start of a word", () => {
    expect(parseSearch("data:text")).toMatchObject({ kind: "text", text: "data:text" });
    expect(parseSearch("g:data:text")).toEqual({ kind: "history", query: query({ message: "data:text" }) });
    expect(parseSearch("x:1")).toMatchObject({ kind: "text" });
  });
  it("waits for a value", () => {
    expect(parseSearch("a:")).toEqual({ kind: "none" });
    expect(parseSearch("a: f:")).toEqual({ kind: "none" });
  });
  it("asks Git only for a prefix or when no loaded commit matches", () => {
    expect(historyQuery(parseSearch("login"), 3)).toBeNull();
    expect(historyQuery(parseSearch("login"), 0)).toEqual(query({ message: "login" }));
    expect(historyQuery(parseSearch("l"), 0)).toBeNull();
    expect(historyQuery(parseSearch("a:ana"), 9)).toEqual(query({ author: "ana" }));
    expect(historyQuery(parseSearch(""), 0)).toBeNull();
  });
});

describe("matches", () => {
  it("are rows of their own without lines to commits that are not shown", () => {
    const lines = "c\0b d\0main\0Merge\0Ana\0" + "2026-01-01\n" + "a\0\0\0First\0Bor\0" + "2025-01-01\n";
    const rows = layoutHistory(hitHistory(lines));
    expect(rows.map((row) => [row.id, row.subject, row.author, row.refs.join()])).toEqual([["c", "Merge", "Ana", "main"], ["a", "First", "Bor", ""]]);
    expect(rows.every((row) => row.lane === 0 && row.edges.length === 0)).toBe(true);
  });
  it("are marked whatever their case", () => {
    expect(splitMatches("Fix Login and login", "login")).toEqual(["Fix ", "Login", " and ", "login", ""]);
    expect(splitMatches("abc", "")).toEqual(["abc"]);
    expect(splitMatches("abc", "x")).toEqual(["abc"]);
    expect(splitMatches("İstanbul fix", "fix")).toEqual(["İstanbul fix"]);
    expect(searchMarks(parseSearch("a:ana g:fix"))).toEqual({ subject: "fix", author: "ana" });
    expect(searchMarks(parseSearch("s:needle"))).toEqual({ subject: "", author: "" });
  });
  it("go around with n and N", () => {
    expect(nextHit(-1, 3, 1)).toBe(0);
    expect(nextHit(-1, 3, -1)).toBe(2);
    expect(nextHit(2, 3, 1)).toBe(0);
    expect(nextHit(0, 3, -1)).toBe(2);
    expect(nextHit(0, 0, 1)).toBe(-1);
  });
});
