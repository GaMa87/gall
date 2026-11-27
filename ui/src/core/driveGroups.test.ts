import { describe, expect, it } from "vitest";
import type { Drive } from "../api/types";
import { groupDrives } from "./driveGroups";

const d = (short: string, mount: string, spaceShownBy?: string): Drive => ({
  short, label: "", mount, fs: "btrfs", total: 100, available: 50, removable: false, lockable: false, spaceShownBy,
});
const shape = (ds: Drive[]) => groupDrives(ds).map((g) => [g.drive.short, g.children.map((c) => c.short)]);

describe("groupDrives", () => {
  it("nests bind mounts under the drive that shows their space", () => {
    const out = [d("~", "/home/u", "/"), d("/", "/"), d("pkg", "/var/cache/pkg", "/"), d("log", "/var/log", "/"), d("data", "/mnt/data")];
    expect(shape(out)).toEqual([["~", []], ["/", ["pkg", "log"]], ["data", []]]);
  });
  it("keeps real separate drives and Home at top level", () => {
    expect(shape([d("~", "/home/u"), d("/", "/"), d("usb", "/run/media/u/usb")])).toEqual([["~", []], ["/", []], ["usb", []]]);
  });
  it("does not nest under a missing owner, nor chain or loop", () => {
    expect(shape([d("a", "/a", "gone")])).toEqual([["a", []]]);
    expect(shape([d("/", "/"), d("x", "/x", "/"), d("y", "/y", "x")])).toEqual([["/", ["x"]], ["y", []]]);
    expect(shape([d("a", "/a", "b"), d("b", "/b", "a")])).toEqual([["a", []], ["b", []]]);
  });
  it("keeps the original order of the children", () => {
    expect(shape([d("/", "/"), d("b", "/b", "/"), d("a", "/a", "/")])[0]).toEqual(["/", ["b", "a"]]);
  });
});
