import { describe, expect, it } from "vitest";
import { relinkFolders, rowOfAnchor, scrollAnchor } from "./compareSession";

describe("scroll anchor", () => {
  it("holds on to a line of the right side, else of the left side, else to nothing", () => {
    expect(scrollAnchor({ left: 3, right: 5 })).toEqual({ left: false, line: 5 });
    expect(scrollAnchor({ left: 3, right: null })).toEqual({ left: true, line: 3 });
    expect(scrollAnchor({ left: null, right: null })).toBeNull();
    expect(scrollAnchor(null)).toBeNull();
  });
  const rows = [
    { left: 1, right: 1 }, { left: 2, right: null }, { left: 3, right: null }, { left: null, right: 2 }, { left: 4, right: 3 }, { left: 5, right: 4 },
  ];
  it("finds the row that shows the line, and does not depend on the window", () => {
    expect(rowOfAnchor(rows, { left: false, line: 3 })).toBe(4);
    expect(rowOfAnchor(rows, { left: true, line: 3 })).toBe(2);
    expect(rowOfAnchor(rows, { left: false, line: 2 })).toBe(3);
  });
  it("falls back to the next shown line, then the last row, then the top", () => {
    // Right line 7 is gone: the last row. Left line 0 is before everything: the first row that shows a later line.
    expect(rowOfAnchor(rows, { left: false, line: 7 })).toBe(5);
    expect(rowOfAnchor(rows, { left: true, line: 0 })).toBe(0);
    // A line that the texts no longer have, between two that are there.
    expect(rowOfAnchor([{ left: 1, right: 1 }, { left: 5, right: 5 }], { left: false, line: 3 })).toBe(1);
    expect(rowOfAnchor(rows, null)).toBe(0);
    expect(rowOfAnchor([], { left: false, line: 1 })).toBe(0);
  });
});

describe("relink of a renamed folder", () => {
  it("takes the folder pair from one file in its old and new place", () => {
    expect(relinkFolders("/home/me/old/project/src/a.txt", "/home/me/new/project/src/a.txt")).toEqual({ from: "/home/me/old", to: "/home/me/new" });
    expect(relinkFolders("/data/2023/report/a.txt", "/backup/report/a.txt")).toEqual({ from: "/data/2023", to: "/backup" });
  });
  it("handles Windows paths and a moved file name that changed nothing below", () => {
    expect(relinkFolders("C:\\Work\\old\\a.txt", "D:\\Work\\new\\a.txt")).toEqual({ from: "C:/Work/old", to: "D:/Work/new" });
  });
  it("knows no move when the paths are the same or share no ending", () => {
    expect(relinkFolders("/a/b/c.txt", "/a/b/c.txt")).toBeNull();
    expect(relinkFolders("/a/b/c.txt", "/a/b/d.txt")).toBeNull();
  });
});
