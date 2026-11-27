import { describe, expect, it } from "vitest";
import { choiceFor, defaultTicked, importable, initialTicks, skippedReason, tally, type ImportPlace } from "./favoritesImport";

const place = (path: string, over: Partial<ImportPlace> = {}): ImportPlace => ({ path, label: path.split("/").pop() || "/", hidden: false, kind: "folder", missing: false, duplicate: false, ...over });

describe("favorites import checklist", () => {
  const places: ImportPlace[] = [
    place("/home/ana/Documents"),
    place("/home/ana/old", { missing: true }),
    place("/mnt/hidden", { hidden: true }),
    place("/home/ana/known", { duplicate: true }),
    place("sftp://host/x", { kind: { skipped: "not a folder on this computer (sftp:)" } }),
    place("/srv/data", { label: "Shared data" }),
  ];

  it("ticks usable places only: not missing, hidden, skipped or already favorites", () => {
    expect([...initialTicks(places)]).toEqual([0, 5]);
    expect(places.map(defaultTicked)).toEqual([true, false, false, false, false, true]);
  });

  it("lets the user tick a missing or hidden place, but never a skipped or duplicate one", () => {
    expect(places.map(importable)).toEqual([true, true, true, false, false, true]);
    expect(skippedReason(places[4])).toContain("sftp");
    expect(skippedReason(places[0])).toBeNull();
  });

  it("sends the ticked places in the source's order with their labels as aliases", () => {
    const choice = choiceFor("GTK bookmarks", places, new Set([5, 0, 1]));
    expect(choice).toEqual({ source: "GTK bookmarks", items: [{ path: "/home/ana/Documents", alias: "Documents" }, { path: "/home/ana/old", alias: "old" }, { path: "/srv/data", alias: "Shared data" }] });
  });

  it("never sends a place that cannot be imported, even when it is ticked", () => {
    expect(choiceFor("S", places, new Set([3, 4]))).toBeNull();
    expect(choiceFor("S", places, new Set())).toBeNull();
    expect(choiceFor("S", [], new Set([0]))).toBeNull();
  });

  it("sends no alias for an empty label", () => {
    const choice = choiceFor("S", [place("/x", { label: "   " })], new Set([0]));
    expect(choice?.items[0].alias).toBeNull();
  });

  it("counts what can be ticked and what is", () => {
    expect(tally(places, initialTicks(places))).toEqual({ usable: 4, ticked: 2 });
    // A tick on something unusable is not counted.
    expect(tally(places, new Set([3, 4, 0]))).toEqual({ usable: 4, ticked: 1 });
    expect(tally([], new Set())).toEqual({ usable: 0, ticked: 0 });
  });
});
