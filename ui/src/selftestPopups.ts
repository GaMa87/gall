// Placement of every menu, submenu and dropdown that a key opens (docs/popup-placement-audit.md). Each scenario first uses
// the mouse far away from the item, then opens the surface with the key, and checks that it sits next to its item, does not
// cover it, and is inside the window. Run at several scales: GALL_SELFTEST=popup-placement@1.5.
import { getCurrentWebview } from "@tauri-apps/api/webview";
import * as A from "./state/actions";
import { join } from "./core/path";
import { controller } from "./state/controllers";
import type { gitSelftest } from "./selftest";

type Box = { left: number; top: number; right: number; bottom: number };

export async function runPopupPlacementSelfTest(t: typeof gitSelftest) {
  const { $, $$, key, click, mouse, until, sleep, shot, setWindowSize, log, focusPane, activePath, updateSettings, getState } = t;
  const root = activePath(0), docs = join(root, "Documents"), repo = join(root, "Projects");
  const round = (b: Box) => [b.left, b.top, b.right, b.bottom].map(Math.round).join(",");
  const box = (el: Element): Box => { const r = el.getBoundingClientRect(); return { left: r.left, top: r.top, right: r.right, bottom: r.bottom }; };
  const overlap = (a: Box, b: Box) => Math.max(0, Math.min(a.right, b.right) - Math.max(a.left, b.left)) * Math.max(0, Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top));
  /** Distance between two boxes; 0 when they touch. */
  const distance = (a: Box, b: Box) => Math.hypot(Math.max(0, a.left - b.right, b.left - a.right), Math.max(0, a.top - b.bottom, b.top - a.bottom));
  const inside = (b: Box) => b.left >= 0 && b.top >= 0 && b.right <= innerWidth + 1 && b.bottom <= innerHeight + 1;
  /** The part of a row that the menu must not cover: from its start to the end of its text. */
  const textBox = (row: Element, text: Element | null): Box => {
    const r = box(row);
    if (!text) return r;
    const range = document.createRange(); range.selectNodeContents(text);
    return { ...r, right: Math.min(range.getBoundingClientRect().right, text.getBoundingClientRect().right) };
  };
  const check = (name: string, surface: Element | null, anchor: Box | null, options: { mayCover?: boolean } = {}) => {
    if (!surface || !anchor) { log(false, name, surface ? "no anchor element" : "did not open"); return; }
    const s = box(surface);
    const tell = `surface ${round(s)} anchor ${round(anchor)} window ${innerWidth}x${innerHeight}`;
    log(inside(s), `${name}: inside the window`, tell);
    log(distance(s, anchor) <= 16, `${name}: next to its item`, tell);
    const w = s.right - s.left, h = s.bottom - s.top, edge = 10;
    const room = innerWidth - anchor.right >= w + edge || anchor.left >= w + edge || innerHeight - anchor.bottom >= h + edge || anchor.top >= h + edge;
    if (!options.mayCover && room) log(overlap(s, anchor) < 1, `${name}: does not cover its item`, tell);
  };
  /** The mouse was last used far from any item: moved, pressed and released in the empty corner of the sidebar. */
  const far = () => {
    const el = $(".sidebar") ?? document.body, at = { clientX: 4, clientY: innerHeight - 4, pageX: 4 } as MouseEventInit;
    for (const type of ["pointermove", "pointerdown", "pointerup"]) el.dispatchEvent(new PointerEvent(type, { bubbles: true, ...at }));
    for (const type of ["mousemove", "mousedown", "mouseup"]) mouse(el, type, at);
  };
  const menu = () => until(() => $(".menu"), 2500);
  const settled = async () => { const el = await menu(); await sleep(260); return el; };
  const close = async () => {
    for (let i = 0; i < 4 && getState().menu; i++) { key("Escape", {}, document.body); await sleep(120); }
    await until(() => !$(".menu"));
  };
  const item = (b: number, name: string) => $$(".fp-list [data-path]", $$(".browser")[b] ?? document).find((r) => r.getAttribute("data-path")!.replace(/\\/g, "/").endsWith("/" + name)) ?? null;
  const rowBox = (row: Element | null) => (row ? textBox(row, row.querySelector(".name .txt, .gname")) : null);
  const focusItem = async (b: number, path: string) => {
    far();
    await focusPane(b);
    controller(b)!.select([path], path);
    await sleep(200);
  };
  /** Enter or Space on a focused button: the browser makes a click event that has no pointer position. */
  const pressButton = async (el: HTMLElement | null) => { if (!el) return; far(); el.focus(); await sleep(60); el.click(); };
  const errors: string[] = [];
  window.addEventListener("error", (e) => errors.push(e.message));
  window.addEventListener("unhandledrejection", (e) => errors.push(String(e.reason)));
  const baseZoom = () => (window as unknown as { __selftestZoom?: number }).__selftestZoom ?? 1;
  const zoom = async (value: number) => { await getCurrentWebview().setZoom(value).catch(() => {}); await sleep(500); };
  const narrow = async (on: boolean) => zoom(on ? Math.max(baseZoom(), innerWidth * baseZoom() / 640) : baseZoom());

  const filePane = async (tag: string, vim: boolean) => {
    const last = join(docs, "file60.txt"), first = join(docs, "file01.txt");
    await A.navigate(0, docs); await A.navigate(1, docs);
    const shown = (b: number) => !!($$(".browser")[b]?.querySelector(".fp-list") as HTMLElement | null)?.offsetWidth;
    await until(() => [0, 1].every((b) => !shown(b) || $$(".fp-list [data-path]", $$(".browser")[b]).length > 5));
    for (const b of [0, 1]) {
      if (!shown(b)) continue;
      for (const [where, path] of [["first", first], ["last", last]] as const) {
        await focusItem(b, path);
        key("F10", { shiftKey: true });
        const el = await settled();
        const name = `file-${where}-pane${b}${tag}`;
        check(`Shift+F10 on the ${where} file of pane ${b}${tag}`, el, rowBox(item(b, where === "first" ? "file01.txt" : "file60.txt")));
        await shot(`popup-${name}`);
        await close();
      }
    }
    // The focused file was scrolled out of the (virtualised) list: the key brings it back and the menu opens at it.
    await focusItem(0, first);
    const list = $(".fp-list", $$(".browser")[0])!;
    key("F10", { shiftKey: true }); await settled(); await close();
    list.scrollTop = list.scrollTop > 0 ? 0 : list.scrollHeight; await sleep(250);
    const hidden = item(0, "file01.txt");
    log(!hidden || overlap(box(hidden), box(list)) < 1, `the focused file is scrolled out of sight${tag}`);
    key("ContextMenu");
    check(`ContextMenu key after scrolling${tag}`, await settled(), rowBox(item(0, "file01.txt")));
    await shot(`popup-file-after-scroll${tag}`);
    await close();
    if (vim) {
      await focusItem(0, first);
      for (const k of [" ", "o", "m"]) { key(k); await sleep(80); }
      check(`Space o m${tag}`, await settled(), rowBox(item(0, "file01.txt")));
      await close();
    }
    // The menu opened under a mouse at rest: nothing is highlighted (Enter would run it) until the mouse moves.
    await focusItem(0, first);
    key("F10", { shiftKey: true });
    if (await settled()) {
      const under = $$(".menu .mi").filter((mi) => !mi.classList.contains("disabled"))[2], u = box(under), rest = { clientX: u.left + 20, clientY: u.top + 6 };
      // The real pointer of this desktop may lie on the menu too; then the synthetic one would be a second mouse.
      const real = !!$(".menu:hover");
      if (!real) { mouse(under, "mouseover", rest); mouse(under, "mousemove", rest); await sleep(60); }
      log(!$(".menu .mi.kb"), `a mouse at rest under a menu opened with a key highlights nothing${tag}`, $(".menu .mi.kb")?.textContent ?? "");
      if (real) { mouse(under, "mousemove", rest); }
      mouse(under, "mousemove", { ...rest, clientX: rest.clientX + 3 }); await sleep(60);
      log(under.classList.contains("kb"), `a moved mouse highlights the item under it${tag}`);
      await close();
    } else log(false, `a mouse at rest under a menu opened with a key${tag}`, "did not open");
    // Submenus: beside the parent menu, left of it in the pane at the right edge; never on top of it.
    for (const b of [0, 1]) {
      if (!shown(b)) continue;
      await focusItem(b, first);
      key("F10", { shiftKey: true });
      if (!await settled()) continue;
      const parentMenu = $(".menu")!;
      const target = $$(".menu .mi").findIndex((mi) => mi.querySelector("svg:last-child") && mi.lastElementChild?.tagName.toLowerCase() === "svg" && !mi.classList.contains("disabled"));
      for (let i = 0; i < 40 && $$(".menu .mi").indexOf($(".menu .mi.kb")!) !== target; i++) { key("ArrowDown", {}, document.body); await sleep(15); }
      const owner = $(".menu .mi.kb");
      key("ArrowRight", {}, document.body);
      await until(() => $$(".menu").length === 2, 1500); await sleep(260);
      const sub = $$(".menu")[1] ?? null;
      check(`submenu in pane ${b}${tag}`, sub, owner ? box(owner) : null, { mayCover: true });
      if (sub) {
        const a = box(sub), p = box(parentMenu), hidden = Math.max(0, Math.min(a.right, p.right) - Math.max(a.left, p.left));
        const roomy = p.left >= a.right - a.left || innerWidth - p.right >= a.right - a.left;
        if (roomy) log(hidden <= 8, `submenu in pane ${b}${tag}: does not cover its parent menu`, `${round(a)} over ${round(p)}`);
      }
      await shot(`popup-submenu-pane${b}${tag}`);
      await close();
    }
    // Header buttons pressed with Enter or Space.
    for (const b of [0, 1]) {
      const pane = $$(".browser")[b];
      if (!pane || !$(".fp-pill", pane)) continue;
      for (const [name, el] of [["sort", $('.fp-pill button:first-of-type', pane)], ["view", $('.fp-pill button:last-of-type', pane)], ["more", $(".fp-more", pane)]] as const) {
        if (!el || !(el as HTMLElement).offsetWidth) continue;
        await pressButton(el as HTMLElement);
        check(`${name} button of pane ${b} by key${tag}`, await settled(), box(el));
        if (b === 1) await shot(`popup-button-${name}-pane${b}${tag}`);
        await close();
      }
    }
    // A mouse still gets its menu at the pointer.
    const header = box($(".fp-head", $$(".browser")[0])!), at = { clientX: header.left + 60, clientY: header.top + 8 };
    mouse($(".fp-head", $$(".browser")[0])!, "contextmenu", { button: 2, ...at });
    const atPointer = await settled();
    if (atPointer) { const m = box(atPointer); const top = Math.max(4, Math.min(at.clientY, innerHeight - (m.bottom - m.top) - 4)); log(Math.abs(m.left - at.clientX) <= 2 && Math.abs(m.top - top) <= 2, `a right click opens the menu at the pointer${tag}`, `${round(m)} at ${Math.round(at.clientX)},${Math.round(at.clientY)}`); }
    else log(false, `a right click opens the menu at the pointer${tag}`, "did not open");
    await close();
  };

  const gitView = async (tag: string, vim: boolean) => {
    far();
    A.openOverlay({ type: "gitGraph", path: repo });
    const commit = (text: string) => $$(".git-graph-lines [data-commit]").find((el) => el.textContent?.includes(text)) ?? null;
    if (!await until(() => commit("second"), 10_000)) { log(false, `the Git view opens${tag}`); return; }
    if (!$(".gg-sidebar")) { const toggle = $("[aria-label='Toggle repository panels']"); if (toggle) { click(toggle); await until(() => $(".gg-sidebar")); } }
    await sleep(300);
    const gitBox = (row: Element | null) => (row ? textBox(row, row.querySelector(".gg-message") ?? row) : null);
    // The commit is clicked while the focus is in the sidebar (WebKit does not focus a clicked button).
    const side = $(".gg-sidebar .gg-side-row") as HTMLElement | null;
    for (const subject of ["second", "first"]) {
      side?.focus(); await sleep(150);
      click(commit(subject)!); await sleep(500);
      far();
      if (vim) {
        key("y", {}, document.activeElement);
        check(`y on “${subject}”${tag}`, await settled(), gitBox(commit(subject)));
        await shot(`popup-git-y-${subject}${tag}`);
        await close();
      }
      key("F10", { shiftKey: true }, document.activeElement);
      check(`Shift+F10 on “${subject}”${tag}`, await settled(), gitBox(commit(subject)));
      const labels = $$(".menu .lbl").map((el) => el.textContent ?? "");
      log(labels.some((l) => l.startsWith("Checkout selected commit")), `Shift+F10 on “${subject}”${tag}: the menu of a commit, not of the focused branch`, labels.slice(0, 3).join(" | "));
      await shot(`popup-git-menu-${subject}${tag}`);
      await close();
    }
    const details = $(".gcd-actions") as HTMLElement | null;
    if (details) {
      details.scrollIntoView({ block: "nearest" });
      await pressButton(details);
      check(`Commit actions button by key${tag}`, await settled(), box(details));
      await shot(`popup-git-details${tag}`);
      await close();
    } else log(false, `Commit actions button by key${tag}`, "no commit details");
    // A sidebar row that the keyboard selected.
    const branch = $$(".gg-sidebar .gg-side-row")[0] as HTMLElement | undefined;
    if (branch) {
      far(); branch.focus(); await sleep(300);
      key("F10", { shiftKey: true }, document.activeElement);
      const row = $(".gg-sidebar .gg-side-row.selected") ?? branch;
      check(`Shift+F10 on a branch of the sidebar${tag}`, await settled(), textBox(row, row));
      await shot(`popup-git-sidebar${tag}`);
      await close();
    }
    const actions = $(".gg-actions") as HTMLElement;
    await pressButton(actions);
    check(`Git Actions button by key${tag}`, await settled(), box(actions));
    await shot(`popup-git-actions${tag}`);
    await close();
    const picker = $("button[aria-label='Repository']") as HTMLButtonElement | null;
    if (picker) {
      far(); picker.focus(); await sleep(100);
      key("Enter", {}, picker);
      const el = await until(() => $(".rp-menu"), 1500); await sleep(260);
      check(`repository picker by key${tag}`, el, box(picker));
      await shot(`popup-git-repository-picker${tag}`);
      key("Escape", {}, picker); await until(() => !$(".rp-menu"));
    } else log(false, `repository picker by key${tag}`, "not shown (the fixture needs a submodule)");
    // Working Changes: i opens the actions of the selected file.
    const working = $(".gg-row.gg-working") as HTMLElement | null;
    if (working) {
      click(working);
      const panel = await until(() => $(".git-changes .gc-file"), 4000); await sleep(400);
      if (vim && panel) {
        far(); ($(".git-changes") as HTMLElement).focus(); await sleep(100);
        key("i", {}, $(".git-changes"));
        const file = $(".gc-file.selected");
        check(`i on a changed file${tag}`, await settled(), file ? textBox(file, file) : null);
        await shot(`popup-git-changes-i${tag}`);
        await close();
      }
      const files = $$(".gc-tools button").find((el) => el.textContent?.includes("File actions")) as HTMLButtonElement | undefined;
      if (files) {
        files.scrollIntoView({ block: "nearest" });
        await pressButton(files);
        check(`File actions button by key${tag}`, await settled(), box(files));
        await close();
      }
    }
    if (vim) {
      ($(".git-graph") as HTMLElement).focus(); await sleep(100);
      key("?", {}, $(".git-graph"));
      const help = await until(() => $(".gg-key-help"), 1500); await sleep(200);
      if (help) { const h = box(help), frame = box($(".git-graph")!); log(inside(h) && h.left >= frame.left - 1 && h.right <= frame.right + 1 && h.bottom <= frame.bottom + 1, `Git ? help is docked inside the workspace${tag}`, `${round(h)} in ${round(frame)}`); }
      else log(false, `Git ? help is docked inside the workspace${tag}`, "did not open");
      await shot(`popup-git-help${tag}`);
      const closeHelp = $("button[aria-label='Close Git shortcuts']"); if (closeHelp) click(closeHelp);
    }
    A.openOverlay(null);
    await until(() => !$(".git-graph"), 2000);
    if ($(".modal")) { key("Escape", {}, $(".modal")); await sleep(200); }
  };

  /** Surfaces that belong to the window and not to an item: in the middle (or docked), whole, wherever the mouse is. */
  const windowSurface = async (name: string, selector: string, tag: string, centred = true) => {
    const el = await until(() => $(selector), 2500); await sleep(260);
    if (!el) { log(false, `${name}${tag}`, "did not open"); return; }
    const b = box(el), middle = (b.left + b.right) / 2;
    log(inside(b), `${name}${tag}: inside the window`, `${round(b)} window ${innerWidth}x${innerHeight}`);
    if (centred) log(Math.abs(middle - innerWidth / 2) <= 12, `${name}${tag}: in the middle of the window`, round(b));
    await shot(`popup-${name.replace(/[^a-z]+/gi, "-").toLowerCase()}${tag}`);
  };
  const dialogs = async (tag: string, vim: boolean) => {
    far(); await focusPane(0);
    A.openOverlay({ type: "quickOpen" }); await windowSurface("quick open", ".qo", tag); A.openOverlay(null); await until(() => !getState().overlay);
    A.openOverlay({ type: "keys" }); await windowSurface("key help", ".kh", tag); A.openOverlay(null); await until(() => !getState().overlay);
    const answer = A.promptText("Name", "file01.txt"); await windowSurface("prompt", ".modal", tag);
    key("Escape", {}, $(".modal input")); await answer; await until(() => !getState().overlay);
    if (vim) {
      await focusPane(0); far();
      key(" "); await sleep(500);
      await windowSurface("which key", ".wk", tag);
      key("Escape"); await until(() => !$(".wk"));
    }
    A.openOverlay({ type: "settings", section: "appearance" });
    const selects = () => $$<HTMLSelectElement>(".settings select").filter((s) => s.offsetWidth > 0 && !s.disabled);
    await until(() => selects().length); await sleep(300);
    for (const [where, pick] of [["first", (l: HTMLSelectElement[]) => l[0]], ["last", (l: HTMLSelectElement[]) => l[l.length - 1]]] as const) {
      const select = pick(selects());
      select.scrollIntoView({ block: where === "last" ? "end" : "center" }); await sleep(250);
      far(); select.focus(); await sleep(60);
      key("F4", {}, select);
      const el = await until(() => $(".themed-select-menu"), 1500); await sleep(200);
      check(`${where} Settings dropdown by key${tag}`, el, box(select));
      await shot(`popup-settings-${where}${tag}`);
      key("Escape", {}, select); await until(() => !$(".themed-select-menu"));
    }
    A.openOverlay(null); await until(() => !getState().overlay);
  };
  const others = async (tag: string) => {
    for (const [name, el] of [["sidebar drives", $(".sidebar .group-head .icon-btn")], ["sidebar history", $(".sidebar-history .group-head .icon-btn")]] as const) {
      if (!el || !(el as HTMLElement).offsetWidth) continue;
      el.scrollIntoView({ block: "nearest" });
      await pressButton(el as HTMLElement);
      check(`${name} button by key${tag}`, await settled(), box(el));
      await shot(`popup-${name.replace(" ", "-")}${tag}`);
      await close();
    }
    // A context menu key on a focused element can come without a position.
    const tab = $(".tab");
    if (tab) {
      far();
      mouse(tab, "contextmenu", { button: 0, clientX: 0, clientY: 0 });
      check(`context menu key on a tab${tag}`, await settled(), box(tab));
      await shot(`popup-tab${tag}`);
      await close();
    }
  };

  try {
    await setWindowSize(1320, 900);
    await updateSettings({ theme: "dark", shortcuts: {}, indexRoots: [root], animations: false, vimKeys: false });
    for (const vim of [true, false]) {
      const tag = vim ? "-vim" : "";
      await updateSettings({ keyboardProfile: vim ? "vim" : "explorer", vimKeys: vim });
      await sleep(200);
      await filePane(tag, vim);
      await others(tag);
      await dialogs(tag, vim);
      await gitView(tag, vim);
    }
    // Grid view: the item is a tile, and the last one in a row sits at the edge of the pane.
    await A.navigate(0, docs); await focusPane(0);
    const viewButton = $('.fp-pill button:last-of-type', $$(".browser")[0]!) as HTMLElement;
    viewButton.click(); await settled();
    const grid = $$(".menu .mi").find((mi) => /grid/i.test(mi.textContent ?? ""));
    if (grid) {
      click(grid); await until(() => $(".fp-list .grid-cell"));
      await focusItem(0, join(docs, "file03.txt"));
      key("F10", { shiftKey: true });
      check("Shift+F10 on a grid tile", await settled(), rowBox(item(0, "file03.txt")));
      await shot("popup-grid-tile");
      await close();
      viewButton.click(); await settled(); click($$(".menu .mi")[0]!); await sleep(300);
    } else { await close(); log(false, "Shift+F10 on a grid tile", "no grid view in the View menu"); }
    // A narrow window.
    await narrow(true);
    log(innerWidth <= 700, "the narrow part runs in a narrow window", `${innerWidth} CSS px`);
    await filePane("-narrow", false);
    await gitView("-narrow", false);
    await narrow(false);
  } catch (e) {
    log(false, "popup placement scenario", e instanceof Error ? `${e.message}\n${e.stack ?? ""}` : String(e));
  }
  log(!errors.length, "no uncaught errors", errors.join("; "));
  await t.finish();
}
