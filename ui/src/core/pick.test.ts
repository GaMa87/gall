import { describe, expect, it } from "vitest";
import type { PickArgs } from "../api/types";
import { acceptLabel, checkPick, globMatch, matchesFilter, validFileName, type Chosen } from "./pick";

const f = (name: string, dir = "/d"): Chosen => ({ path: `${dir}/${name}`, name, isDir: false });
const d = (name: string): Chosen => ({ path: `/d/${name}`, name, isDir: true });
const base: Pick<PickArgs, "mode" | "multiple" | "filters"> = { mode: "open", multiple: false, filters: [] };
const ctx = (selection: Chosen[], over = {}) => ({ dir: "/d", selection, typedName: "", filter: 0, ...over });

describe("filters", () => {
  it("matches globs without regard to case and escapes dots", () => {
    expect(globMatch("Photo.PNG", "*.png")).toBe(true);
    expect(globMatch("photo.png.bak", "*.png")).toBe(false);
    expect(globMatch("aXpng", "*.png")).toBe(false);
    expect(globMatch("r-01.txt", "r-??.txt")).toBe(true);
    expect(globMatch("r-1.txt", "r-??.txt")).toBe(false);
  });
  it("an empty filter lets everything through", () => {
    expect(matchesFilter("x.bin", undefined)).toBe(true);
    expect(matchesFilter("x.bin", { name: "All", patterns: [] })).toBe(true);
    expect(matchesFilter("x.bin", { name: "Images", patterns: ["*.png", "*.jpg"] })).toBe(false);
    expect(matchesFilter("x.JPG", { name: "Images", patterns: ["*.png", "*.jpg"] })).toBe(true);
  });
});

describe("open", () => {
  it("returns the selected files", () => {
    expect(checkPick(base, ctx([f("a.txt")]))).toEqual({ ok: true, paths: ["/d/a.txt"] });
  });
  it("needs a selection of files, one unless multiple", () => {
    expect(checkPick(base, ctx([]))).toMatchObject({ ok: false });
    expect(checkPick(base, ctx([d("x")]))).toMatchObject({ ok: false });
    expect(checkPick(base, ctx([f("a"), f("b")]))).toMatchObject({ ok: false });
    expect(checkPick({ ...base, multiple: true }, ctx([f("a"), f("b")]))).toEqual({ ok: true, paths: ["/d/a", "/d/b"] });
    expect(checkPick({ ...base, multiple: true }, ctx([f("a"), d("x")]))).toMatchObject({ ok: false });
  });
  it("refuses files the active filter hides", () => {
    const pick = { ...base, filters: [{ name: "Images", patterns: ["*.png"] }, { name: "All", patterns: [] }] };
    expect(checkPick(pick, ctx([f("a.txt")]))).toEqual({ ok: false, error: "“a.txt” does not match “Images”" });
    expect(checkPick(pick, ctx([f("a.txt")], { filter: 1 }))).toMatchObject({ ok: true });
  });
});

describe("dir", () => {
  const dir = { ...base, mode: "dir" as const };
  it("chooses the current folder when nothing is selected", () => {
    expect(checkPick(dir, ctx([]))).toEqual({ ok: true, paths: ["/d"] });
  });
  it("chooses selected folders only", () => {
    expect(checkPick(dir, ctx([d("x")]))).toEqual({ ok: true, paths: ["/d/x"] });
    expect(checkPick(dir, ctx([f("a")]))).toMatchObject({ ok: false });
    expect(checkPick(dir, ctx([d("x"), d("y")]))).toMatchObject({ ok: false });
    expect(checkPick({ ...dir, multiple: true }, ctx([d("x"), d("y")]))).toMatchObject({ ok: true });
  });
});

describe("save", () => {
  const save = { ...base, mode: "save" as const };
  it("joins the typed name to the folder", () => {
    expect(checkPick(save, ctx([], { typedName: "  report.txt " }))).toEqual({ ok: true, paths: ["/d/report.txt"] });
    expect(checkPick(save, ctx([], { typedName: "a b #1.md" }))).toEqual({ ok: true, paths: ["/d/a b #1.md"] });
  });
  it("rejects empty names and names with a folder part", () => {
    expect(checkPick(save, ctx([], { typedName: " " }))).toMatchObject({ ok: false });
    for (const bad of ["../x", "a/b", "a\\b", "..", "."]) expect(checkPick(save, ctx([], { typedName: bad })), bad).toMatchObject({ ok: false });
    expect(validFileName("ok.txt")).toBe(true);
  });
});

describe("labels", () => {
  it("uses the caller's label without its mnemonic underscore", () => {
    expect(acceptLabel({ mode: "open", accept: "_Select" })).toBe("Select");
    expect(acceptLabel({ mode: "save", accept: null })).toBe("Save");
    expect(acceptLabel({ mode: "dir", accept: null })).toBe("Select");
    expect(acceptLabel({ mode: "open", accept: null })).toBe("Open");
  });
});
