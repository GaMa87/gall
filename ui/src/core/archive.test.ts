import { describe, expect, it } from "vitest";
import { archiveBaseName, archiveInfo, archiveOf, createFormatFor, isBrowsableArchive } from "./archive";

describe("archive names", () => {
  it("finds the archive a path lies in", () => {
    expect(archiveOf("C:\\x\\photos.zip\\2024\\a.jpg")).toBe("C:\\x\\photos.zip");
    expect(archiveOf("C:\\x\\photos.zip")).toBe("C:\\x\\photos.zip");
    expect(archiveOf("/home/m/src.tar.gz/src/main.rs")).toBe("/home/m/src.tar.gz");
    expect(archiveOf("/home/m/docs/a.txt")).toBeNull();
    expect(archiveOf("/var/log/syslog.gz")).toBeNull(); // one stream, not a folder
    expect(isBrowsableArchive("a.7z") && isBrowsableArchive("b.tar.zst") && !isBrowsableArchive("c.xz")).toBe(true);
  });

  it("detects formats and stems like the backend", () => {
    expect(archiveInfo("Photos.TAR.GZ")).toEqual({ format: "tarGz", stem: "Photos" });
    expect(archiveInfo("a.b.zip")).toEqual({ format: "zip", stem: "a.b" });
    expect(archiveInfo("log.gz")).toEqual({ format: "gz", stem: "log" });
    expect(archiveInfo(".zip")).toBeNull();
    expect(archiveInfo("notes.txt")).toBeNull();
  });
  it("picks the format to create from a typed name", () => {
    expect(createFormatFor("x.7z")).toBe("sevenZ");
    expect(createFormatFor("x.tgz")).toBe("tarGz");
    expect(createFormatFor("x.tar.xz")).toBeNull();
    expect(createFormatFor("x")).toBeNull();
  });
  it("names a new archive after its contents", () => {
    expect(archiveBaseName([{ name: "report.pdf", isDir: false }], "Docs")).toBe("report");
    expect(archiveBaseName([{ name: "Photos", isDir: true }], "Docs")).toBe("Photos");
    expect(archiveBaseName([{ name: "a", isDir: false }, { name: "b", isDir: false }], "Docs")).toBe("Docs");
  });
});
