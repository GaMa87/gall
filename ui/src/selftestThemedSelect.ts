import type { gitSelftest } from "./selftest";

/** Focused real-WebView coverage; native option computed styles cannot prove popup colours. */
export async function runThemedSelectSelfTest(t: typeof gitSelftest) {
  const { $, $$, mouse, click, key, until, sleep, shot, setWindowSize, log, updateSettings, getState } = t;
  const A = await import("./state/actions");
  const press = async (value: string, select: HTMLSelectElement) => { key(value, {}, select); await sleep(60); };
  const popup = () => $(".themed-select-menu[role=listbox]");
  const open = async (select: HTMLSelectElement) => {
    select.scrollIntoView({ block: "center" });
    select.focus();
    await sleep(120); // let the Settings scroll finish before opening its anchored popup
    mouse(select, "mousedown");
    const menu = await until(popup, 1500);
    if (!menu) throw new Error("The app-rendered dropdown did not open (native menus cannot follow custom palettes)");
    return menu;
  };
  const assertPalette = (menu: Element, name: string, grouped = true) => {
    const root = getComputedStyle(document.documentElement);
    const probe = document.createElement("span");
    document.body.append(probe);
    probe.style.backgroundColor = root.getPropertyValue("--bg-2");
    probe.style.color = root.getPropertyValue("--fg");
    const expected = getComputedStyle(probe);
    log(getComputedStyle(menu).backgroundColor === expected.backgroundColor, `${name}: open popup uses the theme background`, getComputedStyle(menu).backgroundColor);
    const option = $("[role=option]", menu)!;
    log(getComputedStyle(option).color === expected.color, `${name}: option text uses the theme foreground`, getComputedStyle(option).color);
    const group = $(".themed-select-group-label", menu);
    if (grouped) log(!!group && getComputedStyle(group).color !== "", `${name}: Included group is rendered inside the themed popup`);
    probe.remove();
    const rect = menu.getBoundingClientRect();
    log(rect.left >= 0 && rect.top >= 0 && rect.right <= innerWidth + 1 && rect.bottom <= innerHeight + 1, `${name}: popup fits the viewport`);
    log(!!$("[role=option][aria-selected=true]", menu), `${name}: current choice is marked`);
  };
  try {
    await setWindowSize(1100, 800);
    await updateSettings({ indexRoots: [], animations: false });
    A.openOverlay({ type: "settings", section: "appearance" });
    await until(() => $(".theme-pick select"));
    for (const theme of ["dark", "light"] as const) {
      for (const customTheme of ["", "gruvbox", "nightowl"]) {
        await updateSettings({ theme, customTheme });
        await until(() => customTheme ? $("#user-theme")?.textContent?.includes(customTheme === "gruvbox" ? "Gruvbox" : "Night Owl") : !$("#user-theme"));
        await sleep(200);
        const name = `select-${customTheme || "default"}-${theme}`;
        const select = $(".theme-pick select") as HTMLSelectElement;
        const menu = await open(select);
        assertPalette(menu, name);
        await shot(name);
        await press("Escape", select);
        log(!popup() && !!$(".settings"), `${name}: Escape dismisses only the choices`);
        log(document.activeElement === select, `${name}: focus stays on the select`);
      }
    }
    // The ungrouped appearance menu was also reported with GTK's system palette.
    for (const theme of ["dark", "light"] as const) {
      await updateSettings({ theme, customTheme: "gruvbox" });
      const select = $("#opt-theme") as HTMLSelectElement;
      const menu = await open(select);
      assertPalette(menu, `appearance ${theme}`, false);
      log($$("[role=option]", menu).map((o) => o.textContent).join(",") === "Follow system,Dark,Light", "appearance menu preserves its three choices");
      await shot(`select-appearance-gruvbox-${theme}`);
      await press("Escape", select);
    }
    const font = $(".font-pick select") as HTMLSelectElement;
    await open(font);
    await until(() => font.options.length > 3);
    await press("End", font);
    await sleep(100);
    const activeFont = $(".themed-select-option.active", popup()!);
    log(!!activeFont && activeFont.getAttribute("aria-disabled") !== "true", "font navigation skips disabled loading options");
    const fontBox = popup()!.getBoundingClientRect(), activeBox = activeFont?.getBoundingClientRect();
    log(!!activeBox && activeBox.top >= fontBox.top && activeBox.bottom <= fontBox.bottom, "long font list scrolls the keyboard choice into view");
    await shot("select-font-list-gruvbox-light");
    await press("Escape", font);
    // Exercise the real theme-file backend and the async list refresh on opening.
    const cfg = getState().info!.configDir;
    await t.api.createDir(cfg, "themes").catch(() => {});
    await t.api.saveText(cfg + "/themes/dropdown-test.css", ':root:root { --bg-2: rgb(34, 48, 42); --fg: rgb(226, 239, 204); --accent: rgb(220, 160, 50) !important; }');
    await updateSettings({ theme: "dark", customTheme: "dropdown-test" });
    await until(() => $("#user-theme")?.textContent?.includes("rgb(34, 48, 42)"));
    let select = $(".theme-pick select") as HTMLSelectElement;
    let menu = await open(select);
    await until(() => $$("[role=option]", popup()!).some((o) => o.textContent?.includes("dropdown-test")));
    menu = popup()!;
    assertPalette(menu, "file theme");
    log(menu.textContent?.includes("Themes folder") ?? false, "theme-file group loads through IPC while popup is open");
    await shot("select-file-theme-dark");
    await press("Escape", select);
    for (const vimKeys of [false, true]) {
      await updateSettings({ vimKeys });
      select = $(".theme-pick select") as HTMLSelectElement;
      select.focus();
      await press(" ", select);
      log(!!await until(popup), `Space opens choices with Vim keys ${vimKeys}`);
      await press("Home", select);
      await press("ArrowDown", select);
      await press("Enter", select);
      log(!!await until(() => getState().settings?.customTheme === "gruvbox"), `keyboard commits Gruvbox across UI/backend with Vim keys ${vimKeys}`);
      await press(" ", select);
      await until(popup);
      await press("n", select);
      await press("Enter", select);
      log(!!await until(() => getState().settings?.customTheme === "nightowl"), `type-ahead selects Night Owl with Vim keys ${vimKeys}`);
      await open(select);
      const beforePage = select.getAttribute("aria-activedescendant");
      await press("PageDown", select);
      log(select.getAttribute("aria-activedescendant") !== beforePage, "PageDown moves the highlighted option");
      log(getState().settings?.customTheme === "nightowl", "PageDown moves the popup cursor without committing the native select");
      await press("PageUp", select);
      log(getState().settings?.customTheme === "nightowl", "PageUp does not change the stored value");
      await press("End", select);
      await press("Escape", select);
      log(getState().settings?.customTheme === "nightowl", "Escape cancels the highlighted uncommitted choice");
    }
    select.focus();
    await press("F4", select);
    log(!!await until(popup), "F4 opens the themed popup instead of a native window");
    await press("F4", select);
    log(!popup(), "F4 toggles the popup closed");
    await setWindowSize(720, 600);
    select = $(".theme-pick select") as HTMLSelectElement;
    menu = await open(select);
    assertPalette(menu, "narrow window");
    await shot("select-nightowl-dark-narrow");
    const none = $$("[role=option]", menu).find((o) => o.textContent?.includes("None"));
    if (!none) throw new Error("None option missing");
    click(none);
    log(!!await until(() => getState().settings?.customTheme === "" && !$("#user-theme")), "pointer selection restores default colours");
    await open(select);
    click($(".settings h2") ?? $(".settings" )!);
    log(!!await until(() => !popup()), "outside click dismisses the menu");
    // Deterministic async-options regression: an inserted item must not change the
    // highlighted value while a font/theme list is being refreshed.
    const { createElement: h } = await import("react");
    const { createRoot } = await import("react-dom/client");
    const { flushSync } = await import("react-dom");
    const { ThemedSelect } = await import("./components/ThemedSelect");
    const host = document.createElement("div");
    host.style.cssText = "position:fixed;top:12px;left:12px;z-index:11000";
    document.body.append(host);
    const root = createRoot(host);
    let chosen = "";
    const render = (insert: boolean) => flushSync(() => root.render(h(ThemedSelect, { defaultValue: "b", "aria-label": "Async options regression", onChange: (e) => { chosen = e.currentTarget.value; } },
      ["a", "b", ...(insert ? ["inserted"] : []), "c"].map((value) => h("option", { key: value, value }, value)))));
    try {
      render(false);
      const probeSelect = host.querySelector("select")!;
      await open(probeSelect);
      await press("ArrowDown", probeSelect);
      log($(".themed-select-option.active", popup()!)?.textContent === "c", "async probe initially highlights c");
      render(true);
      await sleep(60);
      log($(".themed-select-option.active", popup()!)?.textContent === "c", "inserting an option preserves highlighted value rather than index");
      await press("Enter", probeSelect);
      log(chosen === "c", "Enter commits the same value after async insertion");
    } finally { flushSync(() => root.unmount()); host.remove(); }
  } catch (error) {
    log(false, "themed dropdown integration", String(error));
  }
  await t.finish();
}
