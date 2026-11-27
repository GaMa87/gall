import type { OmarchyState } from "./api/types";
import type { gitSelftest } from "./selftest";

/** Synthetic Omarchy states: the real feed is exercised by the backend tests; here the UI side is. */
const DARK: Record<string, string> = {
  "--bg": "#16161e", "--bg-2": "#1a1b26", "--bg-3": "#24283b", "--bg-hover": "#2a2f45", "--fg": "#c0caf5", "--fg-dim": "#9aa5ce",
  "--fg-faint": "#565f89", "--border": "#2f334d", "--accent": "#7aa2f7", "--accent-2": "#bb9af7", "--brand-b": "#e0af68",
  "--danger": "#f7768e", "--ok": "#9ece6a", "--warn": "#e0af68", "--folder": "#7aa2f7",
};
const LIGHT: Record<string, string> = {
  "--bg": "#e6e7ed", "--bg-2": "#f5f5f8", "--bg-3": "#dcdee6", "--bg-hover": "#d3d6e0", "--fg": "#343b58", "--fg-dim": "#5a607f",
  "--fg-faint": "#8a90ad", "--border": "#c5c8d6", "--accent": "#34548a", "--accent-2": "#5a4a78", "--brand-b": "#8f5e15",
  "--danger": "#8c4351", "--ok": "#485e30", "--warn": "#8f5e15", "--folder": "#34548a",
};
const LOW_CONTRAST: Record<string, string> = {
  "--bg": "#202020", "--bg-2": "#222222", "--bg-3": "#242424", "--bg-hover": "#232323", "--fg": "#353535", "--fg-dim": "#303030",
  "--fg-faint": "#292929", "--border": "#232323", "--accent": "#252525", "--accent-2": "#292929", "--brand-b": "#303030",
  "--danger": "#353535", "--ok": "#353535", "--warn": "#353535", "--folder": "#252525",
};
const state = (mode: "dark" | "light", rounding: number | null, over: Partial<OmarchyState> = {}): OmarchyState => ({
  active: true, themeMode: mode, paletteTokens: mode === "dark" ? DARK : LIGHT, customCss: null, rounding, glassy: false,
  monospaceFont: null, tilingWm: false, language: null, ...over,
});

/** Omarchy theme: dark and light palettes, rounding 0 vs 8, Glassy, tiling-WM window buttons. */
export async function runOmarchySelfTest(t: typeof gitSelftest) {
  const { $, $$, click, until, sleep, shot, setWindowSize, log, updateSettings, getState, focusPane, finish } = t;
  const { setState } = await import("./state/store");
  const A = await import("./state/actions");
  const { contrastRatio } = await import("./core/omarchyTheme");
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const errors: string[] = [];
  window.addEventListener("error", (e) => errors.push(e.message));
  window.addEventListener("unhandledrejection", (e) => errors.push(String(e.reason)));
  const css = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  const px = (el: Element | null, prop: string) => (el ? parseFloat(getComputedStyle(el).getPropertyValue(prop)) : NaN);
  const SHOTS = "expected one palette throughout (no leftover default colours), readable list text, accent on the selection";
  try {
    await getCurrentWindow().unmaximize();
    await setWindowSize(1100, 760);
    await updateSettings({ theme: "omarchy", language: "en", keyboardProfile: "explorer", shortcuts: {}, adaptiveLayout: true, windowButtons: "auto" });
    await focusPane(0);

    for (const [mode, rounding, tokens] of [["dark", 8, DARK], ["light", 8, LIGHT], ["dark", 0, DARK], ["light", 0, LIGHT], ["dark", 8, LOW_CONTRAST]] as const) {
      const paletteName = tokens === LOW_CONTRAST ? "low-contrast" : mode;
      setState({ omarchy: state(mode, rounding, { paletteTokens: tokens }) });
      const ok = await until(() => css("--bg-2").toLowerCase() === tokens["--bg-2"] && document.documentElement.dataset.theme === mode);
      log(!!ok, `omarchy ${mode}: palette applied live (--bg-2 ${css("--bg-2")}, data-theme ${document.documentElement.dataset.theme})`);
      log(contrastRatio(css("--accent"), css("--bg-2")) >= 3, `omarchy ${mode}: accent follows the palette with 3:1 control contrast`, css("--accent"));
      const app = $(".app");
      log(!!app && getComputedStyle(app).backgroundColor !== "", `omarchy ${mode}: app surface painted`);
      const r = css("--radius");
      const expectRadius = rounding === 0 ? (r === "0px" || r === "0") : parseFloat(r) > 0;
      log(expectRadius, `omarchy ${mode}: rounding ${rounding} gives --radius ${r}`);
      const probe = $(".tb-layout") ?? $(".icon-btn");
      const radius = px(probe, "border-top-left-radius");
      log(rounding === 0 ? radius <= 2 : radius >= 3, `omarchy ${mode}: buttons are ${rounding === 0 ? "square" : "rounded"} at rounding ${rounding}`, `${radius}px`);
      const sel = $$(".fp-list [data-path]")[0];
      if (sel) { (sel as HTMLElement).click(); await sleep(120); }
      await shot(`omarchy-${paletteName}-rounding-${rounding}`);
      t.note(`SCREENSHOT omarchy-${paletteName}-rounding-${rounding}.png: ${SHOTS}; corners ${rounding === 0 ? "crisp (0-2px)" : "rounded"}.`);
      if (rounding === 8) {
        const controlRatios = {
          border: contrastRatio(css("--om-control-border"), css("--bg-2")),
          button: contrastRatio(css("--om-control-bg"), css("--bg-2")),
          hover: contrastRatio(css("--om-control-hover"), css("--bg-2")),
          focus: contrastRatio(css("--om-control-accent"), css("--bg-2")),
        };
        log(Object.values(controlRatios).every((ratio) => ratio >= 3), `omarchy ${mode}: borders, buttons, hover and focus meet 3:1`, JSON.stringify(controlRatios));
        const transparent = (node: Element) => getComputedStyle(node).backgroundColor === "rgba(0, 0, 0, 0)";
        const paneButtons = $$(".pm-arrow:not(:disabled), .fp-pill button").filter((node) => !node.matches(":hover"));
        log(paneButtons.length > 0 && paneButtons.every(transparent), `omarchy ${paletteName}: idle pane controls have no fill`);
        const quietRatio = contrastRatio(css("--om-control-quiet-text"), css("--bg-2"));
        log(quietRatio >= 4.5, `omarchy ${paletteName}: quiet controls keep a readable foreground`, quietRatio.toFixed(2));
        A.openOverlay({ type: "settings", section: "appearance" });
        const settings = await until(() => $(".settings"));
        const navRow = settings && $("nav button:not(.on):not(:hover)", settings);
        const navIcon = navRow && $("svg", navRow);
        log(!!navRow && transparent(navRow) && !!navIcon
          && getComputedStyle(navIcon).color === getComputedStyle(navRow).color,
        `omarchy ${mode}: idle Settings row has no fill and its icon matches its label`);
        const toggle = settings && $("input.switch", settings) as HTMLInputElement | null;
        const button = settings && $(".btn.primary", settings) as HTMLButtonElement | null;
        log(!!settings && !!toggle && !!button, `omarchy ${mode}: Settings shows toggles and buttons`);
        const canvas = document.createElement("canvas").getContext("2d")!;
        const rgbHex = (color: string) => {
          canvas.clearRect(0, 0, 1, 1);
          canvas.fillStyle = color;
          canvas.fillRect(0, 0, 1, 1);
          return `#${Array.from(canvas.getImageData(0, 0, 1, 1).data).slice(0, 3)
            .map((n) => n.toString(16).padStart(2, "0")).join("")}`;
        };
        // Exercise primary Git controls that do not use the shared .btn class (including Continue).
        const workspace = document.createElement("div"); workspace.className = "gg-workspace";
        const operation = document.createElement("div"); operation.className = "gg-operation";
        const continueButton = document.createElement("button"); continueButton.textContent = "Continue"; operation.append(continueButton);
        const rebase = document.createElement("div"); rebase.className = "grw-footer";
        const startButton = document.createElement("button"); startButton.className = "grw-start"; startButton.textContent = "Start"; rebase.append(startButton);
        const merge = document.createElement("div"); merge.className = "gm";
        const selected = document.createElement("button"); selected.setAttribute("aria-pressed", "true"); selected.textContent = "Both"; merge.append(selected);
        const actions = document.createElement("div"); actions.className = "gm-actions";
        const confirmButton = document.createElement("button"); confirmButton.className = "gm-confirm"; confirmButton.textContent = "Confirm"; actions.append(confirmButton); merge.append(actions);
        workspace.append(operation, rebase, merge); document.body.append(workspace);
        const primaryRatios = [continueButton, startButton, selected, confirmButton].map((node) => {
          const style = getComputedStyle(node);
          return contrastRatio(rgbHex(style.color), rgbHex(style.backgroundColor));
        });
        log(primaryRatios.every((ratio) => ratio >= 4.5), `omarchy ${paletteName}: Git primary buttons have readable labels`, primaryRatios.map((r) => r.toFixed(2)).join(" / "));
        workspace.remove();
        const syntaxProbe = document.createElement("span"); syntaxProbe.className = "sx1"; syntaxProbe.textContent = "comment";
        syntaxProbe.style.background = "color-mix(in srgb, rgb(70, 130, 255) 18%, var(--bg-2))";
        document.body.append(syntaxProbe);
        const syntaxRatio = () => { const style = getComputedStyle(syntaxProbe); return contrastRatio(rgbHex(style.color), rgbHex(style.backgroundColor)); };
        log(!!await until(() => syntaxRatio() >= 4.5, 2000), `omarchy ${paletteName}: syntax remains readable on merge colours after theme changes`, syntaxRatio().toFixed(2));
        syntaxProbe.remove();
        const search = settings && $(".ssearch", settings);
        const searchInput = search && $("input", search) as HTMLInputElement | null;
        searchInput?.focus();
        const searchStyle = search && getComputedStyle(search);
        log(!!searchInput && document.activeElement === searchInput && searchStyle?.outlineStyle === "solid" && searchStyle.borderTopColor === "rgba(0, 0, 0, 0)"
          && getComputedStyle(searchInput).outlineStyle === "none",
        `omarchy ${paletteName}: Settings search focus outlines the whole field once`);
        const field = settings && $("input:not(.ssearch input):not(.switch):not([type=checkbox]):not([type=radio]):not([type=range]), select", settings);
        const ratio = field ? contrastRatio(rgbHex(getComputedStyle(field).borderTopColor), css("--bg-2")) : 0;
        log(ratio >= 3, `omarchy ${paletteName}: text fields keep a 3:1 border against the panel`, ratio.toFixed(2));
        const link = settings && $(".folder-link", settings);
        const linkButtons = link ? $$("button:not(:hover)", link) : [];
        log(linkButtons.length > 0 && linkButtons.every(transparent),
          `omarchy ${paletteName}: Settings folder link and pane shortcuts have no idle fill`);
        const labels = settings ? $$(".scard .opt-label, .scard .opt-text small", settings) : [];
        const labelRatios = labels.map((label) => contrastRatio(rgbHex(getComputedStyle(label).color),
          rgbHex(getComputedStyle(label.closest(".scard")!).backgroundColor)));
        log(labelRatios.length > 0 && Math.min(...labelRatios) >= 4.5,
          `omarchy ${paletteName}: rendered Settings labels meet 4.5:1`, Math.min(...labelRatios).toFixed(2));
        await sleep(220);
        await shot(`omarchy-settings-${paletteName}-controls`);
        if (toggle && button) {
          button.focus();
          await sleep(80);
          log(button.matches(":focus-visible"), `omarchy ${mode}: button keyboard focus ring is visible`);
          await shot(`omarchy-settings-${paletteName}-button-focus`);
          toggle.focus();
          await sleep(100);
          await shot(`omarchy-settings-${paletteName}-focus`);
          log(toggle.matches(":focus-visible"), `omarchy ${mode}: toggle keyboard focus ring is visible`);
          const checked = toggle.checked;
          click(toggle);
          const changed = await until(() => toggle.checked !== checked);
          log(!!changed, `omarchy ${mode}: switch changes to its other state`);
          await sleep(180);
          await shot(`omarchy-settings-${paletteName}-toggle-${toggle.checked ? "on" : "off"}`);
          toggle.disabled = true;
          button.disabled = true;
          await sleep(100);
          log(toggle.disabled && button.disabled
            && getComputedStyle(toggle).borderStyle === "dashed"
            && getComputedStyle(button).borderStyle === "dashed",
          `omarchy ${mode}: disabled controls retain a visible dashed boundary`);
          await shot(`omarchy-settings-${paletteName}-disabled`);
          toggle.disabled = false;
          button.disabled = false;
          if (toggle.checked !== checked) { click(toggle); await until(() => toggle.checked === checked); }
        }
        t.note(`SCREENSHOT omarchy-settings-${paletteName}-controls.png plus focus/toggle/disabled captures: inspect control boundaries, states and palette legibility.`);
        A.openOverlay(null);
        await until(() => !$(".settings"));

        A.openOverlay({ type: "gitGraph", path: `${t.activePath(0)}/Projects` });
        const commit = await until(() => $(".git-graph-lines [data-commit]:not(.selected):not(:hover)"));
        const toolbar = await until(() => $$(".gg-toolbar button").find((node) => !node.matches(":hover")));
        log(!!commit && transparent(commit) && getComputedStyle(commit).boxShadow === "none",
          `omarchy ${paletteName}: idle Git history rows use the workspace surface`);
        log(!!toolbar && transparent(toolbar), `omarchy ${paletteName}: idle Git toolbar buttons have no solid fill`);
        const working = $(".git-graph-lines .gg-working") as HTMLElement | null;
        working?.focus();
        await until(() => $(".gg-focus-history .gg-working.selected"));
        const selection = working && getComputedStyle(working);
        log(!!selection && selection.backgroundImage !== "none" && transparent(working!),
          `omarchy ${paletteName}: selected Git row keeps its tinted gradient`);
        await shot(`omarchy-git-${paletteName}`);
        A.openOverlay(null);
        await until(() => !$(".gg-workspace"));
      }
    }

    // Glassy: translucent surfaces, text stays opaque.
    setState({ omarchy: state("dark", 8, { glassy: true }) });
    await updateSettings({ omarchyGlassy: true });
    const glassy = await until(() => document.documentElement.classList.contains("omarchy-glassy") || document.body.classList.contains("omarchy-glassy"));
    log(!!glassy, "omarchy glassy: the omarchy-glassy class is set");
    const bg = getComputedStyle($(".app") ?? document.body).backgroundColor;
    const alpha = /rgba?\(([^)]*)\)/.exec(bg)?.[1].split(",").map((x) => x.trim())[3];
    log(alpha !== undefined && parseFloat(alpha) < 1, "omarchy glassy: the app surface is translucent", bg);
    const fg = getComputedStyle($(".fp-list [data-path]") ?? document.body).color;
    log(!/rgba\(.*,\s*0?\.\d+\)/.test(fg), "omarchy glassy: list text stays opaque", fg);
    await shot("omarchy-glassy");
    t.note("SCREENSHOT omarchy-glassy.png: expected translucent panels with fully sharp text, icons and thumbnails (no blur or fade on content).");
    await updateSettings({ omarchyGlassy: false });
    setState({ omarchy: state("dark", 8) });
    log(!!(await until(() => !document.documentElement.classList.contains("omarchy-glassy") && !document.body.classList.contains("omarchy-glassy"))), "omarchy glassy: switching it off removes the class");

    // Window buttons: auto hides them in a tiling window manager, always / never override.
    const buttons = () => !!$(".tb-controls") || !!$(".tb-lights");
    setState({ omarchy: state("dark", 8, { tilingWm: true }) });
    log(!!(await until(() => !buttons())), "tiling WM + auto: no minimize / maximize / close buttons");
    await shot("omarchy-tiling-titlebar");
    t.note("SCREENSHOT omarchy-tiling-titlebar.png: expected a title bar without window buttons; layout switch at the right edge.");
    await updateSettings({ windowButtons: "always" });
    log(!!(await until(() => buttons())), "tiling WM + always: window buttons shown");
    await updateSettings({ windowButtons: "never" });
    setState({ omarchy: state("dark", 8, { tilingWm: false }) });
    log(!!(await until(() => !buttons())), "never: window buttons hidden even outside a tiling WM");
    await updateSettings({ windowButtons: "auto" });
    log(!!(await until(() => buttons())), "auto outside a tiling WM: window buttons shown");

    // Switching away restores the built-in palette.
    await updateSettings({ theme: "dark" });
    log(!!(await until(() => css("--bg-2").toLowerCase() !== DARK["--bg-2"])), "leaving the omarchy theme removes its palette", css("--bg-2"));
    setState({ omarchy: null });

    // Custom stylesheet contrast: solid fills, gradient stops, alpha layers, focus, limits in UI, and no stale/loop behaviour.
    const probeCanvas = document.createElement("canvas");
    probeCanvas.width = probeCanvas.height = 1;
    const probeCtx = probeCanvas.getContext("2d", { willReadFrequently: true })!;
    const hexOf = (color: string) => {
      probeCtx.clearRect(0, 0, 1, 1);
      probeCtx.fillStyle = "#000000";
      probeCtx.fillStyle = color;
      probeCtx.fillRect(0, 0, 1, 1);
      return `#${Array.from(probeCtx.getImageData(0, 0, 1, 1).data).slice(0, 3).map((n) => n.toString(16).padStart(2, "0")).join("")}`;
    };
    const injectUserTheme = (cssText: string) => {
      document.getElementById("user-theme")?.remove();
      if (!cssText) return;
      const el = document.createElement("style");
      el.id = "user-theme";
      el.textContent = cssText;
      document.head.appendChild(el);
    };

    // 1. Dark theme with white solid --primary-gradient, light gradient --btn-bg / --sel-gradient, and alpha status colours.
    await updateSettings({ theme: "dark" });
    injectUserTheme(`
      :root:root[data-theme="dark"] {
        --bg: #0d0f13;
        --bg-2: #15181e;
        --bg-3: #1d2129;
        --fg: #e7e9ee;
        --accent: #1a1d24 !important;
        --danger: rgba(242, 112, 111, 0.55);
        --ok: rgba(95, 208, 143, 0.55);
        --primary-gradient: #ffffff;
        --danger-bg: linear-gradient(135deg, #fff5f5, #ffd6d6);
        --btn-bg: linear-gradient(180deg, #f6f8fa, #e8edf2);
        --btn-bg-hover: rgba(255, 255, 255, 0.92);
        --sel-gradient: linear-gradient(90deg, #ffffff, #e6f0ff);
      }
    `);
    log(!!(await until(() => contrastRatio(hexOf(css("--primary-fg")), "#ffffff") >= 4.5, 2000)),
      "custom dark solid white --primary-gradient keeps --primary-fg readable",
      `${css("--primary-fg")} (${contrastRatio(hexOf(css("--primary-fg")), "#ffffff").toFixed(2)}:1)`);
    log(contrastRatio(hexOf(css("--danger-fg")), "#ffd6d6") >= 4.5,
      "custom light gradient --danger-bg keeps --danger-fg readable",
      `${css("--danger-fg")} (${contrastRatio(hexOf(css("--danger-fg")), "#ffd6d6").toFixed(2)}:1)`);
    log(contrastRatio(hexOf(css("--readable-btn-fg")), "#f6f8fa") >= 4.5,
      "custom light gradient --btn-bg keeps --readable-btn-fg readable",
      `${css("--readable-btn-fg")} (${contrastRatio(hexOf(css("--readable-btn-fg")), "#f6f8fa").toFixed(2)}:1)`);
    log(contrastRatio(hexOf(css("--readable-sel-fg")), "#ffffff") >= 4.5,
      "custom light gradient --sel-gradient keeps --readable-sel-fg readable",
      `${css("--readable-sel-fg")} (${contrastRatio(hexOf(css("--readable-sel-fg")), "#ffffff").toFixed(2)}:1)`);
    log(contrastRatio(hexOf(css("--readable-focus")), "#15181e") >= 3,
      "low-contrast dark accent keeps 3:1 focus ring via --readable-focus",
      `${css("--readable-focus")} (${contrastRatio(hexOf(css("--readable-focus")), "#15181e").toFixed(2)}:1)`);
    A.openOverlay({ type: "settings", section: "appearance" });
    await until(() => $(".settings"));
    await sleep(180);
    await shot("custom-contrast-dark-solid-gradient");
    t.note("SCREENSHOT custom-contrast-dark-solid-gradient.png: expected dark Settings panel with readable dark text on white/light primary/button/selected surfaces and no false contrast limit warning.");
    log(!$(".theme-contrast-note"), "clean custom variable theme shows no false contrast limit note");

    // 2. Custom theme with unavoidable limits (opposing gradient + image background + direct low-contrast override).
    injectUserTheme(`
      :root:root[data-theme="dark"] {
        --primary-gradient: linear-gradient(90deg, #000000, #ffffff);
        --danger-bg: url("data:image/gif;base64,R0lGODlhAQABAAAAACw=");
      }
      .custom-probe { color: #999999; opacity: 0.4; }
    `);
    const note = await until(() => $(".theme-contrast-note"), 2000);
    log(!!note && note.textContent!.includes("--danger-bg") && note.textContent!.includes("--primary-gradient"),
      "theme UI exposes image, opposing-gradient and custom CSS contrast limits",
      note?.textContent ?? "missing");
    await sleep(160);
    await shot("custom-contrast-limits-note");
    t.note("SCREENSHOT custom-contrast-limits-note.png: expected Settings › Appearance › Custom theme showing the readable contrast limit note.");

    // 3. Light theme with dark custom --primary-gradient and clean switch-back (no stale colours or observer loop).
    await updateSettings({ theme: "light" });
    injectUserTheme(`
      :root:root[data-theme="light"] {
        --primary-gradient: #111827;
        --sel-gradient: linear-gradient(90deg, #1f2937, #111827);
      }
    `);
    log(!!(await until(() => contrastRatio(hexOf(css("--primary-fg")), "#111827") >= 4.5, 2000)),
      "custom light theme with dark --primary-gradient switches --primary-fg cleanly",
      `${css("--primary-fg")} (${contrastRatio(hexOf(css("--primary-fg")), "#111827").toFixed(2)}:1)`);
    log(contrastRatio(hexOf(css("--readable-sel-fg")), "#111827") >= 4.5,
      "custom light theme with dark --sel-gradient keeps selected text readable",
      `${css("--readable-sel-fg")} (${contrastRatio(hexOf(css("--readable-sel-fg")), "#111827").toFixed(2)}:1)`);
    await sleep(160);
    await shot("custom-contrast-light-dark-primary");
    t.note("SCREENSHOT custom-contrast-light-dark-primary.png: expected light Settings with light text on dark selected nav item and primary controls.");

    // Remove custom theme and verify no stale overrides and no MutationObserver loop.
    injectUserTheme("");
    await updateSettings({ theme: "dark" });
    await until(() => document.documentElement.dataset.theme === "dark" && !$(".theme-contrast-note") && contrastRatio(hexOf(css("--primary-fg")), hexOf(css("--bg-2"))) >= 4.5, 2000);
    await new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
    let rootStyleMutations = 0;
    const loopProbe = new MutationObserver(() => { rootStyleMutations++; });
    loopProbe.observe(document.documentElement, { attributes: true, attributeFilter: ["style"] });
    await sleep(260);
    loopProbe.disconnect();
    log(rootStyleMutations === 0, "theme contrast watcher settles without MutationObserver loops", `mutations=${rootStyleMutations}`);
    A.openOverlay(null);
    await until(() => !$(".settings"));
    void getState;
  } catch (e) {
    log(false, "omarchy visual scenarios", String(e));
  }
  log(!errors.length, "no uncaught omarchy errors", errors.join("; "));
  await finish();
}

/** 2D adaptive layout: wide → side by side, tall half tile → stacked, compact quarter → one browser. */
export async function runAdaptiveLayoutSelfTest(t: typeof gitSelftest) {
  const { $, $$, key, until, sleep, shot, setWindowSize, log, updateSettings, getState, focusPane, activePath, finish } = t;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const A = await import("./state/actions");
  const errors: string[] = [];
  window.addEventListener("error", (e) => errors.push(e.message));
  window.addEventListener("unhandledrejection", (e) => errors.push(String(e.reason)));
  const visible = () => $$(".browser").filter((b) => (b.parentElement as HTMLElement).style.display !== "none" && b.getBoundingClientRect().width > 0);
  const rect = (b: Element) => b.getBoundingClientRect();
  const sidebar = () => $(".sidebar") && $(".sidebar")!.getBoundingClientRect().width > 0;
  const expectMode = async (w: number, h: number, mode: "horizontal" | "vertical" | "single", name: string) => {
    await setWindowSize(w, h);
    const ok = await until(() => getState().adaptiveMode === mode && visible().length === (mode === "single" ? 1 : 2));
    log(!!ok, `${name} ${w}x${h}: ${mode}`, `mode=${getState().adaptiveMode} browsers=${visible().length}`);
    const v = visible();
    if (mode === "horizontal" && v.length === 2) log(Math.abs(rect(v[0]).top - rect(v[1]).top) < 2 && rect(v[0]).right <= rect(v[1]).left + 2, `${name}: panes sit side by side`);
    if (mode === "vertical" && v.length === 2) log(Math.abs(rect(v[0]).left - rect(v[1]).left) < 2 && rect(v[0]).bottom <= rect(v[1]).top + 2, `${name}: panes are stacked`);
    const overflow = document.documentElement.scrollWidth > window.innerWidth + 1;
    log(!overflow, `${name}: no horizontal page overflow`, `${document.documentElement.scrollWidth}/${window.innerWidth}`);
    await sleep(150);
  };
  try {
    await getCurrentWindow().unmaximize();
    await updateSettings({ keyboardProfile: "explorer", shortcuts: {}, theme: "dark", adaptiveLayout: true });
    await A.setLayout("standard");
    await focusPane(0);

    await expectMode(1280, 800, "horizontal", "wide / fullscreen");
    log(!!sidebar(), "wide: the sidebar is shown");
    await shot("adaptive-wide-1280x800");
    t.note("SCREENSHOT adaptive-wide-1280x800.png: expected sidebar + two panes side by side, panel map centred over the divider.");

    await expectMode(840, 1000, "vertical", "tall half tile");
    await shot("adaptive-tall-840x1000");
    t.note("SCREENSHOT adaptive-tall-840x1000.png: expected two panes stacked top / bottom, panel map arrows pointing up / down, divider aligned under the panel map.");

    await expectMode(760, 480, "single", "compact quarter tile");
    log(!sidebar(), "compact: the sidebar hides by itself");
    log(getState().sidebarVisible, "compact: the saved sidebar preference is untouched");
    await shot("adaptive-compact-760x480");
    t.note("SCREENSHOT adaptive-compact-760x480.png: expected one pane using the full width, no sidebar, panel map showing which pane is visible.");

    // Both browsers stay mounted: the other pane keeps its folder, and switching shows it.
    const before = [activePath(0), activePath(1)];
    await focusPane(0);
    key("Tab");
    log(!!(await until(() => getState().session?.activeBrowser === 1 && visible().length === 1)), "compact: Tab switches to the other pane");
    log(activePath(0) === before[0] && activePath(1) === before[1], "compact: both panes keep their folders");
    await shot("adaptive-compact-other-pane");
    t.note("SCREENSHOT adaptive-compact-other-pane.png: expected the second browser filling the window.");
    A.focusBrowser(0);

    // Manual sidebar toggle in a narrow window is temporary and does not touch the preference.
    key("s", { altKey: true });
    log(!!(await until(() => sidebar())), "compact: Alt+S shows the sidebar manually");
    log(getState().sidebarVisible, "compact: the manual show leaves the saved preference");
    await shot("adaptive-compact-sidebar-override");
    t.note("SCREENSHOT adaptive-compact-sidebar-override.png: expected the sidebar overlaying / next to the single pane without clipping.");
    key("s", { altKey: true });

    // Hysteresis: a size just inside the band keeps the previous mode.
    await setWindowSize(1280, 800);
    await until(() => getState().adaptiveMode === "horizontal");
    // The threshold applies to the workspace: the window minus the open sidebar.
    const { sidebarWidth } = await import("./state/desktop");
    const { sidebarShown } = await import("./state/store");
    const inBand = 900 + (sidebarShown(getState()) ? sidebarWidth() : 0);
    await setWindowSize(inBand, 800);
    await sleep(200);
    log(getState().adaptiveMode === "horizontal", "hysteresis: 900 px of workspace keeps two panes side by side after shrinking from wide", `${inBand} px window: ${getState().adaptiveMode}`);
    await shot("adaptive-hysteresis-900x800");
    t.note("SCREENSHOT adaptive-hysteresis-900x800.png: expected still two panes side by side (inside the 36 px dead band).");

    // Explicit choices win over the window size.
    await setWindowSize(1280, 800);
    await A.setLayout("classic");
    log(!!(await until(() => visible().length === 1)), "explicit Classic layout shows one browser in a wide window");
    await A.setLayout("columns");
    log(!!(await until(() => visible().length === 2 && visible()[0].getBoundingClientRect().bottom <= visible()[1].getBoundingClientRect().top + 2)), "explicit Stacked layout stacks the panes in a wide window");
    await A.setLayout("standard");
    await updateSettings({ adaptiveLayout: false });
    await setWindowSize(1100, 1000); // tall, but with adaptive layout off nothing stacks it
    log(!!(await until(() => getState().adaptiveMode === null && visible().length === 2 && Math.abs(rect(visible()[0]).top - rect(visible()[1]).top) < 2)), "adaptive layout off: the layout stays side by side", String(getState().adaptiveMode));
    await updateSettings({ adaptiveLayout: true });
  } catch (e) {
    log(false, "adaptive layout visual scenarios", String(e));
  }
  log(!errors.length, "no uncaught adaptive layout errors", errors.join("; "));
  await finish();
}
