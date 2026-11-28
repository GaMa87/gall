import { describe, expect, it } from "vitest";
import { isMissingError, operationError, volumeMountError } from "./operationError";

describe("operation feedback", () => {
  it("turns common filesystem mount failures into actionable, non-reportable feedback and keeps details", () => {
    const raw = "wrong fs type, bad option, bad superblock on /dev/sdb1, missing codepage or helper program, or other error";
    expect(volumeMountError(raw, "ntfs")).toEqual({
      text: "Could not mount “{name}”. The NTFS volume may have been left hibernated by Windows or may need checking. Fully shut down Windows and disable Fast Startup; if it still will not mount, run chkdsk in Windows. If this system has no NTFS support, install ntfs-3g and ntfsprogs.",
      kind: "warn", reportable: false, details: raw,
    });
    expect(volumeMountError("unknown filesystem type 'exfat'", "exfat")).toMatchObject({
      text: "Could not mount “{name}”. Check that exFAT support (exfatprogs) is installed and that the volume is healthy.",
      kind: "warn", reportable: false,
    });
    expect(volumeMountError("Error mounting /dev/sdb1: No such file or directory", "ntfs")).toMatchObject({
      text: "Could not find volume “{name}”. Check that the drive is connected and try again.",
      kind: "warn", reportable: false,
    });
    expect(volumeMountError("udisks service is unavailable", "ntfs")).toBeUndefined();
  });

  it.each([
    ["operation cancelled", "info"],
    ["invalid name 'bad/name'", "warn"],
    ["'/tmp/draft' already exists", "warn"],
    ["cannot copy: '/tmp/project/child' is inside '/tmp/project'", "warn"],
    ["two items would be named “draft”", "warn"],
    ["“Photos” cannot be replaced: it contains “Photos”", "warn"],
    ["Nothing was copied: cannot copy 1 item inside WSL with its name: “a:b” in /tmp.", "warn"],
    ["cannot safely restore conflicting entry “draft”", "error"],
    ["archive-password:wrong:C:\\data\\a.7z", "warn"],
    ["Wrong passphrase", "warn"],
    ["Not authorized to mount this volume", "warn"],
    ["/run/media/user/drive is not mounted", "warn"],
    ["no-trash: The drive has no trash.", "error"],
    ["zip: invalid archive", "error"],
    ["clipboard unavailable: no display server", "error"],
    ["gio: nfs://192.168.0.101: volume doesn't implement mount", "error"],
    ["Cannot connect to nfs://192.168.0.101: neither KDE kio-fuse nor GVFS is available. Install kio-fuse or gvfs-nfs (or gvfs-backends).", "error"],
    ["failed to disconnect /run/user/1000/gvfs/smb-share:server=nas,share=x", "error"],
    ["rename failed for '/tmp/draft': Permission denied (os error 13)", "error"],
  ])("treats %s as an expected %s", (raw, kind) => {
    expect(operationError(raw, "linux")).toMatchObject({ kind, reportable: false });
  });

  it("uses the OS code for localized messages on the corresponding platform", () => {
    const raw = "rename failed for 'C:\\data\\draft': Die Datei ist belegt. (os error 32)";
    expect(operationError(raw, "windows")).toEqual({ text: "Cannot rename “draft”: it is in use.", kind: "error", reportable: false });
    expect(operationError(raw, "linux").reportable).toBe(true);
  });

  it("recognizes Windows symlink privileges and Darwin path and quota error codes", () => {
    expect(operationError("symlink failed for 'C:\\link': local message (os error 1314)", "windows"))
      .toMatchObject({ text: "Cannot symlink “link”: you have no permission.", reportable: false });
    expect(operationError("mkdir failed for '/tmp/a': local message (os error 63)", "macos"))
      .toMatchObject({ text: "Cannot mkdir “a”: the name or path is too long.", reportable: false });
    expect(operationError("write failed for '/tmp/a': local message (os error 69)", "macos").reportable).toBe(false);
    expect(operationError("mkdir failed for '/tmp/a': local message (os error 36)", "macos").reportable).toBe(true);
  });

  it("keeps unexpected failures reportable even when reformatted", () => {
    const raw = "list failed for '/tmp/Permission denied': Internal failure (os error 3)";
    expect(operationError(raw, "linux")).toEqual({
      text: "Cannot open “Permission denied”: Internal failure", kind: "error", reportable: true,
    });
    expect(operationError("database: unexpected result", "linux").reportable).toBe(true);
  });

  it("recognizes a missing folder without reading an error from the path itself", () => {
    expect(isMissingError("list failed for '/tmp/a': No such file or directory (os error 2)", "linux")).toBe(true);
    expect(isMissingError("list failed for 'C:\\folder': Pfad nicht gefunden. (os error 3)", "windows")).toBe(true);
    expect(isMissingError("list failed for '/tmp/No such file or directory': Unknown failure (os error 3)", "linux")).toBe(false);
  });

  it("handles filenames spanning multiple lines", () => {
    expect(operationError("open failed for '/tmp/line\nbreak': Permission denied", "linux")).toMatchObject({
      text: "Cannot open “line\nbreak”: you have no permission.", reportable: false,
    });
  });
});
