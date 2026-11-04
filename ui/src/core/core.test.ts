import { describe, expect, it } from "vitest";
import { naturalCmp, sortEntries } from "./sort";
import { quickFilterMatches as m } from "./filter";
import { foldText, fuzzyMatch } from "./fuzzy";
import { age } from "./age";
import { ancestors, backOutside, basename, parent, join, splitExt, isInside, joinRel, tildify } from "./path";
import { altGrChar, buildKeymap, chordOf, normalizeChord, optionChar, typedChar } from "./keymap";
import { vimToken } from "./vim";
import { formatSize, unixMode, windowsMode } from "./format";
import type { FileEntry } from "../api/types";

const e = (name: string, isDir: boolean, size = 0, modified = 0): FileEntry => ({
  name, path: "/" + name, kind: isDir ? "dir" : "file", isDir, isHidden: false, isSymlink: false,
  size, modified, created: modified, ext: isDir ? "" : name.includes(".") ? name.split(".").pop()!.toLowerCase() : "", readonly: false,
});

describe("sort (mirrors Rust tests)", () => {
  it("natural order", () => {
    expect(["file10", "File2", "file1", "file02"].sort(naturalCmp)).toEqual(["file1", "File2", "file02", "file10"]);
  });
  it("folders on top + size desc", () => {
    const v = sortEntries([e("b.txt", false, 10), e("Zdir", true), e("a.txt", false, 99), e("adir", true)],
      { key: "size", descending: true, foldersOnTop: true, foldersByName: false });
    expect(v.map((x) => x.name)).toEqual(["Zdir", "adir", "a.txt", "b.txt"]);
  });
  it("folders by name ignores direction", () => {
    const v = sortEntries([e("bdir", true, 0, 9), e("adir", true, 0, 1), e("x.txt", false, 1, 5), e("y.txt", false, 1, 6)],
      { key: "modified", descending: true, foldersOnTop: true, foldersByName: true });
    expect(v.map((x) => x.name)).toEqual(["adir", "bdir", "y.txt", "x.txt"]);
  });
  it("calculated folder sizes sort folders by size (UI only)", () => {
    const sizes: Record<string, number> = { "/big": 900, "/small": 5 };
    const v = sortEntries([e("small", true), e("unknown", true), e("big", true), e("f.txt", false, 50)],
      { key: "size", descending: true, foldersOnTop: true, foldersByName: false }, (p) => sizes[p]);
    expect(v.map((x) => x.name)).toEqual(["big", "small", "unknown", "f.txt"]);
  });
});

describe("quick filter (mirrors Rust tests)", () => {
  it("rules", () => {
    expect(m("Report.pdf", "r")).toBe(true);
    expect(m("Report.pdf", "p")).toBe(false);
    expect(m("Report.pdf", "po")).toBe(true);
    expect(m("Report.pdf", ".pdf")).toBe(true);
    expect(m("Report.pdf.txt", ".pdf")).toBe(false);
    expect(m(".bashrc", ".bash")).toBe(false);
  });
});

describe("age", () => {
  it("labels", () => {
    const now = 10 * 365 * 86400000;
    expect(age(now - 5 * 60000, now).label).toBe("5'");
    expect(age(now - 70 * 60000, now).label).toBe("1h10'");
    expect(age(now - 2 * 86400000, now).label).toBe("2d");
  });
});

describe("paths", () => {
  it("unix", () => {
    expect(ancestors("/home/u/docs")).toEqual(["/", "/home", "/home/u", "/home/u/docs"]);
    expect(basename("/home/u")).toBe("u");
    expect(parent("/home")).toBe("/");
    expect(parent("/")).toBe(null);
    expect(join("/", "a")).toBe("/a");
    expect(isInside("/a/b", "/a")).toBe(true);
    expect(isInside("/ab", "/a")).toBe(false);
  });
  it("windows", () => {
    expect(ancestors("C:\\Users\\me")).toEqual(["C:\\", "C:\\Users", "C:\\Users\\me"]);
    expect(join("C:\\", "x")).toBe("C:\\x");
    expect(ancestors("\\\\srv\\share\\d\\e")).toEqual(["\\\\srv\\share\\", "\\\\srv\\share\\d", "\\\\srv\\share\\d\\e"]);
    expect(isInside("C:\\a\\b", "C:\\a")).toBe(true);
    expect(isInside("C:\\ab", "C:\\a")).toBe(false);
    expect(isInside("C:\\x", "C:\\")).toBe(true);
    expect(joinRel("C:\\l", "sub/new.txt")).toBe("C:\\l\\sub\\new.txt");
    expect(tildify("C:\\Users\\me\\Docs", "C:\\Users\\me")).toBe("~\\Docs");
    expect(tildify("C:\\Users\\meme", "C:\\Users\\me")).toBe("C:\\Users\\meme");
  });
  it("relative joins and ~ (unix)", () => {
    expect(joinRel("/l", "sub/new.txt")).toBe("/l/sub/new.txt");
    expect(joinRel("/", "a/b")).toBe("/a/b");
    expect(tildify("/home/u/x", "/home/u")).toBe("~/x");
    expect(tildify("/home/u", "/home/u")).toBe("~");
    expect(tildify("/home/uu", "/home/u")).toBe("/home/uu");
  });
  it("splitExt", () => {
    expect(splitExt("a.tar.gz")).toEqual(["a.tar", ".gz"]);
    expect(splitExt(".env")).toEqual([".env", ""]);
  });
});

describe("keymap", () => {
  it("chords", () => {
    const ev = (key: string, mods: Partial<{ ctrlKey: boolean; shiftKey: boolean; altKey: boolean; code: string }> = {}) =>
      chordOf({ key, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...mods });
    expect(ev("n", { ctrlKey: true, shiftKey: true })).toBe("Ctrl+Shift+N");
    expect(ev(" ")).toBe("Space");
    expect(ev("~", { shiftKey: true })).toBe("Shift+~");
    expect(ev("ç", { altKey: true, code: "KeyC" })).toBe("Alt+C");
    expect(normalizeChord("shift+ctrl+n")).toBe("Ctrl+Shift+N");
  });
  it("AltGr characters are characters, not Ctrl+Alt shortcuts (Windows)", () => {
    const altGr = (key: string, code: string) =>
      ({ key, code, ctrlKey: true, altKey: true, metaKey: false, shiftKey: false });
    // Slovenian "\" is AltGr+Q: Edit path, not Ctrl+Alt+Q.
    expect(chordOf(altGr("\\", "KeyQ"))).toBe("\\");
    // Polish "ś" is AltGr+S: not Calculate folder sizes (Ctrl+Alt+S).
    expect(chordOf(altGr("ś", "KeyS"))).not.toBe("Ctrl+Alt+S");
    expect(altGrChar(altGr("ś", "KeyS"))).toBe(true);
    // A real Ctrl+Alt shortcut gives the key's own letter.
    expect(chordOf(altGr("s", "KeyS"))).toBe("Ctrl+Alt+S");
    expect(altGrChar(altGr("t", "KeyT"))).toBe(false);
    // Slovenian "`" is AltGr+7.
    expect(chordOf(altGr("`", "Digit7"))).toBe("`");
    expect(vimToken(altGr("[", "KeyF"))).toBe("[");
  });
  it("Option characters are typed text on macOS unless the chord is a shortcut", () => {
    const opt = (key: string, code: string) => ({ key, code, ctrlKey: false, altKey: true, metaKey: false, shiftKey: false });
    const km = buildKeymap({});
    const isShortcut = (c: string) => km.has(c);
    // German / Slovenian Mac: "@" is Option+L, "[" Option+5.
    expect(typedChar(opt("@", "KeyL"), isShortcut, true)).toBe(true);
    expect(typedChar(opt("[", "Digit5"), isShortcut, true)).toBe(true);
    // Option+C is "Copy to other browser": stays a shortcut.
    expect(typedChar(opt("ç", "KeyC"), isShortcut, true)).toBe(false);
    // Not on other systems (Alt there is no character key).
    expect(typedChar(opt("@", "KeyL"), isShortcut, false)).toBe(false);
    expect(optionChar(opt("l", "KeyL"), () => false, true)).toBe(false);
    // Plain keys and AltGr keep working.
    expect(typedChar({ key: "a", code: "KeyA", ctrlKey: false, altKey: false, metaKey: false }, isShortcut, false)).toBe(true);
  });
  it("overrides replace defaults", () => {
    const k = buildKeymap({ toggleSidebar: "Ctrl+B" });
    expect(k.get("Ctrl+B")).toBe("toggleSidebar");
    expect(k.get("Alt+S")).toBeUndefined();
    expect(k.get("F2")).toBe("rename");
  });
});

describe("format", () => {
  it("sizes", () => {
    expect(formatSize(512)).toBe("512 B");
    expect(formatSize(1536)).toBe("1.50 KB");
    expect(formatSize(286.83 * 1024)).toBe("287 KB");
  });
});

describe("leaving a drive", () => {
  it("goes back to the latest folder outside it", () => {
    const back = ["/home/u", "/home/u/Docs", "/run/media/u/Stick", "/run/media/u/Stick/Photos", "/home/u", "/run/media/u/Stick2"];
    expect(backOutside(back, "/run/media/u/Stick")).toEqual(["/run/media/u/Stick2", "/home/u", "/home/u/Docs"]);
    expect(backOutside(["/run/media/u/Stick/a"], "/run/media/u/Stick")).toEqual([]);
  });
});

describe("accents", () => {
  it("fold unless typed", () => {
    expect(foldText("Poročilo čšž ŁÓDŹ đ")).toBe("Porocilo csz LODZ d");
    expect(foldText("日本 😀").length).toBe("日本 😀".length);
    expect(m("Poročilo.txt", "poroc")).toBe(true);
    expect(m("Čebela.txt", "c")).toBe(true);
    expect(m("porocilo.txt", "poroč")).toBe(false);
    expect(fuzzyMatch("porocilo", "Poročilo čšž")?.positions).toEqual([0, 1, 2, 3, 4, 5, 6, 7]);
    expect(fuzzyMatch("poročilo", "porocilo")).toBeNull();
  });
});
describe("unixMode", () => {
  it("reads like ls -l", () => {
    expect(unixMode(0o040755)).toBe("drwxr-xr-x");
    expect(unixMode(0o120777)).toBe("lrwxrwxrwx");
    expect(unixMode(0o100644)).toBe("-rw-r--r--");
    expect(unixMode(0o041777)).toBe("drwxrwxrwt");
    expect(unixMode(0o104755)).toBe("-rwsr-xr-x");
    expect(unixMode(0o102644)).toBe("-rw-r-Sr--");
    expect(unixMode(0o040550)).toBe("dr-xr-x---");
  });
});

describe("windowsMode", () => {
  it("reads like PowerShell 7", () => {
    expect(windowsMode(0x10, true)).toBe("d----");
    expect(windowsMode(0x20, false)).toBe("-a---");
    expect(windowsMode(0x21, false)).toBe("-ar--");
    expect(windowsMode(0x16, true)).toBe("d--hs");
    expect(windowsMode(0x410, true)).toBe("l----");
  });
});
