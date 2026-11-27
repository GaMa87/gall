import type { gitSelftest } from "./selftest";
import { t as tr, tt } from "./i18n";

/** Language switching: the sidebar and title bar follow the setting and Omarchy's language, with RTL. */
export async function runI18nSelfTest(t: typeof gitSelftest) {
  const { $, $$, until, sleep, shot, setWindowSize, log, updateSettings, finish, key } = t;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const { setState } = await import("./state/store");
  const errors: string[] = [];
  window.addEventListener("error", (e) => errors.push(e.message));
  window.addEventListener("unhandledrejection", (e) => errors.push(String(e.reason)));
  const headings = () => $$(".group-head .title").map((el) => el.textContent ?? "");
  const html = document.documentElement;
  const omarchy = (language: { code: string; formatLocale: string; direction: "ltr" | "rtl" }) => ({
    active: true, themeMode: "dark", paletteTokens: {}, customCss: null, rounding: 8, glassy: false, monospaceFont: null, tilingWm: false, language,
  });
  try {
    await getCurrentWindow().unmaximize();
    await setWindowSize(1100, 760);
    await updateSettings({ keyboardProfile: "explorer", shortcuts: {}, language: "en", theme: "dark" });
    log(!!(await until(() => headings().includes("Drives"))), "English: the sidebar says Drives", headings().join(","));
    await shot("i18n-en");
    t.note("SCREENSHOT i18n-en.png: expected the English sidebar.");

    await updateSettings({ language: "sl" });
    log(!!(await until(() => headings().includes("Pogoni"))), "Slovenian: the sidebar says Pogoni", headings().join(","));
    log(html.lang === "sl" && html.dir === "ltr", "Slovenian: lang and dir are set", `${html.lang}/${html.dir}`);
    log($("button[title^='Nastavitve']") !== null, "Slovenian: the settings button is translated");
    await shot("i18n-sl");
    t.note("SCREENSHOT i18n-sl.png: expected Slovenian headings (Pogoni, Omrežje, Zgodovina) and Slovenian button titles.");

    // Context menus follow the language.
    const target = $$(".fp-list [data-path]")[0];
    if (target) {
      const box = target.getBoundingClientRect();
      target.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: box.left + 40, clientY: box.top + 8 }));
      const menuText = () => $(".menu")?.textContent ?? "";
      log(!!(await until(() => /Odpri/.test(menuText()) && /Kopiraj/.test(menuText()))), "Slovenian: the file context menu is translated", menuText().slice(0, 120));
      await shot("i18n-sl-menu");
      t.note("SCREENSHOT i18n-sl-menu.png: expected the file context menu in Slovenian (Odpri, Kopiraj, Prilepi, Preimenuj, Premakni v koš …) without clipped labels.");
      key("Escape");
      await until(() => !$(".menu"));
    }

    // The Settings window is translated, and its search finds Slovenian words.
    const A = await import("./state/actions");
    const { openOverlay } = A;
    openOverlay({ type: "settings" });
    const sText = () => $(".modal.settings")?.textContent ?? "";
    log(!!(await until(() => /Videz/.test(sText()) && /Končano/.test(sText()))), "Slovenian: the Settings window is translated", sText().slice(0, 100));
    await shot("i18n-sl-settings");
    t.note("SCREENSHOT i18n-sl-settings.png: expected Settings in Slovenian (Videz, Pogled, Iskanje …) without clipped labels.");
    openOverlay(null);
    await until(() => !$(".modal.settings"));

    // The command palette and a fixed dialog follow the language.
    openOverlay({ type: "quickOpen", initial: ">Preimenuj" });
    const qText = () => $(".qo-list")?.textContent ?? "";
    log(!!(await until(() => /Preimenuj/.test(qText()))), "Slovenian: the palette lists Preimenuj", qText().slice(0, 100));
    await shot("i18n-sl-palette");
    t.note("SCREENSHOT i18n-sl-palette.png: expected command names in Slovenian with their shortcuts, without clipping.");
    openOverlay(null);
    await until(() => !$(".qo"));
    void A.confirm("Delete permanently?", "x", "Delete", true);
    log(!!(await until(() => /Trajno izbrisati\?/.test($(".modal h3")?.textContent ?? ""))), "Slovenian: a fixed dialog title is translated", $(".modal")?.textContent?.slice(0, 80));
    await shot("i18n-sl-dialog");
    t.note("SCREENSHOT i18n-sl-dialog.png: expected the dialog 'Trajno izbrisati?' with the buttons Prekliči and Izbriši.");
    key("Escape");
    await until(() => !$(".modal"));

    // Automatic: the language comes from Omarchy.
    await updateSettings({ language: "auto" });
    setState({ omarchy: omarchy({ code: "en", formatLocale: "en-US", direction: "ltr" }) });
    log(!!(await until(() => headings().includes("Drives"))), "Automatic with Omarchy English: English", headings().join(","));
    setState({ omarchy: omarchy({ code: "sl", formatLocale: "sl-SI", direction: "ltr" }) });
    log(!!(await until(() => headings().includes("Pogoni"))), "Automatic: follows Omarchy's language live", headings().join(","));

    // Right to left: no catalog yet, but the layout mirrors.
    await updateSettings({ language: "ar" });
    log(!!(await until(() => html.dir === "rtl")), "Arabic: the document is right to left", `${html.lang}/${html.dir}`);
    await sleep(300);
    log(html.scrollWidth <= window.innerWidth + 1, "Arabic: no horizontal page overflow", `${html.scrollWidth}/${window.innerWidth}`);
    await shot("i18n-rtl");
    t.note("SCREENSHOT i18n-rtl.png: expected a mirrored layout (sidebar on the right) without clipped or overlapping controls; text stays English until an Arabic catalog exists.");

    await updateSettings({ language: "en" });
    setState({ omarchy: null });
    log(!!(await until(() => html.dir === "ltr" && headings().includes("Drives"))), "Back to English left to right");

    // Drives that share a partition with another row nest under it and can be folded.
    const kids = () => $$(".sidebar .drive.child").map((e) => $("b", e)?.textContent ?? "");
    const parentBtn = () => $(".sidebar .drive-group .drive:not(.child) .drive-toggle") as HTMLElement | null;
    if (parentBtn()) {
      log(kids().length > 0 && $$(".sidebar .drive-group").every((g) => !g.querySelector(".drive.child") || g.firstElementChild?.matches(".drive:not(.child)")),
        "drives on the same partition nest under their parent", kids().join(","));
      await shot("sidebar-drives-grouped");
      t.note("SCREENSHOT sidebar-drives-grouped.png: expected the bind mounts indented under their parent drive, with a chevron on the parent only.");
      parentBtn()!.click();
      log(!!(await until(() => kids().length === 0)) && localStorage.getItem("sidebar:driveGroups") !== "[]", "folding the parent hides the nested rows and remembers it");
      await shot("sidebar-drives-folded");
      t.note("SCREENSHOT sidebar-drives-folded.png: expected the parent drive alone with a right-pointing chevron.");
      parentBtn()!.click();
      log(!!(await until(() => kids().length > 0)), "unfolding shows them again");
    } else t.note("SKIPPED drive grouping: this machine has no drives that share a partition.");

    await getCurrentWindow().unmaximize();
    await setWindowSize(1100, 760);
    for (const code of ["de", "es"] as const) {
      await updateSettings({ keyboardProfile: "explorer", language: code, theme: "dark" });
      await until(() => html.lang === code && headings().includes(tr("sidebar.drives")));
      const driveLabel = tr("sidebar.drives");
      log(html.lang === code && html.dir === "ltr", `${code}: language and direction are set`, `${html.lang}/${html.dir}`);
      log(driveLabel !== "Drives" && !!(await until(() => headings().includes(driveLabel))), `${code}: the sidebar drive heading is translated`, driveLabel);
      log(tt("Network") !== "Network" && tt("History") !== "History", `${code}: network and history headings are translated`);
      await shot(`i18n-${code}-main`);
      t.note(`SCREENSHOT i18n-${code}-main.png: expected translated sidebar headings and title-bar labels.`);

      const file = $$(".fp-list [data-path]")[0];
      if (file) {
        const box = file.getBoundingClientRect();
        file.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: box.left + 40, clientY: box.top + 8 }));
        const menuText = () => $(".menu")?.textContent ?? "";
        const openText = tt("Open");
        const copyText = tt("Copy");
        log(openText !== "Open" && copyText !== "Copy"
          && !!(await until(() => menuText().includes(openText) && menuText().includes(copyText))),
        `${code}: common context-menu actions are translated`, menuText().slice(0, 120));
        await shot(`i18n-${code}-menu`);
        t.note(`SCREENSHOT i18n-${code}-menu.png: expected Open and Copy actions in ${code}.`);
        key("Escape");
        await until(() => !$(".menu"));
      } else {
        log(false, `${code}: a file row is available for the context menu`);
      }

      openOverlay({ type: "settings", section: "appearance" });
      const settingsText = () => $(".modal.settings")?.textContent ?? "";
      const appearance = tt("Appearance");
      const done = tt("Done");
      log(appearance !== "Appearance" && done !== "Done"
        && !!(await until(() => settingsText().includes(appearance) && settingsText().includes(done))),
      `${code}: Settings labels are translated`);
      await shot(`i18n-${code}-settings`);
      t.note(`SCREENSHOT i18n-${code}-settings.png: expected translated Settings labels and long descriptions without clipping.`);
      if (code === "de") {
        const textBlocks = $$(".settings .opt-text, .settings .section-head, .settings .shead-title, .settings .ssearch");
        const clipped = textBlocks.filter((el) => el.getBoundingClientRect().width > 0 && el.scrollWidth > el.clientWidth + 1);
        log(clipped.length === 0, "German Settings text wraps without horizontal clipping", `${clipped.length} overflowing text blocks`);
      }
      openOverlay(null);
      await until(() => !$(".modal.settings"));

      const rename = tt("Rename");
      openOverlay({ type: "quickOpen", initial: `>${rename}` });
      const quickOpenText = () => $(".qo-list")?.textContent ?? "";
      log(rename !== "Rename" && !!(await until(() => quickOpenText().includes(rename))),
        `${code}: the translated command is searchable`, quickOpenText().slice(0, 100));
      await shot(`i18n-${code}-palette`);
      t.note(`SCREENSHOT i18n-${code}-palette.png: expected the translated Rename command and shortcut without clipping.`);
      openOverlay(null);
      await until(() => !$(".qo"));
    }
    await updateSettings({ language: "en" });
    log(!!(await until(() => html.lang === "en" && headings().includes("Drives"))), "Return to English after German and Spanish checks");
  } catch (e) {
    log(false, "i18n scenarios", String(e));
  }
  log(!errors.length, "no uncaught i18n errors", errors.join("; "));
  await finish();
}
