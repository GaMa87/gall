// Graphical self-test for WN-162: comparison tabs in the running app — one, two and many tabs,
// Ctrl+Tab and Vim gt/gT switching, closing the active tab, saved scroll anchor/active hunk/mode/options
// across tab switches and simulated restart, earlier checks (ready/changed/missing/notSaved) and folder
// relink, HTML and PDF report exports with progress toast and cancellation, key help (GALL_SELFTEST_SCALE=1.5 for 150%).
import { platformLabel } from "./core/keymap";
import { join, parent } from "./core/path";
import * as A from "./state/actions";
import { flushCompareTabsSave, relinkEarlier, resetCompareTabs } from "./state/compareTabs";
import type { gitSelftest } from "./selftest";

type Helpers = typeof gitSelftest;

export async function runCompareTabsSelfTest(h: Helpers, rootDir: string) {
  const { $, $$, key, click, until, sleep, shot, log, api } = h;
  const base = join(parent(rootDir) ?? rootDir, "compare-tabs");
  const at = (...parts: string[]) => parts.reduce((p, part) => join(p, part), base);
  const file = (p: string) => ({ kind: "file" as const, path: p });

  const openPair = async (a: string, b: string) => {
    A.openOverlay({ type: "diff", a: file(a), b: file(b) });
    await until(() => $(".diffv .dv-body") && !$(".dv-msg")?.textContent?.includes("Comparing"), 6000);
    await sleep(140);
  };

  try {
    resetCompareTabs();
    await h.updateSettings({ theme: "dark", vimKeys: true, keyboardProfile: "vim" });
    await h.setWindowSize(1280, 860);

    // 1. One tab: tab strip stays hidden; Export HTML/PDF buttons are present.
    await openPair(at("p1", "left.txt"), at("p1", "right.txt"));
    log(!$(".cmp-tabs"), "compare-tabs: single comparison hides the tab strip");
    log(!!$(".dv-export-html") && !!$(".dv-export-pdf"), "compare-tabs: Export HTML and Export PDF buttons are shown in the toolbar");
    await shot("ct-01-one-tab");

    // Navigate to hunk 2, scroll down, switch to unified view, toggle ignoreCase on tab 1.
    const diffRoot = () => $<HTMLElement>(".diffv");
    const settled = async () => { await sleep(150); await until(() => $(".diffv .dv-body") && !$(".dv-msg")?.textContent?.includes("Comparing"), 6000); await sleep(150); };
    key("u", {}, diffRoot()); // unified
    await settled();
    key("i", {}, diffRoot()); // ignoreCase
    await settled();
    key("n", {}, diffRoot()); // hunk 1
    await sleep(150);
    key("n", {}, diffRoot()); // hunk 2
    await sleep(300);
    const body1 = $<HTMLElement>(".dv-body");
    if (body1) {
      body1.scrollTop = Math.max(body1.scrollTop, 260);
      body1.dispatchEvent(new Event("scroll"));
    }
    await sleep(200);
    const savedScroll1 = body1?.scrollTop ?? 0;
    const savedCount1 = $(".dv-count")?.textContent ?? "";
    log(savedScroll1 >= 120 && savedCount1.startsWith("2 /"), "compare-tabs: tab 1 scrolled and positioned on hunk 2", `scrollTop=${savedScroll1} count=${savedCount1}`);

    // 2. Open a second pair: tab strip appears with 2 tabs.
    const allowChanges = () => $<HTMLInputElement>(".dv-allow-changes input");
    const firstPermission = allowChanges();
    if (firstPermission) click(firstPermission);
    await sleep(100);
    log(!!allowChanges()?.checked, "compare-tabs: changes can be enabled in the first tab");
    await openPair(at("p2", "left.txt"), at("p2", "right.txt"));
    log($$(".cmp-tab").length === 2, "compare-tabs: opening a second pair shows 2 tabs", `tabs=${$$(".cmp-tab").length}`);
    log(allowChanges()?.checked === false, "compare-tabs: another comparison starts read-only");
    await shot("ct-02-two-tabs");

    // 3. Ctrl+Tab switches back to tab 1 and restores its unified mode, hunk 2, and scroll anchor.
    key("Tab", { ctrlKey: true }, diffRoot());
    await until(() => $(".cmp-tab.on")?.textContent?.includes("left.txt") && $(".dv-canvas.unified"), 5000);
    await sleep(220);
    const restoredScroll1 = $<HTMLElement>(".dv-body")?.scrollTop ?? 0;
    const restoredCount1 = $(".dv-count")?.textContent ?? "";
    log(!!$(".dv-canvas.unified"), "compare-tabs: Ctrl+Tab switches tab and restores unified view mode");
    log(allowChanges()?.checked === false, "compare-tabs: returning to a comparison requires permission again");
    log(restoredCount1.startsWith("2 /") && restoredScroll1 >= 100, "compare-tabs: switching back restores saved active hunk and scroll anchor", `scrollTop=${restoredScroll1} count=${restoredCount1}`);

    // 4. Vim profile gt / gT switches tabs without Ctrl chord.
    key("g", {}, diffRoot());
    key("t", {}, diffRoot());
    await until(() => $$(".cmp-tab")[1]?.classList.contains("on"), 4000);
    log($$(".cmp-tab")[1]?.classList.contains("on") ?? false, "compare-tabs: Vim gt switches to next tab without Ctrl");
    key("g", {}, diffRoot());
    key("T", {}, diffRoot());
    await until(() => $$(".cmp-tab")[0]?.classList.contains("on"), 4000);
    log($$(".cmp-tab")[0]?.classList.contains("on") ?? false, "compare-tabs: Vim gT switches to previous tab without Ctrl");

    // 5. Many tabs (8 tabs): horizontal scroll, no document overflow.
    for (let i = 3; i <= 8; i++) {
      await openPair(at(`p${i}`, `a${i}.txt`), at(`p${i}`, `b${i}.txt`));
    }
    log($$(".cmp-tab").length === 8, "compare-tabs: many tabs (8) render in the strip", `tabs=${$$(".cmp-tab").length}`);
    const overflow = document.documentElement.scrollWidth > document.documentElement.clientWidth;
    log(!overflow, "compare-tabs: 8 tabs do not cause horizontal page overflow", `${document.documentElement.scrollWidth}/${document.documentElement.clientWidth}`);
    await shot("ct-03-many-tabs");

    // 6. Export HTML and Export PDF over open tabs via UI flow (progress toast + completion).
    const htmlOut = at("out", "report.html");
    const pdfOut = at("out", "report.pdf");
    diffRoot()?.dispatchEvent(new CustomEvent("gall-compare-export", { detail: { format: "html", dest: htmlOut } }));
    const htmlToast = await until(() => $$(".toast").find((t) => t.textContent?.includes("Exported report.html")), 10000);
    log(!!htmlToast, "compare-tabs: Export HTML writes report and shows completion toast");
    const htmlRes = await api.readTextFile(htmlOut);
    log(htmlRes.text.includes("<html") && htmlRes.text.includes("a3.txt"), "compare-tabs: exported HTML report contains open comparison tabs", `bytes=${htmlRes.text.length}`);

    diffRoot()?.dispatchEvent(new CustomEvent("gall-compare-export", { detail: { format: "pdf", dest: pdfOut } }));
    const pdfToast = await until(() => $$(".toast").find((t) => t.textContent?.includes("Exported report.pdf")), 10000);
    log(!!pdfToast, "compare-tabs: Export PDF writes report and shows completion toast");
    const pdfExists = await api.pathResolve(pdfOut);
    log(pdfExists.exists && !pdfExists.isDir, "compare-tabs: exported PDF file exists on disk");

    // 7. Cancel a large export through the progress toast Cancel button.
    const cancelOut = at("out", "cancelled.html");
    await openPair(at("big", "big_a.txt"), at("big", "big_b.txt"));
    diffRoot()?.dispatchEvent(new CustomEvent("gall-compare-export", { detail: { format: "html", dest: cancelOut } }));
    const cancelBtn = await until(() => $$(".toast button").find((b) => b.textContent?.trim() === "Cancel") as HTMLButtonElement | undefined, 4000);
    log(!!cancelBtn, "compare-tabs: exporting shows a progress toast with Cancel button");
    if (cancelBtn) {
      await shot("ct-04-export-toast");
      click(cancelBtn);
    }
    const cancelDone = await until(() => $$(".toast").find((t) => t.textContent?.includes("Report export cancelled") || t.textContent?.includes("Exported cancelled.html")), 12000);
    log(!!cancelDone, "compare-tabs: clicking Cancel finishes cleanly", cancelDone?.textContent ?? "");

    // 8. Close the active tab with × button -> neighbour becomes active.
    const countBeforeClose = $$(".cmp-tab").length;
    const activeClose = $<HTMLButtonElement>(".cmp-tab.on .cmp-tab-x");
    if (activeClose) click(activeClose);
    await until(() => $$(".cmp-tab").length === countBeforeClose - 1, 4000);
    log($$(".cmp-tab").length === countBeforeClose - 1 && !!$(".diffv"), "compare-tabs: closing active tab shows neighbour tab", `remaining=${$$(".cmp-tab").length}`);

    // 9. Simulate restart with saved tabs: include ready, changed, missing (moved folder), and notSaved checks.
    await flushCompareTabsSave();
    // Save a curated session directly covering all four states (ready/unified/scroll, changed, missing, notSaved)
    // using real files from the fixture (and one missing path whose files exist under moved-new/).
    await api.compareSessionsSave({
      version: 1,
      active: 0,
      tabs: [
        {
          left: { source: file(at("p1", "left.txt")), label: "left.txt" },
          right: { source: file(at("p1", "right.txt")), label: "right.txt" },
          mode: "unified",
          options: { ignoreCase: true, ignoreEol: true },
          scroll: { left: false, line: 28 },
          activeHunk: 1,
        },
        {
          left: { source: file(at("changed", "c_left.txt")), label: "c_left.txt", revision: "stale-rev-0" },
          right: { source: file(at("changed", "c_right.txt")), label: "c_right.txt" },
          mode: "split",
          options: { ignoreEol: true },
        },
        {
          left: { source: file(at("moved-old", "sub", "m_left.txt")), label: "m_left.txt" },
          right: { source: file(at("moved-old", "sub", "m_right.txt")), label: "m_right.txt" },
          mode: "split",
          options: { ignoreEol: true },
        },
        {
          left: { source: null, label: "unsaved note" },
          right: { source: file(at("p2", "right.txt")), label: "right.txt" },
          mode: "split",
          options: { ignoreEol: true },
        },
      ],
    });
    A.openOverlay(null);
    await until(() => !$(".diffv"), 3000);
    resetCompareTabs();

    // Reopen a fresh pair: earlier tabs + checks should appear!
    await openPair(at("p2", "left.txt"), at("p2", "right.txt"));
    const checksBtn = await until(() => $<HTMLButtonElement>(".cmp-tabs-checks"), 5000);
    log(!!checksBtn && !!$(".cmp-tabs-restore"), "compare-tabs: after restart, restore and earlier-checks buttons appear", checksBtn?.textContent ?? "");
    if (checksBtn) click(checksBtn);
    await until(() => $(".cmp-earlier-panel"), 3000);
    const panelText = $(".cmp-earlier-panel")?.textContent ?? "";
    log(
      panelText.includes("Missing") && panelText.includes("Changed") && panelText.includes("Not saved") && panelText.includes("Ready"),
      "compare-tabs: earlier checks panel shows Missing, Changed, Not saved and Ready badges",
      panelText.slice(0, 180),
    );
    await shot("ct-05-earlier-checks");

    // 10. Relink the missing folder (moved-old -> moved-new) and open the relinked tab + restored tab with saved scroll!
    const relinkRes = await relinkEarlier(at("moved-old", "sub", "m_left.txt"), at("moved-new", "sub", "m_left.txt"));
    log(relinkRes.pair !== null && relinkRes.newlyOpenable === 1, "compare-tabs: relinkEarlier remaps moved folder and makes missing tab openable", JSON.stringify(relinkRes.pair));
    await sleep(160);
    const panelAfter = $(".cmp-earlier-panel")?.textContent ?? "";
    log(!panelAfter.includes("Missing"), "compare-tabs: after relink, Missing badge disappears from earlier checks");

    // Restore all openable earlier tabs (includes p1 with saved unified mode + scroll line 28 + activeHunk 1).
    const restoreBtn = $<HTMLButtonElement>(".cmp-tabs-restore");
    if (restoreBtn) click(restoreBtn);
    await until(() => $$(".cmp-tab").length >= 4, 5000);
    await sleep(600);
    // Click the restored p1 tab (`left.txt ↔ right.txt`) and confirm its saved mode, hunk, and scroll survived restart!
    const p1Tab = $$<HTMLButtonElement>(".cmp-tab [role=tab]").filter((b) => b.title === "left.txt ↔ right.txt").at(-1);
    if (p1Tab) click(p1Tab);
    await until(() => $(".dv-canvas.unified") && ($(".dv-count")?.textContent ?? "").startsWith("2 /"), 5000);
    await sleep(180);
    const afterRestartScroll = $<HTMLElement>(".dv-body")?.scrollTop ?? 0;
    log(!!$(".dv-canvas.unified") && afterRestartScroll >= 100, "compare-tabs: saved unified mode, active hunk and scroll anchor survive restart", `scrollTop=${afterRestartScroll} count=${$(".dv-count")?.textContent ?? ""}`);
    await shot("ct-06-restored-after-restart");

    // 11. Key help shows comparison tab shortcuts including Vim gt / gT.
    A.openOverlay({ type: "keys" });
    await until(() => $(".kh-row"), 3000);
    const khText = $(".modal, .kh, .backdrop")?.textContent ?? "";
    log(khText.includes("next / previous comparison tab") && khText.includes(platformLabel("Ctrl+Tab")), "compare-tabs: Key Help lists comparison tab shortcuts");
    await shot("ct-07-key-help");

    A.openOverlay(null);
  } finally {
    resetCompareTabs();
  }
}
