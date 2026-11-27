import { describe, expect, it } from "vitest";
import type { DocBlock, DocExtract } from "../api/types";
import { compareDocumentBlocks, formatBlockGeometry, formatBlockPosition, isSameDocumentKind } from "./documentBlocks";

function makeDoc(format: "docx" | "pdf", items: DocBlock[]): DocExtract {
  return {
    format,
    text: items.map((b) => b.text).join("\n"),
    blocks: items.filter((b) => b.kind !== "sectionBreak").length,
    sections: Math.max(1, ...items.map((b) => b.page)),
    items,
    warnings: [],
    truncated: false,
  };
}

describe("documentBlocks", () => {
  it("matches only same-kind document pairs (.docx/.docx or .pdf/.pdf)", () => {
    expect(isSameDocumentKind("/a/one.pdf", "/b/two.PDF")).toBe(true);
    expect(isSameDocumentKind("/a/one.docx", "/b/two.docx")).toBe(true);
    expect(isSameDocumentKind("/a/one.docx", "/b/two.pdf")).toBe(false);
    expect(isSameDocumentKind("/a/one.txt", "/b/two.txt")).toBe(false);
  });

  it("detects heading-level changes with exact page and block positions", () => {
    const left = makeDoc("docx", [
      { line: 1, page: 1, block: 1, kind: "heading", headingLevel: 1, text: "Introduction" },
      { line: 2, page: 1, block: 2, kind: "paragraph", text: "Unchanged body text." },
    ]);
    const right = makeDoc("docx", [
      { line: 1, page: 1, block: 1, kind: "heading", headingLevel: 2, text: "Introduction" },
      { line: 2, page: 1, block: 2, kind: "paragraph", text: "Unchanged body text." },
    ]);
    const res = compareDocumentBlocks(left, right);
    expect(res.counts.headingChanges).toBe(1);
    expect(res.counts.same).toBe(1);
    expect(res.rows[0].layoutChange).toBe("headingLevel");
    expect(res.rows[0].layoutDetail).toBe("H1 → H2");
    expect(formatBlockPosition(res.rows[0].left!, "docx")).toBe("§1 · #1");
  });

  it("detects a moved block across PDF pages with geometry and links both ends", () => {
    const left = makeDoc("pdf", [
      { line: 1, page: 1, block: 0, kind: "sectionBreak", text: "--- page 1 ---" },
      { line: 2, page: 1, block: 1, kind: "paragraph", text: "Executive summary paragraph stays first.", geometry: { x: 50, y: 720, width: 300, height: 12, pageWidth: 612, pageHeight: 792 } },
      { line: 3, page: 1, block: 2, kind: "paragraph", text: "Important compliance notice moved to page two.", geometry: { x: 50, y: 690, width: 310, height: 12, pageWidth: 612, pageHeight: 792 } },
      { line: 4, page: 2, block: 0, kind: "sectionBreak", text: "--- page 2 ---" },
      { line: 5, page: 2, block: 1, kind: "paragraph", text: "Second page conclusion." },
    ]);
    const right = makeDoc("pdf", [
      { line: 1, page: 1, block: 0, kind: "sectionBreak", text: "--- page 1 ---" },
      { line: 2, page: 1, block: 1, kind: "paragraph", text: "Executive summary paragraph stays first." },
      { line: 3, page: 2, block: 0, kind: "sectionBreak", text: "--- page 2 ---" },
      { line: 4, page: 2, block: 1, kind: "paragraph", text: "Second page conclusion." },
      { line: 5, page: 2, block: 2, kind: "paragraph", text: "Important compliance notice moved to page two.", geometry: { x: 50, y: 650, width: 310, height: 12, pageWidth: 612, pageHeight: 792 } },
    ]);
    const res = compareDocumentBlocks(left, right);
    expect(res.counts.moved).toBe(1);
    const movedFrom = res.rows.find((r) => r.layoutChange === "movedFrom");
    const movedTo = res.rows.find((r) => r.layoutChange === "movedTo");
    expect(movedFrom).toBeDefined();
    expect(movedTo).toBeDefined();
    expect(movedFrom!.moveId).toBe(movedTo!.moveId);
    expect(movedFrom!.movePartnerIndex).toBe(movedTo!.index);
    expect(movedTo!.movePartnerIndex).toBe(movedFrom!.index);
    expect(movedFrom!.layoutDetail).toBe("→ p. 2 · #2");
    expect(movedTo!.layoutDetail).toBe("← p. 1 · #2");
    expect(formatBlockGeometry(movedFrom!.left!)).toBe("x=50 pt, y=690 pt (310×12 pt)");
  });

  it("recognizes inserted table rows and cell-level changes with table coordinates", () => {
    const left = makeDoc("docx", [
      { line: 1, page: 1, block: 1, kind: "tableRow", table: 1, tableRow: 1, cells: ["Region", "Units", "Revenue"], text: "Region | Units | Revenue" },
      { line: 2, page: 1, block: 2, kind: "tableRow", table: 1, tableRow: 2, cells: ["North", "120", "4800"], text: "North | 120 | 4800" },
    ]);
    const right = makeDoc("docx", [
      { line: 1, page: 1, block: 1, kind: "tableRow", table: 1, tableRow: 1, cells: ["Region", "Units", "Revenue"], text: "Region | Units | Revenue" },
      { line: 2, page: 1, block: 2, kind: "tableRow", table: 1, tableRow: 2, cells: ["North", "145", "5800"], text: "North | 145 | 5800" },
      { line: 3, page: 1, block: 3, kind: "tableRow", table: 1, tableRow: 3, cells: ["East", "80", "3200"], text: "East | 80 | 3200" },
    ]);
    const res = compareDocumentBlocks(left, right);
    expect(res.counts.tableRowChanges).toBe(2);
    const edited = res.rows.find((r) => r.layoutChange === "tableRowChanged");
    const inserted = res.rows.find((r) => r.layoutChange === "tableRowAdded");
    expect(edited?.changedCells).toEqual([1, 2]);
    expect(inserted?.right?.cells).toEqual(["East", "80", "3200"]);
    expect(formatBlockPosition(inserted!.right!, "docx")).toBe("§1 · #3 · tbl 1 r 3");
  });

  it("reports zero changes on identical documents and handles empty documents cleanly", () => {
    const empty = makeDoc("pdf", []);
    const res = compareDocumentBlocks(empty, empty);
    expect(res.rows).toHaveLength(0);
    expect(res.counts.added + res.counts.removed + res.counts.changed + res.counts.moved).toBe(0);
  });
});
