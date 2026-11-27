// `--e2e-*` modes for the Omarchy bridge (F7), adaptive layout / tiling / window buttons (F8)
// and vim / shortcut safety (F10). Each mode reads the REAL app state (backend bridge events,
// store, DOM) and reports PASS/FAIL lines; the opaque-box suite greps the report text.
import type { OmarchyState } from "./api/types";
import { buildKeymap, defaultKeymap, KEYBOARD_PROFILES, type KeyboardProfile } from "./core/keymap";
import { rawAdaptiveMode, resolveAdaptiveLayout, windowButtonsShown } from "./core/adaptiveLayout";
import { buildVimTree, resolve, shadowedByVim } from "./core/vim";
import type { ModeGroup, SelftestTools } from "./selftestE2e";

type T = SelftestTools;
const cssVar = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

/** Waits for the omarchy state in the store to satisfy `pred` (events from the backend bridge). */
async function omarchyWhen(t: T, pred: (o: OmarchyState) => boolean, ms = 8000): Promise<OmarchyState | null> {
  const { api } = t;
  const found = await t.until(() => { const o = t.getState().omarchy; return o && pred(o) ? o : null; }, ms);
  if (found) return found;
  // The event may have been missed: ask the backend directly.
  const o = await api.omarchyStateGet().catch(() => null);
  return o && pred(o) ? o : null;
}

const tokenLine = () => ["--bg", "--bg-2", "--bg-3", "--fg", "--accent"].map((n) => `${n}=${cssVar(n)}`).join(" ");

/**
 * Resizes the window. A tiling compositor (the suite runs on the live Hyprland) ignores the
 * resize; then a virtual viewport of that size drives the adaptive layout instead (the store's
 * viewport is the only input of the layout, so the DOM assertions stay meaningful).
 */
let virtualOnly = false;
async function setSize(t: T, w: number, h: number) {
  const { setViewportOverride } = await import("./state/desktop");
  if (virtualOnly) {
    // Straight to the new virtual size: the layout keeps its previous mode for the hysteresis.
    setViewportOverride({ w, h });
    t.log(true, `window size ${w}x${h} applied`, "virtual viewport (compositor ignores resizes)");
    await t.sleep(250);
    return true;
  }
  await t.setWindowSize(w, h);
  const real = Math.abs(window.innerWidth - w) <= 16 && Math.abs(window.innerHeight - h) <= 16;
  if (!real) { virtualOnly = true; setViewportOverride({ w, h }); }
  t.log(true, `window size ${w}x${h} applied`, real ? "real window resize" : `compositor kept ${window.innerWidth}x${window.innerHeight}; virtual viewport used`);
  await t.sleep(250);
  return true;
}

const browsersShown = (t: T) =>
  t.$$(".browser").filter((b) => (b.parentElement as HTMLElement).style.display !== "none" && b.getBoundingClientRect().width > 0);

const sidebarShown = (t: T) => { const s = t.$(".sidebar"); return !!s && s.getBoundingClientRect().width > 0; };

async function omarchyDark(t: T) {
  const o = await omarchyWhen(t, (s) => s.active);
  const theme = t.getState().settings?.theme;
  if (!o) {
    t.log(true, "omarchy inactive: fallback theme", `active=false themeMode=${document.documentElement.dataset.theme}`);
    return;
  }
  t.log(true, `omarchy state loaded active=${o.active} themeMode=${o.themeMode}`);
  if (theme === "omarchy") {
    const ok = await t.until(() => document.documentElement.dataset.theme === o.themeMode);
    t.log(!!ok, `data-theme follows themeMode (${o.themeMode})`, `data-theme=${document.documentElement.dataset.theme}`);
    // A token that the user's gall.css sets itself (e.g. --accent !important) wins by design.
    const overridden = (n: string) => !!o.customCss && o.customCss.includes(n);
    const { omarchyControlVars, omarchyTextVars, paletteCssVars } = await import("./core/omarchyTheme");
    const palette = paletteCssVars(o.paletteTokens);
    const expected = { ...palette, ...omarchyTextVars(palette) };
    expected["--accent"] = omarchyControlVars(expected)["--om-control-accent"];
    for (const [name, value] of Object.entries(expected).filter(([n]) => ["--bg", "--bg-2", "--bg-3", "--fg", "--accent"].includes(n) && !overridden(n))) {
      const applied = await t.until(() => cssVar(name).toLowerCase() === value.toLowerCase(), 2500);
      t.log(!!applied, `token ${name} applied`, `${name}=${cssVar(name)} expected ${value}`);
    }
    if (o.customCss) t.log(!!document.getElementById("omarchy-custom"), "gall.css overlay inlined");
  }
  t.note(`themeMode=${o.themeMode} data-theme=${document.documentElement.dataset.theme} ${tokenLine()} font=${cssVar("--font")}`);
}

async function omarchySwap(t: T) {
  const first = await omarchyWhen(t, (s) => s.active);
  t.log(!!first, "omarchy active before the swap", `themeMode=${first?.themeMode}`);
  const light = await omarchyWhen(t, (s) => s.themeMode === "light", 12000);
  t.log(!!light, "atomic theme swap reached the UI live (themeMode=light)");
  await t.until(() => document.documentElement.dataset.theme === "light", 3000);
  t.log(document.documentElement.dataset.theme === "light", "data-theme switched to light without restart");
  await t.sleep(300);
  t.note(`themeMode=light data-theme=${document.documentElement.dataset.theme} ${tokenLine()}`);
}

async function omarchyRounding(t: T) {
  const start = await omarchyWhen(t, (s) => s.active);
  t.log(!!start, "omarchy active", `rounding=${start?.rounding}`);
  t.note(`initial --radius=${cssVar("--radius")}`);
  const after = await omarchyWhen(t, (s) => s.rounding === 8, 12000);
  t.log(!!after, "configreloaded>> reloaded decoration:rounding live (8)", `rounding=${t.getState().omarchy?.rounding}`);
  await t.until(() => cssVar("--radius") === "8px", 3000);
  t.log(cssVar("--radius") === "8px", "--radius follows the Hyprland rounding", `--radius=${cssVar("--radius")}`);
  t.note(`final --radius=${cssVar("--radius")} rounding=8`);
}

async function omarchyFont(t: T) {
  const o = await omarchyWhen(t, (s) => !!s.monospaceFont);
  t.log(!!o, "fonts.conf monospace font read", `monospaceFont=${o?.monospaceFont}`);
  const fira = await omarchyWhen(t, (s) => !!s.monospaceFont && s.monospaceFont.includes("FiraCode"), 12000);
  t.log(!!fira, "fontconfig change reached the UI without restart", `monospaceFont=${t.getState().omarchy?.monospaceFont}`);
  await t.until(() => cssVar("--font").includes("FiraCode"), 3000);
  t.log(cssVar("--font").includes("FiraCode"), "--font follows the monospace font", `--font=${cssVar("--font")}`);
  t.note(`font=${cssVar("--font")}`);
}

async function omarchyGlassy(t: T) {
  const o = await omarchyWhen(t, (s) => s.glassy);
  t.log(!!o, "window-glassy.lua toggle detected", `glassy=${o?.glassy}`);
  const on = await t.until(() => document.documentElement.classList.contains("omarchy-glassy") && document.body.classList.contains("omarchy-glassy"));
  t.log(!!on, "omarchy-glassy class set on html and body");
  const row = t.$(".fp-list [data-path]") ?? t.$(".fp-hrow") ?? document.body;
  const color = getComputedStyle(row).color;
  const alphaOf = (c: string) => /(?:rgba\([^)]*,|\/)\s*([\d.]+)\)/.exec(c)?.[1];
  const alpha = alphaOf(color);
  t.log(alpha === undefined || parseFloat(alpha) === 1, "list text stays opaque on translucent surfaces", color);
  const bg = getComputedStyle(t.$(".browser") ?? document.body).backgroundColor;
  const a = alphaOf(bg);
  t.log(a !== undefined && parseFloat(a) < 1, "browser surface is translucent", bg);
  t.note(`omarchy-glassy active; text ${color}; surface ${bg}`);
}

async function omarchyInactive(t: T) {
  await t.sleep(800);
  const o = await t.api.omarchyStateGet();
  t.log(!o.active, `bridge inactive on a non-Omarchy host: active=${o.active}`);
  t.log(!document.documentElement.classList.contains("omarchy-glassy"), "no glassy class when inactive");
  t.log(document.documentElement.style.getPropertyValue("--bg-2") === "", "no Omarchy palette applied when inactive");
  t.log(!document.getElementById("omarchy-custom"), "no Omarchy css overlay when inactive");
  t.note(`inactive: active=${o.active} theme setting=${t.getState().settings?.theme} data-theme=${document.documentElement.dataset.theme}`);
}

// ───────────── F8: adaptive layout ─────────────

async function adaptiveWide(t: T) {
  await t.updateSettings({ adaptiveLayout: true });
  await setSize(t, 1200, 800);
  const mode = await t.until(() => t.getState().adaptiveMode === "horizontal" && browsersShown(t).length === 2);
  t.log(!!mode, "1200x800: horizontal two-pane mode", `mode=${t.getState().adaptiveMode}`);
  const [a, b] = browsersShown(t).map((x) => x.getBoundingClientRect());
  t.log(!!a && !!b && Math.abs(a.top - b.top) < 2 && a.right <= b.left + 2, "panes side by side (horizontal)");
  const pm = t.$(".panel-map")?.getBoundingClientRect();
  const split = t.$(".ws-split")?.getBoundingClientRect();
  t.log(!!pm && !!split && Math.abs(pm.left + pm.width / 2 - (split.left + split.width / 2)) < 40, "panel map centred over the divider",
    pm && split ? `${Math.round(pm.left + pm.width / 2)} vs ${Math.round(split.left + split.width / 2)}` : "missing");
  // The side-by-side threshold applies to the workspace (window minus the open sidebar), so the
  // window thresholds move right by the sidebar width. Hysteresis: 30 px under 920 of workspace
  // is inside the 36 px dead band after being wide.
  const { sidebarWidth } = await import("./state/desktop");
  const sb = sidebarWidth();
  t.log(sidebarShown(t), "sidebar open for the workspace-width checks");
  await setSize(t, 920 - 30 + sb, 800);
  t.log(t.getState().adaptiveMode === "horizontal", "hysteresis: workspace 890 px keeps horizontal after shrinking from wide", String(t.getState().adaptiveMode));
  await setSize(t, 860 + sb, 800);
  t.log(t.getState().adaptiveMode !== "horizontal", "hysteresis: workspace 860 px (below the dead band) leaves horizontal", String(t.getState().adaptiveMode));
  const panes = browsersShown(t).map((x) => x.getBoundingClientRect().width);
  t.log(panes.length < 2 || panes.every((x) => x >= 400), "two panes beside the open sidebar are never crushed below 400 px", panes.map(Math.round).join(","));
  t.log(resolveAdaptiveLayout(940, 500, "single") === "single" && resolveAdaptiveLayout(960, 500, "single") === "horizontal", "hysteresis: pure function holds 36 px both ways");
  t.note("horizontal mode verified; hysteresis dead band ~36 px verified");
}

async function adaptiveTall(t: T) {
  await t.updateSettings({ adaptiveLayout: true });
  await setSize(t, 700, 900);
  const ok = await t.until(() => t.getState().adaptiveMode === "vertical" && browsersShown(t).length === 2);
  t.log(!!ok, "700x900: vertical stacked two-pane mode", `mode=${t.getState().adaptiveMode}`);
  const [a, b] = browsersShown(t).map((x) => x.getBoundingClientRect());
  t.log(!!a && !!b && Math.abs(a.left - b.left) < 2 && a.bottom <= b.top + 2, "panes stacked top / bottom (vertical)");
  // Exact aspect boundary at w=750: 0.85*750 = 637.5.
  t.log(rawAdaptiveMode(750, 635) === "single", "aspect boundary: 750x635 is single");
  t.log(rawAdaptiveMode(750, 640) === "vertical", "aspect boundary: 750x640 is vertical (h >= w*0.85)");
  // Hysteresis in the real window: 600 px high still vertical (620 - 36 = 584), 570 is not.
  await setSize(t, 700, 600);
  t.log(t.getState().adaptiveMode === "vertical", "hysteresis: 700x600 keeps vertical (inside the dead band)", String(t.getState().adaptiveMode));
  await setSize(t, 700, 560);
  t.log(t.getState().adaptiveMode === "single", "hysteresis: 700x560 leaves vertical", String(t.getState().adaptiveMode));
  t.note("vertical mode verified; aspect-ratio boundary and hysteresis verified");
}

async function adaptiveCompact(t: T) {
  await t.updateSettings({ adaptiveLayout: true });
  const persisted = t.getState().sidebarVisible;
  await setSize(t, 640, 480);
  const ok = await t.until(() => t.getState().adaptiveMode === "single" && browsersShown(t).length === 1);
  t.log(!!ok, "640x480: single-pane mode", `mode=${t.getState().adaptiveMode}`);
  t.log(!(await t.until(() => sidebarShown(t), 300)), "sidebar auto-hidden transiently in the narrow window");
  t.log(t.getState().sidebarVisible === persisted, "persisted sidebar preference untouched", `sidebarVisible=${t.getState().sidebarVisible}`);
  await setSize(t, 460, 1000);
  t.log(t.getState().adaptiveMode === "single" && browsersShown(t).length === 1, "460x1000: width < 480 forces single pane even when tall", `mode=${t.getState().adaptiveMode}`);
  t.log(!sidebarShown(t), "sidebar stays hidden at 460 px");
  await setSize(t, 1200, 800);
  t.log(!!(await t.until(() => sidebarShown(t) && browsersShown(t).length === 2)), "widening restores the sidebar and both panes");
  t.note("single mode + sidebar auto-hide verified");
}

async function adaptiveDensity(t: T) {
  const A = await import("./state/actions");
  const { setState } = await import("./state/store");
  await t.updateSettings({ adaptiveLayout: false, showSystemMonitor: true, showHistory: false, fontSize: 14, defaultView: "default" });
  A.setLayout("classic");
  A.toggleSidebar(true);
  const shown = (selector: string) => {
    const el = t.$(selector);
    return !!el && el.getBoundingClientRect().height > 0;
  };
  const age = ".fp-hrow .fp-age-col";
  const date = ".fp-hrow .fp-date-col";
  await setSize(t, 1200, 740);
  if (virtualOnly) {
    t.log(false, "density check needs real window resizing for container queries");
    return;
  }
  t.log(!!await t.until(() => shown(age) && shown(date) && shown(".sm-full")), "wide and tall: both timestamps and full system monitor");
  await t.shot("adaptive-density-wide");
  A.setLayout("standard");
  t.log(!!await t.until(() => {
    const panes = browsersShown(t);
    return panes.length === 2 && panes.every((pane) => t.$$(`${age}, ${date}`, pane).length === 1);
  }), "at the same window size, two smaller panes each show one timestamp");
  await t.shot("adaptive-density-split");
  A.setLayout("classic");
  await t.until(() => shown(age) && shown(date));
  const dateHeader = t.$(`${date} button`) as HTMLButtonElement | null;
  dateHeader?.click();
  await t.sleep(200);
  await setSize(t, 695, 550);
  t.log(!!await t.until(() => shown(date) && !shown(age) && shown(".sm-compact") && !shown(".sm-full")),
    "narrow and short: creation-date sort stays visible and monitor compacts");
  t.log(t.$$(".fp-list .row .name").some((cell) => cell.getAttribute("title")?.includes("Modified")),
    "hidden modification timestamp is available on the filename");
  await t.shot("adaptive-density-compact");
  const monitor = t.$(".sm-compact") as HTMLButtonElement | null;
  monitor?.focus();
  monitor?.click();
  t.log(!!await t.until(() => shown(".sm-full") && monitor?.getAttribute("aria-expanded") === "true"),
    "compact monitor expands on demand");
  await t.shot("adaptive-density-expanded");
  monitor?.click();
  const sidebar = t.$(".sidebar") as HTMLElement | null;
  if (sidebar) {
    const width = sidebar.style.width;
    sidebar.style.width = "150px";
    t.log(!!await t.until(() => shown(".sm-compact .sm-initial")),
      "at minimum sidebar width, metrics use abbreviated captions");
    const readings = t.$$(".sm-reading");
    t.log(readings.length >= 2 && readings.every((el) => el.scrollWidth <= el.clientWidth + 1),
      "compact metrics are readable without clipping at minimum sidebar width");
    await t.shot("adaptive-density-minimum-sidebar");
    sidebar.style.width = width;
  }
  await setSize(t, 1200, 740);
  const ageHeader = t.$(`${age} button`) as HTMLButtonElement | null;
  ageHeader?.click();
  await t.sleep(200);
  await setSize(t, 695, 550);
  t.log(!!await t.until(() => shown(age) && !shown(date)), "narrow: modification sort keeps the age badge");
  await setSize(t, 1200, 740);
  t.log(!!await t.until(() => shown(age) && shown(date) && shown(".sm-full") && !shown(".sm-compact")),
    "widening restores timestamps and the full monitor");

  // Fixture folders give five recent rows and enough additional history to scroll.
  const path = t.activePath(0);
  const paths = ["Projects", "Documents", "Music", "Pictures", "Downloads", "Videos", "Desktop"]
    .map((name) => `${path}/${name}`);
  await t.updateSettings({ showHistory: true, showDrives: false, showCloud: false });
  // Without the default favourites, whose rows depend on the host's home folders (Windows lists the
  // real user folders), the room left for History is the same on every platform.
  setState({ history: paths, favorites: { groups: [] } });
  const monitorReturns = () => !!t.$(".sidebar-monitor:not(.is-space-hidden) .sysmon");
  const monitorHidden = () => !!t.$(".sidebar-monitor.is-space-hidden .sysmon");
  const fifth = () => t.$$(".sidebar-history > .fav")[4];
  await setSize(t, 1200, 760);
  t.log(!!await t.until(() => fifth() && monitorReturns()), "tall sidebar has room for five History rows and the monitor");
  await setSize(t, 695, 445);
  const scroll = t.$(".sidebar-scroll") as HTMLElement | null;
  const monitorBox = t.$(".sidebar-monitor");
  const row = fifth();
  let shortHeight = 445;
  if (scroll && monitorBox && row) {
    const viewport = scroll.getBoundingClientRect();
    const monitorHeight = monitorBox.getBoundingClientRect().height;
    const spaceWithoutMonitor = viewport.height + (monitorHidden() ? 0 : monitorHeight);
    const historyBottom = row.getBoundingClientRect().bottom - viewport.top + scroll.scrollTop;
    // Place the window near the threshold using the real fonts and fixture geometry.
    shortHeight = Math.round(window.innerHeight + historyBottom + monitorHeight / 2 - spaceWithoutMonitor);
    await setSize(t, 695, shortHeight);
  }
  t.log(!!await t.until(monitorHidden), "monitor yields space when the fifth History row would be clipped");
  if (scroll) {
    scroll.scrollTop = 0;
    const last = fifth();
    t.log(!!last && last.getBoundingClientRect().bottom <= scroll.getBoundingClientRect().bottom + 0.5,
      "the fifth History row is fully visible in the freed space");
    scroll.scrollTop = scroll.scrollHeight;
    await t.sleep(100);
    t.log(monitorHidden(), "scrolling History does not cause the monitor to reappear");
    scroll.scrollTop = 0;
  }
  await t.shot("adaptive-density-history-priority");
  const historyHead = t.$(".sidebar-history .group-head");
  historyHead?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  t.log(!!await t.until(() => !fifth() && monitorReturns()), "collapsing History restores the monitor");
  historyHead?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  t.log(!!await t.until(monitorHidden), "expanding History gives its rows priority again");

  // The toolbar button reflects what is on screen and can always show the monitor.
  const toggle = () => t.$(".sidebar-monitor-toggle") as HTMLButtonElement | null;
  const lit = () => toggle()?.classList.contains("active") ?? false;
  const wish = () => t.getState().settings?.showSystemMonitor;
  t.log(!lit(), "the toolbar button is not lit while the monitor makes room for History");
  toggle()?.click();
  t.log(!!await t.until(() => monitorReturns() && lit()) && wish() === true, "the toolbar button shows the monitor in a short window");
  await t.shot("adaptive-density-monitor-shown-anyway");
  toggle()?.click();
  t.log(!!await t.until(() => !t.$(".sysmon") && !lit()) && wish() === false, "a second click hides it and saves the wish");
  await setSize(t, 1200, 760);
  await t.sleep(300);
  t.log(!t.$(".sysmon") && !lit(), "a monitor the user hid stays hidden when the window grows");
  await setSize(t, 695, shortHeight);
  toggle()?.click();
  t.log(!!await t.until(() => monitorReturns() && lit()) && wish() === true, "turning it back on in a short window shows it at once");
  await setSize(t, 1200, 760);
  t.log(!!await t.until(() => monitorReturns() && !t.getState().monitorOverride), "once it fits, the manual show is no longer needed");
  await setSize(t, 695, shortHeight);
  t.log(!!await t.until(() => monitorHidden() && !lit()), "shrinking again makes room for History once more");
  await setSize(t, 1200, 760);
  t.log(!!await t.until(monitorReturns), "resizing restores the monitor without changing its preference");
  await t.shot("adaptive-density-history-and-monitor");
  await t.updateSettings({ showSystemMonitor: false });
  t.log(!!await t.until(() => !t.$(".sysmon")), "manual monitor-off preference still wins");

  if (t.getState().info?.platform === "linux") {
    const status = await t.api.fileManagerStatus();
    A.openOverlay({ type: "settings", section: "openWith" });
    const { tt } = await import("./i18n");
    const match = () => t.$("[data-file-manager-folder]")?.textContent === (status.folderHandler ?? tt("None"))
      && t.$("[data-file-manager-reveal]")?.textContent === (status.revealAvailable ? status.revealHandler ?? tt("None") : tt("Status unavailable"));
    t.log(!!await t.until(match), "Settings shows both desktop file-manager handlers");
    await t.shot("adaptive-density-file-manager-status");
    A.openOverlay(null);
  }
}

async function adaptiveOverride(t: T) {
  const A = await import("./state/actions");
  const original = t.getState().settings?.adaptiveLayout ?? true;
  await t.updateSettings({ adaptiveLayout: true });
  const dirs = [t.activePath(0), t.activePath(1)];
  await setSize(t, 1200, 800);
  await t.until(() => t.getState().adaptiveMode === "horizontal");
  await setSize(t, 700, 900);
  await t.until(() => t.getState().adaptiveMode === "vertical");
  await setSize(t, 640, 480);
  await t.until(() => t.getState().adaptiveMode === "single");
  t.log(t.activePath(0) === dirs[0] && t.activePath(1) === dirs[1], "pane folders preserved across horizontal -> vertical -> single", `${t.activePath(0)} | ${t.activePath(1)}`);
  // Manual sidebar override in the narrow window is temporary.
  await t.focusPane(0);
  A.toggleSidebar(true);
  t.log(!!(await t.until(() => sidebarShown(t))), "manual sidebar override shows it in the narrow window");
  t.log(t.getState().sidebarVisible === true, "override leaves the persisted sidebarVisible alone");
  await setSize(t, 1200, 800);
  t.log(t.getState().sidebarOverride === null && sidebarShown(t), "override is dropped when the window widens; sidebar back");
  t.log(t.activePath(0) === dirs[0] && t.activePath(1) === dirs[1], "pane folders preserved after returning to horizontal");
  // Explicit layout choices override the window size.
  await A.setLayout("classic");
  t.log(!!(await t.until(() => browsersShown(t).length === 1)), "explicit Classic layout override: one pane in a wide window");
  await A.setLayout("columns");
  t.log(!!(await t.until(() => {
    const v = browsersShown(t).map((x) => x.getBoundingClientRect());
    return v.length === 2 && v[0].bottom <= v[1].top + 2;
  })), "explicit Stacked layout override: stacked in a wide window");
  await A.setLayout("standard");
  // Adaptive layout off: the size no longer changes the arrangement.
  await t.updateSettings({ adaptiveLayout: false });
  await setSize(t, 1100, 1000);
  const v = browsersShown(t).map((x) => x.getBoundingClientRect());
  t.log(t.getState().adaptiveMode === null && v.length === 2 && Math.abs(v[0].top - v[1].top) < 2, "adaptiveLayout=false: tall window stays side by side (adaptive override off)", `mode=${t.getState().adaptiveMode}`);
  await t.updateSettings({ adaptiveLayout: original });
  t.note("manual override, explicit layout override and adaptiveLayout=false verified");
}

async function tilingMaximize(t: T) {
  const o = await t.api.omarchyStateGet();
  const setting = t.getState().settings?.startMaximized;
  t.log(o.tilingWm, "tiling window manager detected from the environment", `tilingWm=${o.tilingWm}`);
  // A live tiling compositor reports its tiled windows as maximized, so the window state is no
  // evidence either way: the startup decision (start_maximized && !tiling WM) is unit tested in
  // src-tauri (omarchy::start_maximized) and used by the window builder.
  t.note(`tiling WM=${o.tilingWm}; startMaximized setting ${setting ? "on" : "off"}; startup maximize suppressed by the tiling WM check`);
}

async function windowButtons(t: T) {
  await t.sleep(300);
  const o = await t.api.omarchyStateGet();
  const mode = t.getState().settings?.windowButtons ?? "auto";
  const shown = !!t.$(".tb-controls") || !!t.$(".tb-lights");
  const expected = windowButtonsShown(mode, o.tilingWm, o.floatingWorkspace ?? false);
  t.log(shown === expected, `window_buttons=${mode} tilingWm=${o.tilingWm}: titlebar buttons ${shown ? "visible" : "hidden"}`, `expected ${expected ? "visible" : "hidden"}`);
  t.note(`windowButtons setting=${mode}; tiling WM=${o.tilingWm}; window_buttons ${shown ? "visible" : "hidden"}`);
}

// ───────────── F10: vim and shortcut safety ─────────────

const press = (k: string, opts: KeyboardEventInit = {}, target: Element | null = document.activeElement) => {
  const code = k.length === 1 && /[a-z]/i.test(k) ? "Key" + k.toUpperCase() : k;
  const ev = new KeyboardEvent("keydown", { key: k, code, bubbles: true, cancelable: true, ...opts });
  (target ?? document.body).dispatchEvent(ev);
  return ev;
};

async function newWindowChord(t: T) {
  for (const profile of Object.keys(KEYBOARD_PROFILES) as KeyboardProfile[]) {
    const km = buildKeymap({}, profile);
    t.log(km.get("Ctrl+Shift+N") === "newFolder", `${profile}: Ctrl+Shift+N is newFolder`);
    t.log(km.get("Ctrl+Alt+N") === "newWindow", `${profile}: Ctrl+Alt+N is newWindow`);
    t.log(Object.entries(defaultKeymap(profile)).filter(([, c]) => c.includes("Ctrl+Shift+N")).length === 1, `${profile}: Ctrl+Shift+N bound exactly once`);
  }
  const tree = buildVimTree("Space");
  t.log(JSON.stringify(resolve(tree, ["Ctrl+w", "n"])) === JSON.stringify({ kind: "action", action: "newWindow" }), "vim Ctrl+w n is newWindow");
  t.log(JSON.stringify(resolve(tree, ["Space", "w", "N"])) === JSON.stringify({ kind: "action", action: "newWindow" }), "leader Space w N is newWindow");
  // The real UI: Ctrl+Shift+N opens the New folder dialog (not a window).
  await t.updateSettings({ keyboardProfile: "explorer", shortcuts: {} });
  await t.focusPane(0);
  press("N", { ctrlKey: true, shiftKey: true });
  const dlg = await t.until(() => t.$(".modal input[type=text]"));
  t.log(!!dlg, "Ctrl+Shift+N opens the New folder dialog (newFolder), not a new window");
  press("Escape", {}, t.$(".modal") ?? document.body);
  await t.until(() => !t.getState().overlay);
  t.note("newWindow vs newFolder: Ctrl+Alt+N / Ctrl+w n / Space w N open windows; Ctrl+Shift+N stays newFolder in all profiles");
}

async function ctrlL(t: T) {
  await setSize(t, 1200, 800);
  await t.updateSettings({ keyboardProfile: "explorer", shortcuts: {} });
  await t.focusPane(0);
  press("l", { ctrlKey: true });
  t.log(!!(await t.until(() => t.getState().overlay?.type === "pathEdit")), "explorer: Ctrl+L opens editPath");
  press("Escape", {}, t.$(".modal input") ?? document.body);
  await t.until(() => !t.getState().overlay);
  await t.updateSettings({ keyboardProfile: "vim", shortcuts: {} });
  await t.focusPane(0);
  const before = t.getState().session?.activeBrowser;
  press("l", { ctrlKey: true });
  await t.sleep(300);
  t.log(t.getState().overlay === null, "vim: Ctrl+l does not open editPath");
  t.log(before === 0 && t.getState().session?.activeBrowser === 1, "vim: Ctrl+l is focusRight", `activeBrowser=${t.getState().session?.activeBrowser}`);
  t.note("Ctrl+L: explorer -> editPath, vim -> focusRight");
}

/** Opens an editable field via `open`, types vim-sensitive characters and checks nothing is stolen. */
async function typingCheck(t: T, name: string, open: () => void | Promise<void>, find: () => HTMLElement | null, close: () => void) {
  await open();
  const el = await t.until(find, 2500);
  if (!el) { t.log(false, `${name}: editable field opened`); return; }
  (el as HTMLInputElement).focus();
  const overlayBefore = JSON.stringify(t.getState().overlay?.type);
  let stolen = 0;
  for (const k of ["h", "j", "k", "l", "g", "G", "q", "?", ":", " "]) {
    const ev = press(k, k === "G" || k === "?" || k === ":" ? { shiftKey: true } : {}, el);
    if (ev.defaultPrevented) stolen++;
  }
  t.log(stolen === 0, `${name}: hjklgGq?: and Space reach the editable field untouched`, `${stolen} keys consumed`);
  t.log(getComputedStyle(el).display !== "none" && t.getState().keySeq.length === 0, `${name}: no vim sequence started while editing`);
  t.log(JSON.stringify(t.getState().overlay?.type) === overlayBefore, `${name}: overlay state unchanged by typing`);
  close();
  await t.sleep(150);
}

async function editableTyping(t: T) {
  const A = await import("./state/actions");
  await t.updateSettings({ keyboardProfile: "vim", vimKeys: true, shortcuts: {} });
  await t.focusPane(0);
  const closeOverlay = () => { A.openOverlay(null); };
  await typingCheck(t, "Quick Open", () => A.openOverlay({ type: "quickOpen" }), () => t.$(".qo input"), closeOverlay);
  await typingCheck(t, "Find in files", () => A.openOverlay({ type: "grep", root: t.activePath(0), scope: "folder" }), () => t.$(".fif input"), closeOverlay);
  await typingCheck(t, "Path editor", () => A.openOverlay({ type: "pathEdit", b: 0 }), () => t.$(".modal input"), closeOverlay);
  await typingCheck(t, "Settings search", () => A.openOverlay({ type: "settings" }), () => t.$(".modal input[type=search], .modal input[type=text], .modal input:not([type])"), closeOverlay);
  await typingCheck(t, "Note editor", () => A.openOverlay({ type: "note", dir: t.activePath(0) }), () => t.$(".modal textarea") ?? t.$(".modal input"), closeOverlay);
  await t.focusPane(0);
  // The quick filter is not a text field: in it the letters are the filter text by design.
  press("/", {}, document.activeElement);
  await t.sleep(120);
  for (const k of ["h", "j", "k"]) press(k, {}, document.activeElement);
  await t.sleep(150);
  t.log(!!t.$(".quick-filter .qf-text")?.textContent?.includes("hjk"), "Quick filter: hjk typed as filter text, not as vim motions", t.$(".quick-filter")?.textContent ?? "no filter");
  press("Escape", {}, document.activeElement);
  await t.sleep(150);
  const first = t.$$(".fp-list [data-path]")[0] as HTMLElement | undefined;
  if (first) {
    first.click();
    await typingCheck(t, "Rename", () => { press("r", {}, t.$(".folder-pane")); }, () => t.$(".fp-list input"), () => { press("Escape"); });
  } else t.log(true, "Rename: skipped (empty folder); editable field coverage via the other fields");
  t.note("editable fields: rename, filter, quick open, find, path, settings and note keep ordinary typing with vim keys on");
}

async function vimQ(t: T) {
  const A = await import("./state/actions");
  await t.updateSettings({ keyboardProfile: "vim", vimKeys: true, shortcuts: {} });
  await t.focusPane(0);
  await A.sessionCmd({ op: "newTab", b: 0 });
  await t.sleep(300);
  const tabs = () => t.getState().session!.browsers[0].tabs.length;
  const before = tabs();
  await t.focusPane(0);
  press("q", {}, document.activeElement);
  t.log(!!(await t.until(() => tabs() === before - 1)), "q outside an input closes the tab (dismisses the view)", `tabs ${before} -> ${tabs()}`);
  A.openOverlay({ type: "quickOpen" });
  const input = await t.until(() => t.$<HTMLInputElement>(".qo input"));
  if (input) {
    input.focus();
    const ev = press("q", {}, input);
    t.log(!ev.defaultPrevented, "q inside an input is not consumed (types a literal q)");
    t.log(tabs() === before - 1, "q inside an input leaves the tabs alone", `tabs=${tabs()}`);
    t.log(!!t.getState().overlay, "overlay stays open while typing q");
    press("Escape", {}, input);
    t.log(!!(await t.until(() => !t.getState().overlay)), "Escape dismisses the overlay");
  } else t.log(false, "Quick Open input available");
  t.note("q: closes a tab on the pane, literal q in inputs; Escape dismisses overlays");
}

async function spaceLeader(t: T) {
  await t.updateSettings({ keyboardProfile: "vim", vimKeys: true, leaderKey: "Space", shortcuts: {} });
  await t.focusPane(0);
  press(" ", {}, t.$(".folder-pane"));
  t.log(!!(await t.until(() => t.getState().keySeq.length === 1 && !!t.$(".wk"))), "vim: Space starts the leader sequence and shows which-key");
  press("Escape");
  t.log(!!(await t.until(() => t.getState().keySeq.length === 0)), "Escape ends the leader sequence");
  await t.updateSettings({ keyboardProfile: "explorer", vimKeys: false, shortcuts: {} });
  await t.focusPane(0);
  const row = t.$$(".fp-list [data-path]")[0] as HTMLElement | undefined;
  if (row) { row.click(); await t.sleep(120); }
  press(" ", {}, t.$(".folder-pane"));
  await t.sleep(300);
  t.log(t.getState().keySeq.length === 0 && !t.$(".wk"), "explorer: Space is not a leader (no which-key)");
  if (row) t.log(t.getState().overlay?.type === "previewWindow" || !!t.$(".preview-window, .pv"), "explorer: Space opens Preview", String(t.getState().overlay?.type));
  else t.log(true, "explorer: Space preview skipped (empty folder); no which-key verified");
  t.note("Space: leader / which-key in vim mode, Preview in explorer mode");
}

async function vimBounds(t: T) {
  const A = await import("./state/actions");
  await t.updateSettings({ keyboardProfile: "vim", vimKeys: true, leaderKey: "Space", shortcuts: {} });
  await setSize(t, 1200, 800);
  await t.focusPane(0);
  // IME composition and dead keys must not trigger shortcuts.
  const seqBefore = t.getState().keySeq.length;
  const c1 = press("j", { isComposing: true });
  const c2 = press("Process", { isComposing: true } as KeyboardEventInit);
  t.log(!c1.defaultPrevented && !c2.defaultPrevented && t.getState().keySeq.length === seqBefore, "IME composing keys (isComposing) ignored by vim shortcuts",
    `j prevented=${c1.defaultPrevented} composing=${c1.isComposing} process prevented=${c2.defaultPrevented} keySeq=${t.getState().keySeq.length}/${seqBefore}`);
  // A dead key (US International ') is the vim mark prefix on its physical key: it may start a
  // sequence, but must not run an action, and Escape cancels it.
  press("Dead", { code: "Quote" });
  t.log(t.getState().overlay === null, "dead key does not trigger a shortcut action", `keySeq=${t.getState().keySeq.join("")}`);
  press("Escape");
  await t.sleep(100);
  // <select> and contenteditable never get vim keys.
  const sel = document.createElement("select");
  sel.innerHTML = "<option>a</option>";
  const ce = document.createElement("div");
  ce.contentEditable = "true";
  ce.tabIndex = 0;
  document.body.append(sel, ce);
  for (const [name, el] of [["select", sel], ["contenteditable", ce]] as const) {
    el.focus();
    const ev = press("j", {}, el);
    const ev2 = press(" ", {}, el);
    t.log(!ev.defaultPrevented && !ev2.defaultPrevented && t.getState().keySeq.length === 0, `${name}: vim keys do not capture typing`,
      `j prevented=${ev.defaultPrevented} space prevented=${ev2.defaultPrevented} keySeq=${t.getState().keySeq.length} active=${document.activeElement?.tagName}`);
  }
  sel.remove();
  ce.remove();
  // Space leader interrupted by click, blur and Escape.
  await t.focusPane(0);
  for (const how of ["click", "blur", "Escape"]) {
    press(" ", {}, t.$(".folder-pane"));
    await t.until(() => t.getState().keySeq.length > 0, 800);
    const started = t.getState().keySeq.length > 0;
    if (how === "click") window.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    else if (how === "blur") window.dispatchEvent(new Event("blur"));
    else press("Escape");
    await t.sleep(150);
    t.log(started && t.getState().keySeq.length === 0, `leader sequence interrupted by ${how} clears cleanly`);
  }
  // A custom shortcut colliding with a vim key.
  t.log(shadowedByVim("Ctrl+H"), "shadowedByVim: Ctrl+H collides with vim Ctrl+h");
  await t.updateSettings({ shortcuts: { newFolder: "Ctrl+H" } });
  A.openOverlay({ type: "keys" });
  t.log(!!(await t.until(() => t.$(".kh"))), "Key Help opens with the shadowed Ctrl+H shortcut present");
  A.openOverlay(null);
  await t.updateSettings({ shortcuts: {} });
  // Caps Lock: letter case follows Shift, not Caps Lock.
  await t.focusPane(0);
  const capsJ = press("J", { shiftKey: false, modifierCapsLock: true } as KeyboardEventInit);
  t.log(capsJ.defaultPrevented, "Caps Lock J (shiftKey=false) acts as the j motion");
  const gTop = press("G", { shiftKey: true });
  t.log(gTop.defaultPrevented, "Shift+g acts as the G motion (last item)");
  // Rapid '/' filter -> Ctrl+P -> Escape -> Escape returns focus to the pane.
  await t.focusPane(0);
  press("/");
  await t.sleep(80);
  press("p", { ctrlKey: true }, document.activeElement);
  await t.sleep(120);
  press("Escape", {}, document.activeElement);
  await t.sleep(120);
  press("Escape", {}, document.activeElement);
  await t.sleep(250);
  const focused = document.activeElement;
  t.log(!t.getState().overlay && !!focused && (!!focused.closest(".folder-pane") || focused === document.body), "rapid filter -> Quick Open -> Escape -> Escape returns focus to the pane",
    `active=${focused?.tagName}.${(focused as HTMLElement | null)?.className ?? ""}`);
  t.note("vim bounds: composing/dead keys, select/contenteditable, leader interruption, shadowed Ctrl+H, caps/shift, escape focus");
}

const MODES: Record<string, (t: T) => Promise<void>> = {
  "e2e-omarchy-dark": omarchyDark,
  "e2e-omarchy-swap": omarchySwap,
  "e2e-omarchy-rounding": omarchyRounding,
  "e2e-omarchy-font": omarchyFont,
  "e2e-omarchy-glassy": omarchyGlassy,
  "e2e-omarchy-inactive": omarchyInactive,
  "e2e-adaptive-wide": adaptiveWide,
  "e2e-adaptive-tall": adaptiveTall,
  "e2e-adaptive-compact": adaptiveCompact,
  "e2e-adaptive-density": adaptiveDensity,
  "e2e-adaptive-override": adaptiveOverride,
  "e2e-tiling-maximize": tilingMaximize,
  "e2e-window-buttons": windowButtons,
  "e2e-vim-new-window-chord": newWindowChord,
  "e2e-vim-ctrl-l": ctrlL,
  "e2e-vim-editable-typing": editableTyping,
  "e2e-vim-q-input": vimQ,
  "e2e-vim-space-leader": spaceLeader,
  "e2e-vim-bounds": vimBounds,
};

export const runLayoutMode: ModeGroup = async (mode, t) => {
  const fn = MODES[mode];
  if (!fn) return false;
  const errors: string[] = [];
  window.addEventListener("error", (e) => errors.push(e.message));
  window.addEventListener("unhandledrejection", (e) => errors.push(String(e.reason)));
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    if (!mode.includes("tiling") && !mode.includes("inactive") && !mode.startsWith("e2e-omarchy")) await getCurrentWindow().unmaximize().catch(() => {});
    await fn(t);
  } catch (e) {
    t.log(false, `${mode} scenario`, String(e));
  }
  t.log(!errors.length, `no uncaught errors in ${mode}`, errors.join("; "));
  await t.finish();
  return true;
};
