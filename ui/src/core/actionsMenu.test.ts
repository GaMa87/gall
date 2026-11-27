import { describe, expect, it } from "vitest";
import type { ActionDef } from "../api/types";
import { actionLabel, actionPaths, mergeZoxide, validateServerUri } from "./actionsMenu";

const def = (o: Partial<ActionDef>): ActionDef => ({ id: "x", label: "Send", ...o });

describe("validateServerUri", () => {
  it("accepts the supported schemes", () => {
    for (const s of ["smb://nas/share", "sftp://me@host/home", "nfs://10.0.0.2/export", "davs://cloud/remote.php/dav", "ftp://host"]) {
      expect(validateServerUri(`  ${s} `)).toEqual({ ok: true, uri: s });
    }
    expect(validateServerUri("SMB://Nas/Share")).toEqual({ ok: true, uri: "smb://Nas/Share" });
  });
  it("rejects bad input", () => {
    expect(validateServerUri("").ok).toBe(false);
    expect(validateServerUri("nas/share").ok).toBe(false);
    expect(validateServerUri("http://x").ok).toBe(false);
    expect(validateServerUri("file:///etc").ok).toBe(false);
    expect(validateServerUri("smb://").ok).toBe(false);
    expect(validateServerUri("smb:///x").ok).toBe(false);
    expect(validateServerUri("smb://a b").ok).toBe(false);
  });
});

describe("actionLabel", () => {
  it("uses the plural label with {count} for several items", () => {
    const d = def({ label: "Send via LocalSend", pluralLabel: "Send {count} items via LocalSend" });
    expect(actionLabel(d, 1)).toBe("Send via LocalSend");
    expect(actionLabel(d, 3)).toBe("Send 3 items via LocalSend");
  });
  it("falls back to the singular label", () => {
    expect(actionLabel(def({}), 4)).toBe("Send");
    expect(actionLabel(def({ label: "Do {count}" }), 2)).toBe("Do 2");
  });
});

describe("actionPaths", () => {
  it("targets the folder when nothing is selected", () => {
    expect(actionPaths([], "/home/a")).toEqual(["/home/a"]);
  });
});

describe("mergeZoxide", () => {
  const z = ["/home/a/proj", "/home/a/other", "/srv/x", "/home/a/proj2"];
  it("dedupes against existing hits and filters", () => {
    expect(mergeZoxide(z, ["/home/a/proj"], (p) => p.includes("proj"), null)).toEqual(["/home/a/proj2"]);
  });
  it("limits to a scope and a count", () => {
    expect(mergeZoxide(z, [], () => true, "/home/a")).toEqual(["/home/a/proj", "/home/a/other", "/home/a/proj2"]);
    expect(mergeZoxide(z, [], () => true, null, 2)).toEqual(["/home/a/proj", "/home/a/other"]);
  });
});
