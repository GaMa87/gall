import { describe, expect, it } from "vitest";
import type { SheetDiff } from "../api/types";
import { changeReport } from "./tableReport";

const sheet: SheetDiff = {
  name: "S", presence: "both", key: "id", keyCandidates: ["id"],
  columns: [{ name: "id", left: 0, right: 0 }, { name: "price", left: 1, right: 1 }],
  leftRows: [["1", "1.0"], ["2", "5"]],
  rightRows: [["1", "2,5"], ["3", "7"]],
  rows: [
    { status: "changed", left: 0, right: 0, cells: [1] },
    { status: "removed", left: 1, right: null, cells: [] },
    { status: "added", left: null, right: 1, cells: [] },
  ],
  counts: { same: 0, added: 1, removed: 1, changed: 1, cells: 1 }, truncated: false,
};

describe("table change report", () => {
  it("one line per changed cell / row, CSV-quoted", () => {
    const lines = changeReport(sheet).trim().split("\n");
    expect(lines[0]).toBe("sheet,change,id,column,left,right");
    expect(lines[1]).toBe('S,changed,1,price,1.0,"2,5"');
    expect(lines[2]).toBe("S,removed,2,,2 | 5,");
    expect(lines[3]).toBe("S,added,3,,,3 | 7");
  });
});
