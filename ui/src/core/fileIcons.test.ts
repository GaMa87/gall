import { describe, expect, it } from "vitest";
import { fileIconId, folderIconId, iconUrl } from "./fileIcons";

describe("catppuccin icon lookup", () => {
  it("maps by extension, stem and compound suffix", () => {
    expect(fileIconId("main.c")).toBe("c");
    expect(fileIconId("main.h")).toBe("@h");
    expect(fileIconId("vec.hpp")).toBe("@hpp");
    expect(iconUrl("@h", false)).toBe("/icons/gall/mocha/h.svg");
    expect(fileIconId("Cargo.toml")).toBe("cargo");
    expect(fileIconId("App.TSX")).toBe("typescript-react");
    expect(fileIconId("x.service.ts")).not.toBe("_file");
  });
  it("falls back safely", () => {
    expect(fileIconId("noext")).toBe("_file");
    expect(fileIconId("constructor")).toBe("_file");
    expect(folderIconId("Downloads")).toBe("folder_download");
    expect(folderIconId("whatever", true)).toBe("_folder_open");
  });
});
