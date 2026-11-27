// Graphical self-test for WN-132: exercises DocumentCompare on a two-column PDF pair
// (via the isolated child-process PDF worker) and a DOCX pair with heading-level changes,
// moved blocks, table-row edits, jump-to-moved-block navigation, and Text view fallback.
import { join, parent } from "./core/path";
import * as A from "./state/actions";
import type { gitSelftest } from "./selftest";

type Helpers = typeof gitSelftest;

export async function runDocCompareSelfTest(h: Helpers, rootDir: string) {
  const { $, $$, click, key, until, sleep, shot, log } = h;
  const fixtureDir = join(parent(rootDir) ?? rootDir, "doc-compare");
  const leftPdf = join(fixtureDir, "left.pdf");
  const rightPdf = join(fixtureDir, "right.pdf");
  const leftDocx = join(fixtureDir, "left.docx");
  const rightDocx = join(fixtureDir, "right.docx");

  const closeOverlay = async () => {
    A.openOverlay(null);
    await until(() => !$(".diffv"), 3000);
    await sleep(80);
  };

  try {
    await h.updateSettings({ theme: "dark" });
    await h.setWindowSize(1320, 900);

    // 1. Two-column PDF pair with moved block + heading change (extracted via isolated child process).
    A.compare({ kind: "file", path: leftPdf }, { kind: "file", path: rightPdf });
    const pdfView = await until(() => $(".diffv .doccmp .doc-body"), 12000);
    log(!!pdfView, "doc-compare: two-column PDF pair opens DocumentCompare via isolated worker");
    const pdfRows = $$(".diffv .doc-row");
    const pdfTexts = pdfRows.map((r) => r.textContent ?? "").join("\n");
    const hasPositions = $$(".diffv .doc-pos").some((el) => /p\.\s*\d+\s*·\s*#\d+/.test(el.textContent ?? ""));
    const hasMoved = $$(".diffv .doc-badge.move").length >= 2;
    const leftIdx = pdfTexts.indexOf("Left column second line");
    const rightIdx = pdfTexts.indexOf("Right column first line");
    log(hasPositions && hasMoved && leftIdx >= 0 && rightIdx > leftIdx,
      "doc-compare: PDF shows page/block positions, column-by-column reading order and moved-block link",
      `rows=${pdfRows.length} leftIdx=${leftIdx} rightIdx=${rightIdx}`);
    await shot("doc-compare-pdf");
    h.note("SCREENSHOT doc-compare-pdf.png: DocumentCompare on two-column PDF pair showing page/block positions, column order, and moved block badge.");

    // Click Text view fallback to verify plain-text comparison remains one click away.
    const textBtn = $$<HTMLButtonElement>(".diffv .doc-bar button").find((b) => b.textContent?.includes("Text view"));
    if (textBtn) {
      click(textBtn);
      const textDiff = await until(() => $(".diffv .dv-body"), 6000);
      log(!!textDiff, "doc-compare: Text view switches PDF comparison to line-by-line text diff");
    }
    await closeOverlay();

    // 2. DOCX pair with heading-level change, moved block and table-row insertion/edit.
    A.compare({ kind: "file", path: leftDocx }, { kind: "file", path: rightDocx });
    const docxView = await until(() => $(".diffv .doccmp .doc-body"), 10000);
    log(!!docxView, "doc-compare: DOCX pair opens DocumentCompare");
    const badges = $$(".diffv .doc-badge").map((el) => el.textContent ?? "");
    const hasHeadingBadge = badges.some((b) => b.includes("H1 → H2"));
    const hasMoveBadge = badges.some((b) => b.includes("Moved"));
    const hasTableBadge = badges.some((b) => b.includes("Table row"));
    log(hasHeadingBadge && hasMoveBadge && hasTableBadge,
      "doc-compare: DOCX shows heading-level shift, moved block and table-row change badges",
      badges.join(" | "));
    const docxFoot = ($(".diffv .dv-foot")?.textContent ?? "").toLowerCase();
    log(docxFoot.includes("changes only") && !docxFoot.includes("whitespace"),
      "doc-compare: document footer advertises document-mode shortcuts", docxFoot);
    await shot("doc-compare-docx");
    h.note("SCREENSHOT doc-compare-docx.png: DocumentCompare on DOCX pair showing H1->H2 heading change, moved block and table-row cell highlight.");
    key("Escape", {}, $(".diffv"));
    await until(() => !$(".diffv"), 3000);
  } catch (e) {
    log(false, "doc-compare scenario", e instanceof Error ? e.stack ?? e.message : String(e));
    await shot("doc-compare-failure");
  }
}
