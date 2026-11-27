import type { gitSelftest } from "./selftest";
import { paneFrameDevicePx } from "./core/pixelGrid";
import * as A from "./state/actions";

/**
 * Sharpness of icons, borders and rounded corners at several webview scales. Each scale is
 * captured in the dark, light and Omarchy themes; the checks measure what blurs in practice:
 * icons and boxes that do not sit on whole device pixels, emblem detail that is smaller than a
 * device pixel, and borders with a gap at the rounded corners.
 */
export async function runRenderingSelfTest(t: typeof gitSelftest) {
  const { $$, until, sleep, shot, setWindowSize, log, updateSettings, focusPane, finish } = t;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  const errors: string[] = [];
  window.addEventListener("error", (e) => errors.push(e.message));
  window.addEventListener("unhandledrejection", (e) => errors.push(String(e.reason)));
  const dpr = () => window.devicePixelRatio;
  const frac = (v: number) => Math.abs(v - Math.round(v));
  /** Elements whose box is not on a whole device pixel (they get resampled and look soft). */
  const offGrid = (sel: string) => $$(sel).filter((el) => {
    const r = el.getBoundingClientRect();
    if (!r.width) return false;
    const d = dpr();
    return frac(r.left * d) > 0.02 || frac(r.top * d) > 0.02;
  });
  const rectInfo = (el: Element) => {
    const r = el.getBoundingClientRect();
    const d = dpr();
    return `(${(r.left * d).toFixed(2)},${(r.top * d).toFixed(2)} ${(r.width * d).toFixed(2)}x${(r.height * d).toFixed(2)})`;
  };
  /**
   * The focus frame of the Git commit list is whole: the list scrolls under a sticky column header, and an inset
   * outline was painted under that header and the rows, so only its left edge and a piece beside the scrollbar showed.
   * The frame must be a border of the device-pixel pane-frame width on all four sides; the screenshot shows it whole.
   */
  const gitFocusFrame = async (tag: string) => {
    A.openOverlay({ type: "gitGraph", path: `${t.activePath(0)}/Projects` });
    try {
      // The first opening in a run is cold (the history is read from a fresh folder); 3 s failed once at `dark-dpr1`.
      const lines = await until(() => t.$(".git-graph-lines .gg-row") && t.$<HTMLElement>(".git-graph-lines"), 15000);
      if (!lines) { log(false, `${tag}: the Git commit list opens for the focus frame check`); return; }
      lines.focus();
      await until(() => t.$(".gg-focus-history"));
      await sleep(150);
      // A border lies outside the scrolled content, which the list clips to its padding box, so the header, rows and
      // scrollbars cannot cover it; an outline drawn inside the box can be covered.
      const cs = getComputedStyle(lines), sides = ["top", "right", "bottom", "left"] as const;
      log(cs.outlineStyle === "none" || parseFloat(cs.outlineWidth) === 0, `${tag}: the Git commit list has no inset outline that its content can cover`, `${cs.outlineStyle} ${cs.outlineWidth} offset ${cs.outlineOffset}`);
      const widths = sides.map((s) => Math.round(parseFloat(cs.getPropertyValue(`border-${s}-width`)) * dpr() * 100) / 100);
      const colors = new Set(sides.map((s) => cs.getPropertyValue(`border-${s}-color`)));
      const want = paneFrameDevicePx(dpr()), nominal = Math.round(2 * dpr() * 100) / 100; // WebKit reports the nominal width at fractional zoom and snaps it when painting, as for the pane frame
      log(widths.every((w) => w === widths[0] && (w === want || w === nominal)) && colors.size === 1 && !/rgba\([^)]*,\s*0\)|transparent/.test([...colors][0]), `${tag}: the Git commit list focus frame is ${want} device px and coloured on all four sides`, `${widths.join("/")} ${[...colors].join(" | ")}`);
      await shot(`rendering-git-focus-${tag}`);
      t.note(`SCREENSHOT rendering-git-focus-${tag}.png: expected an unbroken accent frame around the commit list, its top edge above the column header.`);
    } finally {
      A.openOverlay(null);
      await until(() => !t.$(".git-graph"));
    }
  };
  try {
    await getCurrentWindow().unmaximize();
    await setWindowSize(1100, 760);
    await updateSettings({ keyboardProfile: "explorer", shortcuts: {}, adaptiveLayout: true, showNavColumns: false });
    await focusPane(0);
    for (const theme of ["dark", "light"] as const) {
      await updateSettings({ theme });
      // Webview zoom scales devicePixelRatio. The zoom for each wanted scale comes from the scale this
      // display really has (a 2x display and a 1.5x laptop need different zooms for 1 / 1.25 / 1.5 / 2).
      await getCurrentWebview().setZoom(1).catch(() => {});
      await sleep(300);
      const baseDpr = dpr();
      for (const target of [1, 1.25, 1.5, 2]) {
        const zoom = target / baseDpr;
        await getCurrentWebview().setZoom(zoom).catch((e) => log(false, `setZoom ${zoom}`, String(e)));
        await sleep(400);
        await until(() => $$(".ficon").length > 4);
        const tag = `${theme}-dpr${String(Math.round(dpr() * 100) / 100).replace(".", "_")}`;
        log(Math.abs(dpr() - target) < 0.02, `${tag}: the display scale is ${target}`, `devicePixelRatio ${dpr()}, base ${baseDpr}`);
        // The big icon of the preview panel is centred by layout and can sit on a half pixel; the browser snaps an SVG or
        // image to whole device pixels when it paints it (checked on the screenshots at every scale), so it is not held to the grid.
        const icons = offGrid(".ficon svg, .ficon img").filter((el) => !el.closest(".pv-icon"));
        // The sidebar spacing is snapped to device pixels (`--sp-N`), so its rows and icons must sit
        // on whole device pixels at every scale, 125 % and 150 % included.
        const strict = true;
        log(!strict || icons.length === 0, `${tag}: file and folder icons sit on whole device pixels`, `${icons.length}/${$$(".ficon svg, .ficon img").length} off-grid at dpr ${dpr().toFixed(2)}: ${icons.slice(0, 4).map(rectInfo).join(" ")}`);
        // Splitter grips are centred by layout, not by a transform, so their borders stay crisp.
        const grips = offGrid(".splitter .grip button");
        log(grips.length === 0, `${tag}: splitter grip buttons sit on whole device pixels`, `${grips.length} off-grid: ${grips.slice(0, 4).map(rectInfo).join(" ")}`);
        // The second pane starts where the draggable split ratio puts it, so only the sidebar and the
        // first pane are held to the grid (the live system monitor block is sized by its meters).
        const firstBrowser = $$(".browser")[0];
        const boxes = offGrid(".group-head, .fav, .drive, .search-pill, .tabbar, .fp-head").filter((el) => { const b = el.closest(".browser"); return (!b || b === firstBrowser) && !el.closest(".sysmon"); });
        log(!strict || boxes.length === 0, `${tag}: sidebar rows and bars sit on whole device pixels`, `${boxes.length} off-grid: ${boxes.slice(0, 40).map((el) => `${el.className.toString().split(" ")[0]}@${rectInfo(el)}`).join(" ")}`);
        { const chain: string[] = []; for (let el = icons[0]?.parentElement as Element | null; el && el !== document.body; el = el.parentElement) chain.push(`${el.className.toString().split(" ")[0] || el.tagName}:${rectInfo(el)}`); log(true, `${tag}: ancestors of the first off-grid icon`, chain.join(" < ")); }
        // The frame of the active pane: the same whole number of device pixels on all four sides, so the
        // rounded corners are as heavy as the straight edges, and nothing is drawn over it at the corner.
        const active = $$(".browser.active")[0] as HTMLElement | undefined;
        if (active) {
          const cs = getComputedStyle(active);
          const sides = (["top", "right", "bottom", "left"] as const).map((s) => Math.round(parseFloat(cs.getPropertyValue(`border-${s}-width`)) * dpr() * 100) / 100);
          const logical = 2 * target;
          const snapped = Math.round(logical);
          // WebKit can report the nominal CSS width at fractional zoom and snap it when painting.
          const width = sides[0];
          log(sides.every((w) => w === width) && (width === snapped || width === logical),
            `${tag}: the 2 CSS px pane frame is even on all four sides`, sides.join("/"));
          log(getComputedStyle(active, "::before").content === "none", `${tag}: no extra edge is drawn over the pane frame`);
          const tab = active.querySelector(".tab.active");
          const ring = tab && /([\d.]+)px\s+inset/.exec(getComputedStyle(tab).boxShadow);
          log(!!ring && Math.floor(parseFloat(ring[1]) * dpr() + 1e-6) === 1 && parseFloat(ring[1]) * dpr() < 1.1, `${tag}: the active tab ring is one device pixel`, ring ? `${(parseFloat(ring[1]) * dpr()).toFixed(3)} device px` : "no ring");
        }
        // Folder emblems stay inside the icon's box and do not reach past the folder on the right, and the
        // gap between icon and name is the same on every row.
        const withEmblem = $$(".ficon-folder").filter((svg) => svg.querySelector("g[data-emblem]") && svg.getBoundingClientRect().width > 0); // hidden panes have none
        const protruding = withEmblem.filter((svg) => {
          const box = svg.getBoundingClientRect();
          const mark = svg.querySelector("g[data-emblem] > g:not(.fe-halo)")!.getBoundingClientRect();
          const body = svg.querySelector(".ff")!.getBoundingClientRect();
          const k = Number(/scale\((\d+)\)/.exec(svg.querySelector("g[data-emblem]")!.getAttribute("transform") ?? "")?.[1] ?? 1);
          const half = (0.5 * k) / dpr(); // the strokes are centred on the outline of the glyph
          return mark.right + half > body.right + 0.01 || mark.bottom + half > box.bottom + 0.01 || mark.left < box.left - 0.01 || mark.top < box.top - 0.01;
        });
        const why = (svg: Element) => { const box = svg.getBoundingClientRect(); const mark = svg.querySelector("g[data-emblem] > g:not(.fe-halo)")!.getBoundingClientRect(); const body = svg.querySelector(".ff")!.getBoundingClientRect(); const d = dpr(); return `${svg.parentElement?.parentElement?.className.toString().split(" ")[0]} box ${(box.width * d).toFixed(1)}px emblem ${svg.querySelector("g[data-emblem]")!.getAttribute("transform")} dRight ${((mark.right - body.right) * d).toFixed(2)} dBottom ${((mark.bottom - box.bottom) * d).toFixed(2)} dLeft ${((mark.left - box.left) * d).toFixed(2)} dTop ${((mark.top - box.top) * d).toFixed(2)}`; };
        log(withEmblem.length > 0 && protruding.length === 0, `${tag}: folder emblems stay inside the icon and the folder's right edge`, `${protruding.length}/${withEmblem.length} protrude: ${protruding.slice(0, 3).map(why).join(" | ")}`);
        const gaps = $$(".fp-list [data-path]", firstBrowser ?? document).map((row) => {
          const ic = row.querySelector(".ficon");
          const name = row.querySelector(".txt");
          return ic && name ? Math.round((name.getBoundingClientRect().left - ic.getBoundingClientRect().right) * dpr() * 100) / 100 : NaN;
        }).filter(Number.isFinite);
        log(gaps.length > 4 && new Set(gaps).size === 1, `${tag}: the gap between icon and name is the same on every row`, `${[...new Set(gaps)].join(", ")} device px over ${gaps.length} rows`);
        // A list row's emblem is drawn at a whole number of device pixels per unit, so it is never smaller than
        // the Catppuccin art at 100 %: at least one device pixel per unit of its own glyph. The first emblem is not
        // always git (6.5 units): in a linked worktree `.git` is a file, so the first folder with an emblem is another
        // one (6.0 units), which a fixed 6.4 device px threshold wrongly failed at 100 % and 125 %.
        const emblem = withEmblem.find((svg) => firstBrowser?.contains(svg))?.querySelector("g[data-emblem] > g:not(.fe-halo)");
        const emblemDev = emblem ? emblem.getBoundingClientRect().width * dpr() : 0;
        const emblemUnits = emblem instanceof SVGGraphicsElement ? emblem.getBBox().width : 0;
        log(!!emblem && emblemUnits > 0 && emblemDev >= emblemUnits - 0.05, `${tag}: a list folder emblem is at least as large as the 16 px Catppuccin art`, `${emblemDev.toFixed(1)} device px for ${emblemUnits.toFixed(1)} units`);
        log(true, `${tag}: first pane bars`, [".fp-head", ".fp-nav", ".fp-hrow", ".fp-list"].map((q) => { const el = firstBrowser?.querySelector(q); return `${q}:${el ? rectInfo(el) : "none"}`; }).join(" "));
        log(true, `${tag}: off-grid icon parents`, icons.map((el) => `${el.parentElement?.parentElement?.className}`).join(" | "));
        await shot(`rendering-${tag}`);
        t.note(`SCREENSHOT rendering-${tag}.png: expected crisp hairlines, folder icons with legible emblems, and a continuous pane frame.`);
        await gitFocusFrame(tag);
      }
      await getCurrentWebview().setZoom(1).catch(() => {});
    }
  } catch (e) {
    log(false, "rendering scenarios", String(e));
  }
  log(!errors.length, "no uncaught rendering errors", errors.join("; "));
  await finish();
}
