import { describe, expect, it } from "vitest";
import { canApply, effectiveFormat, hunkSummary, isPatchFile, needsTarget, offsetText, parseStrip } from "./patchImport";

describe("patch import rules", () => {
  it("opens on .patch and .diff files only", () => {
    expect(isPatchFile({ name: "fix.PATCH", isDir: false })).toBe(true);
    expect(isPatchFile({ name: "a.diff", isDir: false })).toBe(true);
    expect(isPatchFile({ name: "x.patch", isDir: true })).toBe(false);
    expect(isPatchFile({ name: "notes.txt", isDir: false })).toBe(false);
    expect(isPatchFile({ name: "patch", isDir: false })).toBe(false);
  });
  it("reads with the chosen format, or the one the text announces", () => {
    expect(effectiveFormat("auto", "context")).toBe("context");
    expect(effectiveFormat("auto", null)).toBeNull();
    expect(effectiveFormat("normal", "unified")).toBe("normal");
  });
  it("takes an empty strip as automatic, whole numbers up to 20, and refuses the rest", () => {
    expect(parseStrip("")).toBeNull();
    expect(parseStrip(" 1 ")).toBe(1);
    expect(parseStrip("0")).toBe(0);
    expect(parseStrip("20")).toBe(20);
    for (const bad of ["21", "-1", "1.5", "a", "100", "1 2"]) expect(parseStrip(bad), bad).toBeUndefined();
  });
  it("asks for the file when the patch has no names", () => {
    expect(needsTarget([], "normal")).toBe(true);
    expect(needsTarget(["a patch without a file name needs its target chosen"], "unified")).toBe(true);
    expect(needsTarget(["f.txt: the file does not exist"], "unified")).toBe(false);
  });
  it("puts offsets into words and counts the hunks that were found away", () => {
    expect(offsetText(0)).toBe("");
    expect(offsetText(1)).toBe("1 line later than stated");
    expect(offsetText(-3)).toBe("3 lines earlier than stated");
    expect(hunkSummary(2, [0, 0])).toBe("2 hunks");
    expect(hunkSummary(1, [0])).toBe("1 hunk");
    expect(hunkSummary(3, [0, 4, -1])).toBe("3 hunks, 2 found away from the stated line (4 lines later than stated; 1 line earlier than stated)");
  });
  it("can be applied only after a clean preview of the current text and settings", () => {
    const clean = { files: [{}], problems: [] };
    expect(canApply(clean, true)).toBe(true);
    expect(canApply(clean, false)).toBe(false);
    expect(canApply({ files: [{}], problems: ["x"] }, true)).toBe(false);
    expect(canApply({ files: [], problems: [] }, true)).toBe(false);
    expect(canApply(null, true)).toBe(false);
  });
});
