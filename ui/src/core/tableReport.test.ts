import { describe, expect, it } from "vitest";
import type { SheetDiff } from "../api/types";
import { changeReport, csvCell } from "./tableReport";

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

describe("CSV escaping of the report", () => {
  it("escapes the key header the way it escapes data cells", () => {
    for (const key of ['id,"x"', "multi\nline", "semi;colon", "carriage\rreturn", 'say "hi"']) {
      const keyed: SheetDiff = { ...sheet, key, keyCandidates: [key], columns: [{ name: key, left: 0, right: 0 }, sheet.columns[1]] };
      const text = changeReport(keyed);
      expect(text.startsWith(`sheet,change,${csvCell(key)},column,left,right\n`), key).toBe(true);
      // Parsed back as one CSV record, the third field is the name as it was.
      expect(csvCell(key).startsWith('"')).toBe(true);
      expect(csvCell(key).slice(1, -1).replace(/""/g, '"')).toBe(key);
    }
  });

  it("leaves plain cells alone and quotes a bare carriage return", () => {
    expect(csvCell("plain")).toBe("plain");
    expect(csvCell("")).toBe("");
    expect(csvCell("a\rb")).toBe('"a\rb"');
    expect(csvCell('a"b')).toBe('"a""b"');
    expect(csvCell("a,b")).toBe('"a,b"');
  });

  it("an unkeyed report names its row column plainly", () => {
    const text = changeReport({ ...sheet, key: null });
    expect(text.split("\n")[0]).toBe("sheet,change,row,column,left,right");
  });
});
