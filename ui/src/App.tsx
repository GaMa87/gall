import { PreviousBugReport } from "./components/BugReport";
import { ReportCapture } from "./components/ReportCapture";
import { checkUpdates } from "./state/updates";
// Root component: theme application, global shortcuts, layout shell.
import { useEffect, useMemo } from "react";
import { api, fileSrc } from "./api/ipc";
import { buildKeymap, chordOf, keyboardProfile, type Action } from "./core/keymap";
import { controller } from "./state/controllers";
import { buildLeaderTree, buildVimTree, CTRL_TAP, leaderToken, resolve, vimToken } from "./core/vim";
import { run } from "./state/run";
import { join } from "./core/path";
import * as A from "./state/actions";
import { getState, setState, useStore } from "./state/store";
import { Workspace } from "./components/Browser";
import { ContextMenu, Notifications, Overlays } from "./components/Overlays";
import { TitleBar } from "./components/TitleBar";
import { Sidebar } from "./components/Sidebar";
import { DragHintLabel } from "./components/DragHint";
import { WhichKey } from "./components/WhichKey";
import { BUILTIN_THEMES } from "./themes";

function useTheme() {
  const settings = useStore((s) => s.settings);
  const info = useStore((s) => s.info);
  const nonce = useStore((s) => s.themeNonce);
  useEffect(() => {
    if (!settings) return;
    const root = document.documentElement;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      const mode = settings.theme === "system" ? (mq.matches ? "dark" : "light") : settings.theme;
      root.dataset.theme = mode;
    };
    apply();
    mq.addEventListener("change", apply);
    root.style.setProperty("--accent", settings.accent);
    root.style.setProperty("--font-size", `${settings.fontSize}px`);
    root.style.setProperty("--row-pad", `${settings.rowPadding}px`);
    // A family name with spaces must be quoted for CSS ("Segoe UI"); a list stays as written.
    const fam = settings.fontFamily.trim();
    const css = fam && !/[,"']/.test(fam) ? `"${fam}"` : fam;
    if (css) root.style.setProperty("--font", `${css}, system-ui, sans-serif`);
    else root.style.removeProperty("--font");
    const st = settings.fontStyle ?? "regular";
    root.style.setProperty("--font-weight", st === "bold" || st === "boldItalic" ? "700" : "400");
    root.style.setProperty("--font-style", st === "italic" || st === "boldItalic" ? "italic" : "normal");
    root.style.setProperty("--anim", settings.animations ? "150ms" : "0ms");
    document.body.classList.toggle("thin-scroll", settings.thinScrollbars);
    document.body.classList.toggle("no-anim", !settings.animations);
    return () => mq.removeEventListener("change", apply);
  }, [settings]);

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
    fetch(`${fileSrc(join(join(info.configDir, "themes"), name + ".css"))}?v=${nonce}`)
      .then((r) => (r.ok ? r.text() : null))
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
}

/** bootstrap() ran (see the effect in App). */
let booted = false;

export function App() {
  const ready = useStore((s) => !!s.session && !!s.settings);
  const shortcuts = useStore((s) => s.settings?.shortcuts);
  const profile = useStore((s) => keyboardProfile(s.settings));
  const keymap = useMemo(() => buildKeymap(shortcuts ?? {}, profile), [shortcuts, profile]);
  const vimOn = useStore((s) => s.settings?.vimKeys ?? false);
  const leaderSetting = useStore((s) => (s.settings ? (s.settings.vimKeys ? s.settings.leaderKey || "Space" : s.settings.leaderKeyBasic ?? "Ctrl Ctrl") : ""));
  const leaderTok = leaderToken(leaderSetting);
  const vimTree = useMemo(() => (vimOn ? buildVimTree(leaderTok) : buildLeaderTree(leaderTok)), [vimOn, leaderTok]);
  useTheme();

  useEffect(() => {
    // Once per page: React StrictMode (dev builds) runs effects twice, which would
    // register every backend listener twice and start the self-test twice at once.
    if (booted) return;
    booted = true;
    // The window starts hidden: show it once the loaded UI is rendered, also after
    // a failure. Not requestAnimationFrame: a hidden webview draws no frames, so
    // it would never fire. React renders the store update before this timer.
    const reveal = () => setTimeout(() => { api.windowReady().catch(() => {}); }, 30);
    const t0 = performance.now();
    api.uiLog("info", `startup: UI script running ${Math.round(t0)} ms after page start`).catch(() => {});
    A.bootstrap()
      .then(() => api.uiLog("info", `startup: bootstrap took ${Math.round(performance.now() - t0)} ms`).catch(() => {})).catch((e) => A.fail(`Startup failed: ${e}`)).finally(() => { reveal(); if (!import.meta.env.DEV) void checkUpdates(false); });
  }, []);

  useEffect(() => {
    const isField = (t: EventTarget | null) => {
      const el = t as HTMLElement | null;
      return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
    };
    // Text selected in a preview / details area: Ctrl+C and right-click copy the text
    // there, not the focused file (the file list keeps the focus while text is selected).
    const TEXT_AREAS = ".pv-text, .pv-md, .pv-archive, .pv-note, .props, .fif-pv-code, .pv-fb-card, .pv-pdf";
    let lastDown: Element | null = null;
    const textArea = (el: EventTarget | null) => (el instanceof Element ? el.closest(TEXT_AREAS) : null);
    const selectedText = (area: Element | null) => {
      const s = window.getSelection();
      if (!area || !s || s.isCollapsed || !area.contains(s.anchorNode)) return "";
      return s.toString().trim() ? s.toString() : "";
    };
    const onDown = (e: MouseEvent) => { lastDown = e.target as Element; };
    const onKey = (e: KeyboardEvent) => {
      if (getState().overlay || getState().menu) {
        if (getState().keySeq.length) setState({ keySeq: [] });
        return;
      }
      if (isField(e.target)) return;
      if (keymap.get(chordOf(e)) === "copy" && selectedText(textArea(lastDown))) return;
      // Vim keys and leader sequences (which-key) come first. Without vim keys only the
      // leader (and a sequence it started) is handled here; letters keep filtering.
      const pending = getState().keySeq.length > 0;
      if (vimOn || pending || (leaderTok !== CTRL_TAP && vimToken(e) === leaderTok)) {
        const seq = getState().keySeq;
        if (seq.length && e.key === "Escape") {
          e.preventDefault();
          setState({ keySeq: [] });
          return;
        }
        const tok = vimToken(e);
        if (tok === null && seq.length && ["Shift", "Control", "Alt", "Meta", "CapsLock"].includes(e.key)) return;
        // Visual mode: y yanks and d / x cut the selection at once, as in Vim.
        const VISUAL_KEYS: Record<string, Action> = { y: "copy", d: "cut", x: "cut" };
        if (vimOn && !seq.length && tok && VISUAL_KEYS[tok] && controller(A.activeBrowser())?.visual?.()) {
          e.preventDefault();
          e.stopPropagation();
          run(VISUAL_KEYS[tok]);
          return;
        }
        if (tok !== null) {
          const r = resolve(vimTree, [...seq, tok]);
          if (r.kind !== "none" || seq.length) {
            e.preventDefault();
            e.stopPropagation();
            if (r.kind === "pending") setState({ keySeq: [...seq, tok] });
            else {
              setState({ keySeq: [] });
              if (r.kind === "action") run(r.action);
            }
            return;
          }
        }
      }
      const action = keymap.get(chordOf(e));
      if (!action) return;
      e.preventDefault();
      run(action);
    };
    const onContext = (e: MouseEvent) => {
      if (isField(e.target)) return;
      e.preventDefault();
      const area = textArea(e.target);
      if (!area) return;
      const text = selectedText(area);
      A.showMenu(e.clientX, e.clientY, [
        { label: "Copy", icon: "copy", action: "copy", disabled: !text, run: () => { api.clipSetText(text).catch(A.fail); } },
        { label: "Select all", run: () => window.getSelection()?.selectAllChildren(area) },
      ]);
    };
    const onMouse = (e: MouseEvent) => {
      // Mouse buttons 4/5 = back/forward (N-07).
      if (e.button === 3 || e.button === 4) {
        e.preventDefault();
        A.sessionCmd({ op: e.button === 3 ? "back" : "forward", b: A.activeBrowser() });
      }
    };
    // Ctrl as leader: a tap is Ctrl pressed and released alone within TAP_MS; "Ctrl Ctrl"
    // needs two taps within DOUBLE_MS. Any other key, click or wheel turn in between cancels it.
    const TAP_MS = 350, DOUBLE_MS = 450;
    let downAt = 0, clean = false, lastTap = 0;
    const onTapDown = (e: KeyboardEvent) => {
      if (e.key === "Control" && !e.repeat && !e.shiftKey && !e.altKey && !e.metaKey) {
        downAt = performance.now();
        clean = true;
      } else if (e.key !== "Control") {
        clean = false;
        lastTap = 0;
      }
    };
    const onTapUp = (e: KeyboardEvent) => {
      if (e.key !== "Control" || !clean || leaderTok !== CTRL_TAP) return;
      clean = false;
      const now = performance.now();
      if (now - downAt > TAP_MS) return;
      if (leaderSetting === "Ctrl Ctrl" && now - lastTap > DOUBLE_MS) {
        lastTap = now;
        return;
      }
      lastTap = 0;
      if (getState().overlay || getState().menu || isField(document.activeElement)) return;
      setState({ keySeq: getState().keySeq.length ? [] : [CTRL_TAP] });
    };
    const cancelTap = () => {
      clean = false;
      lastTap = 0;
    };
    // A click (or leaving the window) also ends a pending leader / vim sequence: its
    // which-key popup would stay and swallow the next key.
    const cancelSeq = () => {
      cancelTap();
      if (getState().keySeq.length) setState({ keySeq: [] });
    };
    window.addEventListener("keydown", onTapDown, true);
    window.addEventListener("keyup", onTapUp, true);
    window.addEventListener("mousedown", cancelSeq, true);
    window.addEventListener("mousedown", onDown, true);
    window.addEventListener("blur", cancelSeq);
    // Ctrl+wheel zooms the thumbnails: releasing Ctrl after it is no tap.
    window.addEventListener("wheel", cancelTap, { capture: true, passive: true });
    window.addEventListener("keydown", onKey);
    window.addEventListener("contextmenu", onContext);
    window.addEventListener("mouseup", onMouse);
    // Prevent the webview from navigating when files are dropped outside a target.
    const stop = (e: DragEvent) => e.preventDefault();
    window.addEventListener("dragover", stop);
    window.addEventListener("drop", stop);
    return () => {
      window.removeEventListener("keydown", onTapDown, true);
      window.removeEventListener("keyup", onTapUp, true);
      window.removeEventListener("mousedown", cancelSeq, true);
      window.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("blur", cancelSeq);
      window.removeEventListener("wheel", cancelTap, true);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("contextmenu", onContext);
      window.removeEventListener("mouseup", onMouse);
      window.removeEventListener("dragover", stop);
      window.removeEventListener("drop", stop);
    };
  }, [keymap, vimOn, vimTree, leaderTok, leaderSetting]);

  if (!ready) return <div style={{ padding: 20, color: "var(--fg-dim)" }}>Loading…</div>;
  return (
    <div className="frame">
    <TitleBar />
    <div className="app">
      <Sidebar />
      <div className="workspace" style={{ flexDirection: "column" }}>
        <Workspace />
        <Notifications />
      </div>
      <Overlays /><PreviousBugReport /><ReportCapture />
      <ContextMenu />
      <DragHintLabel />
      <WhichKey tree={vimTree} />
    </div>
    </div>
  );
}
