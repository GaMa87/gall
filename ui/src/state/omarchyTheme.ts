// Theme application: Settings (theme, accent, font, animations), an optional custom theme CSS and,
// on Omarchy, the live desktop theme (palette, Hyprland rounding, monospace font, Glassy).
import { useEffect } from "react";
import { api, fileSrc } from "../api/ipc";
import { join } from "../core/path";
import { watchThemeContrast } from "./themeContrast";
import {
  fontStack, omarchyControlVars, omarchyCustomCss, omarchyFontStack, omarchyGlassyOn, omarchyMode, omarchyTextVars, omarchyThemeOn, paletteCssVars, radiusVars,
} from "../core/omarchyTheme";
import { BUILTIN_THEMES } from "../themes";
import * as A from "./actions";
import { getState, useStore } from "./store";

const OMARCHY_STYLE_ID = "omarchy-custom";

/**
 * Applies the Omarchy desktop theme to the document while Settings › Theme is "Omarchy" and
 * Omarchy is detected: palette custom properties, data-theme, corner radii, the monospace font and
 * the `omarchy-glassy` class. Everything it sets is removed again when it stops applying, and the
 * changes arrive live from `omarchy-state-changed` (no restart).
 */
function useOmarchyTheme() {
  const settings = useStore((s) => s.settings);
  const omarchy = useStore((s) => s.omarchy);
  useEffect(() => {
    if (!settings) return;
    const on = omarchyThemeOn(settings.theme, omarchy?.active);
    if (!on || !omarchy) return;
    const root = document.documentElement;
    const palette = paletteCssVars(omarchy.paletteTokens);
    const vars = { ...palette, ...omarchyTextVars(palette) };
    for (const [k, v] of Object.entries(vars)) root.style.setProperty(k, v);
    const controls = omarchyControlVars(vars);
    for (const [k, v] of Object.entries(controls)) root.style.setProperty(k, v);
    root.style.setProperty("--accent", controls["--om-control-accent"]);
    root.classList.add("omarchy-controls");
    root.dataset.theme = omarchyMode(omarchy.themeMode);
    const radii = omarchy.rounding != null ? radiusVars(omarchy.rounding) : null;
    if (radii) for (const [k, v] of Object.entries(radii)) root.style.setProperty(k, v);
    // The main theme effect has just set --font from the user's choice (or removed it).
    const font = omarchyFontStack({ follow: settings.omarchyFollowFont, userFont: settings.fontFamily, monospace: omarchy.monospaceFont });
    const prevFont = root.style.getPropertyValue("--font");
    if (font) root.style.setProperty("--font", font);
    const glassy = omarchyGlassyOn(true, settings.omarchyGlassy, omarchy.glassy);
    root.classList.toggle("omarchy-glassy", glassy);
    document.body.classList.toggle("omarchy-glassy", glassy);
    const css = omarchyCustomCss(omarchy.customCss);
    let styleEl: HTMLStyleElement | null = null;
    if (css) {
      styleEl = document.createElement("style");
      styleEl.id = OMARCHY_STYLE_ID;
      styleEl.textContent = css;
      document.head.appendChild(styleEl);
    }
    return () => {
      for (const k of Object.keys(vars)) root.style.removeProperty(k);
      for (const k of Object.keys(controls)) root.style.removeProperty(k);
      if (radii) for (const k of Object.keys(radii)) root.style.removeProperty(k);
      if (font) {
        if (prevFont) root.style.setProperty("--font", prevFont);
        else root.style.removeProperty("--font");
      }
      root.classList.remove("omarchy-glassy");
      root.classList.remove("omarchy-controls");
      document.body.classList.remove("omarchy-glassy");
      styleEl?.remove();
    };
  }, [settings, omarchy]);
}

export function useTheme() {
  const settings = useStore((s) => s.settings);
  const info = useStore((s) => s.info);
  const nonce = useStore((s) => s.themeNonce);
  const omarchyActive = useStore((s) => s.omarchy?.active);
  useEffect(() => {
    if (!settings) return;
    const root = document.documentElement;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    // The Omarchy theme (below) owns data-theme and the accent while it is active.
    const omarchyOn = omarchyThemeOn(settings.theme, getState().omarchy?.active);
    const apply = () => {
      if (omarchyOn) return;
      const mode = settings.theme === "dark" || settings.theme === "light" ? settings.theme : (mq.matches ? "dark" : "light");
      root.dataset.theme = mode;
    };
    apply();
    mq.addEventListener("change", apply);
    if (omarchyOn) root.style.removeProperty("--accent");
    else root.style.setProperty("--accent", settings.accent);
    root.style.setProperty("--font-size", `${settings.fontSize}px`);
    root.style.setProperty("--row-pad", `${settings.rowPadding}px`);
    // A family name with spaces must be quoted for CSS ("Segoe UI"); a list stays as written.
    const stack = fontStack(settings.fontFamily);
    if (stack) root.style.setProperty("--font", stack);
    else root.style.removeProperty("--font");
    const st = settings.fontStyle ?? "regular";
    root.style.setProperty("--font-weight", st === "bold" || st === "boldItalic" ? "700" : "400");
    root.style.setProperty("--font-style", st === "italic" || st === "boldItalic" ? "italic" : "normal");
    root.style.setProperty("--anim", settings.animations ? "150ms" : "0ms");
    document.body.classList.toggle("thin-scroll", settings.thinScrollbars);
    document.body.classList.toggle("no-anim", !settings.animations);
    return () => mq.removeEventListener("change", apply);
  }, [settings, omarchyActive]);

  // The layout never scrolls as a whole, but overflow:hidden containers can still be scrolled
  // programmatically (e.g. by scrollIntoView). Snap them back so the UI can't get stuck shifted.
  useEffect(() => {
    const pinned = [document.documentElement, document.body, document.getElementById("root")];
    const onScroll = (e: Event) => {
      const t = e.target === document ? document.documentElement : (e.target as HTMLElement);
      if ((pinned.includes(t) || t.classList?.contains("app")) && (t.scrollTop || t.scrollLeft)) { t.scrollTop = 0; t.scrollLeft = 0; }
    };
    document.addEventListener("scroll", onScroll, true);
    return () => document.removeEventListener("scroll", onScroll, true);
  }, []);

  // Optional theme: <config>/themes/<name>.css, else a built-in one (A-01). The CSS is read as
  // text and inlined as the last <style>, so it works the same in dev (Vite) and release builds.
  useEffect(() => {
    const name = settings?.customTheme ?? "";
    let cancelled = false;
    const apply = (css: string) => {
      if (cancelled) return;
      document.getElementById("user-theme")?.remove();
      if (!css) return;
      const el = document.createElement("style");
      el.id = "user-theme";
      el.textContent = css;
      document.head.appendChild(el);
    };
    if (!name || !info) {
      apply("");
      return;
    }
    const dir = join(info.configDir, "themes");
    // Built-in themes usually have no file of their own: look at the folder first, so the asset
    // protocol is not asked for a file that does not exist (it logs an error for every miss).
    const hasFile = BUILTIN_THEMES[name]
      ? api.listDir(dir, { showHidden: false, hideTemp: true })
          .then((es) => es.some((e) => !e.isDir && e.name.toLowerCase() === (name + ".css").toLowerCase()))
          .catch(() => false)
      : Promise.resolve(true);
    hasFile
      .then((yes) => (yes ? fetch(`${fileSrc(join(dir, name + ".css"))}?v=${nonce}`) : null))
      .then((r) => (r && r.ok ? r.text() : null))
      .catch(() => null)
      .then((css) => {
        if (cancelled) return;
        const found = css ?? BUILTIN_THEMES[name]?.css;
        if (found == null) A.notify(`Theme “${name}” not found in ${join(info.configDir, "themes")}`, "warn");
        apply(found ?? "");
      });
    return () => {
      cancelled = true;
    };
  }, [settings?.customTheme, info, nonce]);
  // Declared last: its effect runs after the ones above, so Omarchy values win over them.
  useOmarchyTheme();
  useEffect(() => watchThemeContrast(), []);
}
