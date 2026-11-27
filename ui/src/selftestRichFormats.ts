// Graphical self-test for WN-131:
// 1. Image adapter parity (disk DiffView vs Git GitImageDiff) for unequal dimensions, alpha, and limits,
//    plus DiffView image-mode footer / keyboard hints and SVG image <-> text toggle.
// 2. Hex view on a 4 GiB sparse pair: fast open without whole-file transfer, chunked reads,
//    dropped stale reads on fast scroll, bounded cache, and 60 Hz scroll budget.
// 3. Table view on a 50,000-row CSV pair (T01 scale): 60 Hz scroll budget and mounted row count.
import { join, parent } from "./core/path";
import * as A from "./state/actions";
import type { gitSelftest } from "./selftest";

type Helpers = typeof gitSelftest;

const frame = () => new Promise<number>((resolve) => requestAnimationFrame(resolve));
const percentile = (values: number[], fraction: number) => {
  const ordered = values.slice().sort((a, b) => a - b);
  return ordered[Math.max(0, Math.ceil(ordered.length * fraction) - 1)] ?? 0;
};

export async function runRichFormatsSelfTest(h: Helpers, rootDir: string) {
  const { $, $$, key, click, until, sleep, shot, log, api } = h;
  const fixtureDir = join(parent(rootDir) ?? rootDir, "rich-formats");
  const leftPng = join(fixtureDir, "left.png");
  const rightPng = join(fixtureDir, "right.png");
  const leftSvg = join(fixtureDir, "left.svg");
  const rightSvg = join(fixtureDir, "right.svg");
  const gitImgRepo = join(fixtureDir, "git-img");
  const t01BinA = join(fixtureDir, "t01-a.bin");
  const t01BinB = join(fixtureDir, "t01-b.bin");
  const sparseA = join(fixtureDir, "sparse-a.bin");
  const sparseB = join(fixtureDir, "sparse-b.bin");
  const tableA = join(fixtureDir, "table-a.csv");
  const tableB = join(fixtureDir, "table-b.csv");

  const closeOverlay = async () => {
    A.openOverlay(null);
    await until(() => !$(".diffv") && !$(".git-ws"), 3000);
    await sleep(80);
  };

  try {
    await h.updateSettings({ theme: "dark" });
    await h.setWindowSize(1320, 900);
    // ── 1. Disk image comparison (unequal dimensions + alpha + image-only footer & keys) ──
    localStorage.setItem("imgcmp:mode", "side");
    A.openOverlay({ type: "diff", a: { kind: "file", path: leftPng }, b: { kind: "file", path: rightPng } });
    const imgCmp = await until(() => $(".diffv .imgcmp"), 8000);
    log(!!imgCmp, "rich-formats: disk unequal RGBA PNG pair opens ImageCompare");
    const dimsReady = await until(() => {
      const txt = $(".diffv .ic-dims")?.textContent ?? "";
      return txt.includes("320×240") && txt.includes("420×280") && txt.includes("sizes differ");
    }, 6000);
    log(!!dimsReady, "rich-formats: disk image shows natural dimensions and sizes-differ warning", $(".diffv .ic-dims")?.textContent ?? "");

    const footText = ($(".diffv .dv-foot")?.textContent ?? "").toLowerCase();
    const hasImageHints = footText.includes("mode") && footText.includes("divider") && footText.includes("swap") && footText.includes("close");
    const hasTextOnlyHints = ["whitespace", "structural", "fold", "wrap", "outline", "explain", "copy patch"].some((w) => footText.includes(w));
    log(hasImageHints && !hasTextOnlyHints, "rich-formats: PNG image footer advertises only image shortcuts (no text-only hints)", footText);

    // Text-only shortcuts must be ignored in image mode.
    for (const k of ["w", "t", "z", "l", "u", "o"]) key(k, {}, $(".diffv"));
    await sleep(120);
    log(!!$(".diffv .imgcmp") && !$(".dv-outline") && !$(".dv-body"), "rich-formats: text-only shortcuts do not disrupt PNG image mode");

    // Swipe mode: shared pixel scale aligned at top-left.
    key("2", {}, $(".diffv"));
    const aligned = await until(() => {
      const imgs = $$<HTMLImageElement>(".diffv .ic-frame img.ic-aligned");
      return imgs.length === 2 ? imgs : null;
    }, 4000);
    const leftW = aligned ? parseFloat(aligned[0].style.width) : 0;
    const rightW = aligned ? parseFloat(aligned[1].style.width) : 0;
    log(!!aligned && Math.abs(leftW - (320 / 420) * 100) < 0.2 && Math.abs(rightW - 100) < 0.2,
      "rich-formats: disk Swipe aligns unequal images onto one pixel scale",
      `leftW=${leftW.toFixed(2)}% rightW=${rightW.toFixed(2)}%`);
    key("]", {}, $(".diffv"));
    await sleep(80);
    const divLeft = $(".diffv .ic-divider")?.getAttribute("style") ?? "";
    log(divLeft.includes("55%"), "rich-formats: ] moves Swipe divider by 5%", divLeft);
    await shot("rich-disk-image-swipe");
    h.note("SCREENSHOT rich-disk-image-swipe.png: unequal RGBA PNG pair (320x240 vs 420x280) in Swipe mode with alpha checkerboard, aligned scale and image-only footer.");

    // Difference mode: pixel stats + overlap text.
    key("4", {}, $(".diffv"));
    const diskDiffStat = await until(() => $(".diffv .ic-stat")?.textContent?.trim(), 10000);
    const diskDiffImg = await until(() => {
      const im = $<HTMLImageElement>(".diffv .ic-stage img[alt='difference']");
      return im && im.complete && im.naturalWidth > 0 ? im : null;
    }, 8000);
    log(!!diskDiffStat && diskDiffStat.includes("320×240 overlap") && !!diskDiffImg,
      "rich-formats: disk Difference mode reports changed pixels and overlap region",
      diskDiffStat ?? "missing");
    await shot("rich-disk-image-diff");
    h.note("SCREENSHOT rich-disk-image-diff.png: disk RGBA pixel difference view showing overlap summary and image-only footer.");
    await closeOverlay();

    // ── 2. SVG pair: image hints in image mode, text hints restored in text mode ──
    A.openOverlay({ type: "diff", a: { kind: "file", path: leftSvg }, b: { kind: "file", path: rightSvg } });
    await until(() => $(".diffv .imgcmp"), 6000);
    const svgImgFoot = ($(".diffv .dv-foot")?.textContent ?? "").toLowerCase();
    log(svgImgFoot.includes("mode") && !svgImgFoot.includes("whitespace") && !svgImgFoot.includes("structural"),
      "rich-formats: SVG in image mode shows image-only footer", svgImgFoot);
    const showTextBtn = $$<HTMLButtonElement>(".diffv button").find((b) => b.textContent?.includes("Show as text"));
    if (!showTextBtn) throw new Error("Missing Show as text button for SVG pair");
    click(showTextBtn);
    await until(() => $(".diffv .dv-body"), 6000);
    const svgTextFoot = ($(".diffv .dv-foot")?.textContent ?? "").toLowerCase();
    log(svgTextFoot.includes("whitespace") && svgTextFoot.includes("structural") && svgTextFoot.includes("fold") && svgTextFoot.includes("wrap"),
      "rich-formats: SVG switched back to text regains text shortcut hints", svgTextFoot);
    await closeOverlay();

    // ── 3. Git image adapter parity (GitImageDiff on the exact same RGBA PNG pair) ──
    localStorage.setItem("imgcmp:mode", "swipe");
    await A.openGitWorkspace(gitImgRepo);
    await until(() => $(".git-ws"), 8000);
    const workingNode = $(".gg-working");
    if (workingNode) click(workingNode);
    const pngRow = await until(() => $$(".gc-file").find((r) => r.textContent?.includes("sample.png")), 8000);
    log(!!pngRow, "rich-formats: Git workspace lists changed sample.png");
    if (pngRow) {
      click(pngRow);
      const gitImgCmp = await until(() => $(".gid .imgcmp"), 8000);
      const gitDims = await until(() => {
        const txt = $(".gid .ic-dims")?.textContent ?? "";
        return txt.includes("320×240") && txt.includes("420×280") && txt.includes("sizes differ");
      }, 6000);
      const gitAligned = await until(() => {
        const imgs = $$<HTMLImageElement>(".gid .ic-frame img.ic-aligned");
        return imgs.length === 2 ? imgs : null;
      }, 6000);
      const gitLeftW = gitAligned ? parseFloat(gitAligned[0].style.width) : 0;
      const gitRightW = gitAligned ? parseFloat(gitAligned[1].style.width) : 0;
      log(!!gitImgCmp && !!gitDims && !!gitAligned && Math.abs(gitLeftW - leftW) < 0.2 && Math.abs(gitRightW - rightW) < 0.2,
        "rich-formats: Git image adapter matches disk adapter scaling and dimensions",
        `gitLeftW=${gitLeftW.toFixed(2)}% gitRightW=${gitRightW.toFixed(2)}%`);
      await shot("rich-git-image-swipe");
      h.note("SCREENSHOT rich-git-image-swipe.png: Git workspace GitImageDiff on the same unequal RGBA PNG pair in Swipe mode.");

      // Switch Git image adapter to Difference mode and compare summary parity with disk adapter.
      const gitDiffBtn = $$<HTMLButtonElement>(".gid .ic-bar .seg button").find((b) => b.textContent === "Difference");
      if (gitDiffBtn) click(gitDiffBtn);
      const gitDiffStat = await until(() => $(".gid .ic-stat")?.textContent?.trim(), 10000);
      log(!!gitDiffStat && gitDiffStat === diskDiffStat,
        "rich-formats: Git image Difference summary matches disk adapter identically",
        `disk="${diskDiffStat ?? ""}" git="${gitDiffStat ?? ""}"`);
    }
    await closeOverlay();

    // ── 4. Hex view: 4 GiB sparse pair, chunked reads, stale-read drop, 60 Hz scroll budget ──
    const readBytesLog: { offset: number; len: number }[] = [];
    const origReadBytes = api.readBytes;
    api.readBytes = (path: string, offset: number, len: number) => {
      readBytesLog.push({ offset, len });
      return origReadBytes(path, offset, len);
    };
    try {
      const hexOpenStart = performance.now();
      A.openOverlay({ type: "diff", a: { kind: "file", path: sparseA }, b: { kind: "file", path: sparseB } });
      const hxBody = await until(() => $<HTMLElement>(".diffv .hx-body"), 15000);
      if (!hxBody) throw new Error("HexDiff did not open for 4 GiB sparse pair");
      const hexOpenMs = performance.now() - hexOpenStart;
      const hexLoaded = await until(() => {
        const text = $(".diffv .hx-verdict")?.textContent ?? "";
        const anyHex = $$(".diffv .hx-hex").some((el) => /[0-9A-F]{2}/.test(el.textContent ?? ""));
        return text.includes("differ") && anyHex;
      }, 10000);
      const maxReadLen = Math.max(0, ...readBytesLog.map((r) => r.len));
      log(!!hexLoaded && maxReadLen > 0 && maxReadLen <= 16 * 1024 && hexOpenMs < 5000,
        "rich-formats: 4 GiB sparse binary pair opens quickly with chunked <=16 KiB reads",
        `open=${hexOpenMs.toFixed(1)} ms reads=${readBytesLog.length} maxChunk=${maxReadLen} bytes`);

      // Verify consecutive rows on a 4 GiB scaled file never stack on top of each other.
      const initialRows = $$<HTMLElement>(".diffv .hx-row");
      const rowDelta = initialRows.length >= 2
        ? initialRows[1].getBoundingClientRect().top - initialRows[0].getBoundingClientRect().top
        : 0;
      log(rowDelta >= 14, "rich-formats: 4 GiB scaled HexDiff rows do not stack vertically", `rowDelta=${rowDelta.toFixed(1)}px`);

      // Rapid jumps across the 4 GiB file to verify stale reads are dropped and the cache stays bounded.
      // Delay intermediate chunk responses briefly so fast scrolling reliably overtakes in-flight reads
      // even when the OS page cache serves sparse reads in sub-millisecond time.
      const maxScroll = Math.max(1000, hxBody.scrollHeight - hxBody.clientHeight);
      api.readBytes = (path: string, offset: number, len: number) => {
        readBytesLog.push({ offset, len });
        return new Promise((resolve, reject) => {
          setTimeout(() => { origReadBytes(path, offset, len).then(resolve, reject); }, 60);
        });
      };
      for (let i = 0; i < 40; i++) {
        hxBody.scrollTop = Math.floor(((i + 1) / 41) * maxScroll);
        await frame();
      }
      await sleep(160);
      api.readBytes = (path: string, offset: number, len: number) => {
        readBytesLog.push({ offset, len });
        return origReadBytes(path, offset, len);
      };
      const hxRoot = $(".diffv .hx");
      const cachedBlocks = Number(hxRoot?.getAttribute("data-cached-blocks") ?? "0");
      const droppedReads = Number(hxRoot?.getAttribute("data-read-dropped") ?? "0");
      const startedReads = Number(hxRoot?.getAttribute("data-read-started") ?? "0");
      log(cachedBlocks <= 64 && droppedReads > 0,
        "rich-formats: HexDiff keeps block cache bounded (<=64) and drops stale reads on fast scroll",
        `cached=${cachedBlocks} started=${startedReads} dropped=${droppedReads}`);

      // Jump to the 4 GiB end via offset form and verify tail bytes render cleanly.
      const gotoInput = $<HTMLInputElement>(".diffv .hx-goto input");
      const gotoBtn = $<HTMLButtonElement>(".diffv .hx-goto button[type='submit']");
      if (gotoInput && gotoBtn) {
        h.typeInto(gotoInput, "end");
        click(gotoBtn);
        const tailVisible = await until(() => $$(".diffv .hx-txt").some((el) => el.textContent?.includes("SPARSE-TAIL")), 5000);
        log(!!tailVisible, "rich-formats: jumping to end on 4 GiB sparse pair renders tail bytes");
      }
      await shot("rich-hex-4gib-sparse");
      h.note("SCREENSHOT rich-hex-4gib-sparse.png: HexDiff on 4 GiB sparse binary pair after jumping near the end.");
    } finally {
      api.readBytes = origReadBytes;
      await closeOverlay();
    }

    // Shared 60 Hz frame-budget helper (matching selftestStandaloneMerge's Wayland presentation trace).
    const measureScrollBudget = async (
      scroller: HTMLElement,
      rowSel: string,
      label: string,
      stepPx = 96,
    ) => {
      let idleTs = await frame();
      const idleIntervals: number[] = [];
      for (let s = 0; s < 30; s++) {
        const now = await frame();
        idleIntervals.push(now - idleTs);
        idleTs = now;
      }
      const idleP50 = percentile(idleIntervals, 0.5);
      const idleP95 = percentile(idleIntervals, 0.95);
      const warmupFrames = 30;
      const measuredFrames = 120;
      let maxRows = 0;
      const workTimes: number[] = [];
      const scrollLoop = async (first: number, count: number, recordWork: boolean) => {
        let last = await frame();
        const intervals: number[] = [];
        for (let i = first; i < first + count; i++) {
          const t0 = performance.now();
          scroller.scrollTop = i * stepPx;
          if (recordWork) workTimes.push(performance.now() - t0);
          const now = await frame();
          maxRows = Math.max(maxRows, scroller.querySelectorAll(rowSel).length);
          intervals.push(now - last);
          last = now;
        }
        return intervals;
      };
      await scrollLoop(0, warmupFrames, false);
      const recording = await api.selftestFrameTrace(true);
      let frames: Awaited<ReturnType<typeof api.selftestFrameTrace>> = null;
      let intervals: number[];
      try {
        intervals = await scrollLoop(warmupFrames, measuredFrames, true);
      } finally {
        if (recording !== null) frames = await api.selftestFrameTrace(false);
      }
      const webP50 = percentile(intervals, 0.5);
      const webP95 = percentile(intervals, 0.95);
      const workP95 = percentile(workTimes, 0.95);
      log(maxRows > 0 && maxRows <= 80, `${label}: mounted row count stays bounded`, `maxRows=${maxRows}`);
      if (frames === null) {
        log(webP95 <= 17.0, `${label}: 60 Hz scroll budget`,
          `WebKit p50=${webP50.toFixed(2)} ms p95=${webP95.toFixed(2)} ms (idle p50=${idleP50.toFixed(2)} p95=${idleP95.toFixed(2)} ms), work p95=${workP95.toFixed(2)} ms, maxRows=${maxRows}`);
      } else {
        const presented = [...new Set(frames.map(([timestamp]) => timestamp))].sort((a, b) => a - b);
        const presentedIntervals = presented.slice(1).map((timestamp, i) => (timestamp - presented[i]) / 1e6);
        const p50 = presentedIntervals.length ? percentile(presentedIntervals, 0.5) : Infinity;
        const p95 = presentedIntervals.length ? percentile(presentedIntervals, 0.95) : Infinity;
        const refresh = frames[0]?.[1] / 1e6;
        const onReference = Math.abs(refresh - 1000 / 60) <= 0.05 && frames.every(([, period]) => period === frames[0][1]);
        log(onReference && presented.length >= Math.ceil(measuredFrames * 0.95) && p95 <= 16.7,
          `${label}: 60 Hz scroll budget`,
          `presented p50=${p50.toFixed(3)} ms p95=${p95.toFixed(3)} ms, ${presented.length}/${measuredFrames} frames at ${(1000 / refresh).toFixed(2)} Hz, WebKit p50=${webP50.toFixed(2)} ms p95=${webP95.toFixed(2)} ms (idle p50=${idleP50.toFixed(2)} p95=${idleP95.toFixed(2)} ms), work p95=${workP95.toFixed(2)} ms, maxRows=${maxRows}`);
      }
    };

    // ── 4b. Hex view 60 Hz scroll budget on T01-scale 4 MiB binary pair (262,144 rows) ──
    A.openOverlay({ type: "diff", a: { kind: "file", path: t01BinA }, b: { kind: "file", path: t01BinB } });
    const t01HxBody = await until(() => $<HTMLElement>(".diffv .hx-body"), 15000);
    if (!t01HxBody) throw new Error("HexDiff did not open for 4 MiB T01 binary pair");
    await until(() => $$(".diffv .hx-hex").some((el) => /[0-9A-F]{2}/.test(el.textContent ?? "")), 10000);
    await measureScrollBudget(t01HxBody, ".hx-row", "rich-formats: HexDiff 4 MiB (T01 scale)");
    await closeOverlay();

    // ── 5. Table view: 50,000-row CSV pair (T01 scale), 60 Hz scroll budget ──
    const tblOpenStart = performance.now();
    A.openOverlay({ type: "diff", a: { kind: "file", path: tableA }, b: { kind: "file", path: tableB } });
    const tblBody = await until(() => $<HTMLElement>(".diffv .tb-body"), 20000);
    if (!tblBody) throw new Error("TableDiff did not open for 50k CSV pair");
    const tblOpenMs = performance.now() - tblOpenStart;
    // Toggle "Changes only" off so all 50,000 rows are scrollable.
    const changesBtn = $$<HTMLButtonElement>(".diffv .tb-bar button").find((b) => b.textContent?.includes("Changes only"));
    if (changesBtn && changesBtn.classList.contains("on")) {
      click(changesBtn);
      await until(() => (tblBody.querySelector<HTMLElement>(".tb-rows")?.offsetHeight ?? 0) > 500_000, 8000);
    }
    const tblFoot = ($(".diffv .dv-foot")?.textContent ?? "").toLowerCase();
    log(tblFoot.includes("changes only") && !tblFoot.includes("whitespace"),
      "rich-formats: TableDiff footer shows table-specific hints", tblFoot);
    h.note(`rich-formats: TableDiff 50,000-row CSV opened in ${tblOpenMs.toFixed(1)} ms`);
    await measureScrollBudget(tblBody, ".tb-row", "rich-formats: TableDiff 50,000-row CSV (T01 scale)");
    await shot("rich-table-50k");
    h.note("SCREENSHOT rich-table-50k.png: TableDiff on 50,000-row CSV pair with virtualized rows and table-specific footer.");
    await closeOverlay();
  } catch (e) {
    log(false, "rich-formats scenario", e instanceof Error ? e.stack ?? e.message : String(e));
    await shot("rich-formats-failure");
  }
}
