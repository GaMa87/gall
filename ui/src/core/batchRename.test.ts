import { describe, expect, it } from "vitest";
import { DEFAULT_RULES, namesFromText, newNames, renameIssues, type RenameItem, type RenameRules } from "./batchRename";

const item = (name: string, isDir = false, modified: number | null = new Date(2026, 10, 3, 9, 5, 7).getTime()): RenameItem =>
  ({ name, isDir, modified, folder: "Holiday" });
const run = (names: string[], r: Partial<RenameRules>) => newNames(names.map((n) => item(n)), { ...DEFAULT_RULES, ...r });

describe("batch rename names", () => {
  it("keeps names with the default rules", () => {
    expect(run(["a.jpg", "b.png"], {}).names).toEqual(["a.jpg", "b.png"]);
  });

  it("expands template tokens and keeps the extension", () => {
    expect(run(["IMG_1.jpg", "IMG_2.JPG"], { template: "{folder} {n:3}" }).names).toEqual(["Holiday 001.jpg", "Holiday 002.JPG"]);
    expect(run(["a.jpg", "b.jpg"], { template: "{n}-{name}", start: 10, step: 5 }).names).toEqual(["10-a.jpg", "15-b.jpg"]);
    expect(run(["a.jpg"], { template: "{date} {time} {name}.{ext}" }).names).toEqual(["2026-11-03 09.05.07 a.jpg.jpg"]);
  });

  it("finds and replaces literally or with a regular expression", () => {
    expect(run(["IMG_0001.jpg", "img_0002.jpg"], { find: "img_", replace: "Photo " }).names).toEqual(["Photo 0001.jpg", "Photo 0002.jpg"]);
    expect(run(["IMG_0001.jpg"], { find: "img_", replace: "x", matchCase: true }).names).toEqual(["IMG_0001.jpg"]);
    expect(run(["a.b.txt"], { find: ".", replace: "$" }).names).toEqual(["a$b.txt"]);
    expect(run(["2026-11-03 trip.jpg"], { find: "(\\d+)-(\\d+)-(\\d+)", replace: "$3.$2.$1", regex: true }).names).toEqual(["03.11.2026 trip.jpg"]);
    expect(run(["a.jpg"], { find: "(", regex: true }).error).toBeTruthy();
  });

  it("changes case, with or without the extension", () => {
    expect(run(["my holiday-photo.JPG"], { caseMode: "title" }).names).toEqual(["My Holiday-Photo.JPG"]);
    expect(run(["Report.PDF"], { caseMode: "lower" }).names).toEqual(["report.PDF"]);
    expect(run(["Report.PDF"], { caseMode: "lower", withExt: true }).names).toEqual(["report.pdf"]);
  });

  it("treats a folder name as a whole", () => {
    expect(newNames([item("v1.2", true)], { ...DEFAULT_RULES, template: "{name} old" }).names).toEqual(["v1.2 old"]);
  });

  it("reads names typed one per line", () => {
    expect(namesFromText("a\nb\n", 2)).toEqual(["a", "b"]);
    expect(namesFromText("a\r\nb", 2)).toEqual(["a", "b"]);
    expect(namesFromText("a", 2)).toBeNull();
  });
});

describe("batch rename checks", () => {
  const unix = { windows: false, ignoreCase: false };
  it("flags empty, invalid, duplicate and taken names", () => {
    const old = ["1.jpg", "2.jpg", "3.jpg", "4.jpg", "5.jpg"];
    const next = ["", "a/b.jpg", "x.jpg", "x.jpg", "other.jpg"];
    expect(renameIssues(old, next, ["1.jpg", "2.jpg", "3.jpg", "4.jpg", "5.jpg", "other.jpg"], unix))
      .toEqual(["empty", "invalid", "duplicate", "duplicate", "exists"]);
  });

  it("allows swaps and names freed by the batch", () => {
    expect(renameIssues(["a", "b"], ["b", "a"], ["a", "b"], unix)).toEqual([null, null]);
  });

  it("knows Windows rules and case-insensitive file systems", () => {
    const win = { windows: true, ignoreCase: true };
    expect(renameIssues(["1", "2", "3"], ["a:b", "con.txt", "end."], [], win)).toEqual(["invalid", "invalid", "invalid"]);
    expect(renameIssues(["1", "2"], ["A.jpg", "a.jpg"], [], win)).toEqual(["duplicate", "duplicate"]);
    expect(renameIssues(["1"], ["Other"], ["other"], win)).toEqual(["exists"]);
    expect(renameIssues(["1"], ["Other"], ["other"], unix)).toEqual([null]);
  });
});
