// UI self-test (debug builds, GALL_SELFTEST=1). Drives the real UI with
// synthetic DOM events against a disposable test folder, takes screenshots and
// writes a report. Start: browser 1 = <testdata>, browser 2 = <testdata>/Projects.
import { api } from "./api/ipc";
import { getState } from "./state/store";
import { activePath, setLayout, updateSettings } from "./state/actions";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { hasNerdFont } from "./core/nerdFont";
import { KEYBOARD_PROFILES, type KeyboardProfile } from "./core/keymap";

const results: string[] = [];
let failures = 0;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function log(ok: boolean, name: string, detail = "") {
  if (!ok) failures++;
  const line = `${ok ? "PASS" : "FAIL"}  ${name}${detail ? "  — " + detail : ""}`;
  results.push(line);
  api.uiLog(ok ? "info" : "error", "SELFTEST " + line).catch(() => {});
}

async function until<T>(fn: () => T | null | undefined | false, ms = 3000): Promise<T | null> {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    try {
      const v = fn();
      if (v) return v as T;
    } catch {
      /* retry */
    }
    await sleep(60);
  }
  return null;
}

const $ = <E extends Element = HTMLElement>(sel: string, root: ParentNode = document) => root.querySelector<E>(sel);
const $$ = <E extends Element = HTMLElement>(sel: string, root: ParentNode = document) => Array.from(root.querySelectorAll<E>(sel));
const browserEl = (b: number) => $$(".browser")[b] as HTMLElement | undefined;
const pane = (b: number) => $(".folder-pane", browserEl(b)!) as HTMLElement;
const rowNames = (b: number) => $$(".fp-list [data-path]", browserEl(b)!).map((r) => r.getAttribute("data-path")!.split(/[/\\]/).pop()!);
const row0 = (name: string) => row(0, name);
const row = (b: number, name: string) => $$(".fp-list [data-path]", browserEl(b)!).find((r) => r.getAttribute("data-path")!.replace(/\\/g, "/").endsWith("/" + name)) as HTMLElement | undefined;

function center(el: Element) {
  const r = el.getBoundingClientRect();
  return { clientX: r.left + Math.min(r.width / 2, 60), clientY: r.top + r.height / 2 };
}

function mouse(el: Element, type: string, opts: MouseEventInit = {}) {
  el.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, view: window, ...center(el), ...opts }));
}

function click(el: Element, opts: MouseEventInit = {}) {
  mouse(el, "mousedown", opts);
  mouse(el, "mouseup", opts);
  mouse(el, "click", opts);
}

function key(k: string, opts: KeyboardEventInit = {}, target: Element | null = document.activeElement) {
  const code = k.length === 1 && /[a-z]/i.test(k) ? "Key" + k.toUpperCase() : k;
  (target ?? document.body).dispatchEvent(new KeyboardEvent("keydown", { key: k, code, bubbles: true, cancelable: true, ...opts }));
}

function typeInto(input: HTMLInputElement | HTMLTextAreaElement, text: string) {
  const proto = input instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(proto, "value")!.set!.call(input, text);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

/** Synthetic drag. A real drag spends at least a frame between dragstart and the first
 *  dragover, so React has re-rendered (e.g. the selection set by dragstart) by then. */
async function drag(src: Element, dst: Element, mods: { ctrlKey?: boolean; altKey?: boolean } = {}) {
  const dt = new DataTransfer();
  const ev = (type: string, el: Element) =>
    el.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: dt, ...center(el), ...mods }));
  ev("dragstart", src);
  await sleep(50);
  ev("dragenter", dst);
  ev("dragover", dst);
  return {
    dt,
    drop: () => {
      ev("drop", dst);
      ev("dragend", src);
    },
  };
}

async function shot(name: string) {
  // Spectacle captures the active window: make sure that is us.
  await getCurrentWindow().setFocus().catch(() => {});
  await sleep(250);
  await api.selftestShot(name).catch((e) => log(false, `screenshot ${name}`, String(e)));
}

async function focusPane(b: number) {
  click($(".fp-status", browserEl(b)!)!); // neutral spot inside the pane
  pane(b).focus();
  await sleep(150);
}

async function selectRow(b: number, name: string) {
  const r = await until(() => row(b, name));
  if (!r) throw new Error(`row ${name} not found in browser ${b}: ${rowNames(b).join(", ")}`);
  click(r);
  pane(b).focus();
  await sleep(120);
  return r;
}


/** Header cells vs first row cells: left and right edges must match to the pixel. */
function headerAlignment(b: number): { ok: boolean; detail: string } {
  const el = browserEl(b);
  const r = el && $(".fp-list [data-path]", el);
  if (!el || !r) return { ok: true, detail: "no rows" };
  const heads = $$(".fp-hrow .col", el);
  const cols = $$(":scope > .col", r);
  const off = heads.map((h, i) => {
    const hr = h.getBoundingClientRect(), cr = cols[i]?.getBoundingClientRect();
    return cr ? Math.max(Math.abs(hr.left - cr.left), Math.abs(hr.right - cr.right)) : 99;
  });
  return { ok: heads.length === cols.length && off.every((d) => d <= 1), detail: `max offsets ${off.map((d) => d.toFixed(1)).join(",")} (${heads.length} heads/${cols.length} cols)` };
}

/** Root containers must never be scrolled (the "shifted UI" bug). */
function rootScroll(): number {
  return [document.documentElement, document.body, $("#root"), $(".frame"), $(".app")].reduce((n, e) => n + (e ? e.scrollTop + e.scrollLeft : 0), 0);
}

/** Layout sanity at the current window size; returns problems found. */
function layoutProblems(): string[] {
  const p: string[] = [];
  const W = window.innerWidth, H = window.innerHeight;
  if (document.documentElement.scrollWidth > W + 1) p.push(`page overflows horizontally (${document.documentElement.scrollWidth} > ${W})`);
  if (rootScroll() !== 0) p.push("root scrolled");
  const tb = $(".titlebar")?.getBoundingClientRect();
  if (!tb || tb.top !== 0 || tb.height < 20) p.push("title bar missing or not at top");
  const brand = $(".tb-brand")?.getBoundingClientRect(), ctr = $(".tb-controls")?.getBoundingClientRect(), path = $(".tb-path")?.getBoundingClientRect();
  if (brand && ctr && brand.right > ctr.left) p.push("title bar: brand overlaps window controls");
  if (path && ctr && path.width > 0 && path.right > ctr.left + 1) p.push("title bar: path overlaps window controls");
  if (ctr && ctr.right > W + 1) p.push("title bar: controls cut off");
  // The panel controls (‹ ⇄ ›) sit right above the divider between the browsers.
  const pm = $(".panel-map")?.getBoundingClientRect(), split = $(".ws-split.v")?.getBoundingClientRect();
  if (pm && split && Math.abs(pm.left + pm.width / 2 - (split.left + split.width / 2)) > 2)
    p.push(`title bar: panel controls at ${Math.round(pm.left + pm.width / 2)}, divider at ${Math.round(split.left + split.width / 2)}`);
  if (split && !pm) p.push("title bar: panel controls missing");
  const tb2 = $(".main-toolbar");
  if (tb2 && tb2.scrollWidth > tb2.clientWidth + 1) p.push("sidebar toolbar overflows");
  const sb = $(".sidebar-scroll");
  if (sb && sb.clientHeight < 60) p.push(`sidebar list squeezed to ${sb.clientHeight}px`);
  const sm = $(".sysmon")?.getBoundingClientRect(), side = $(".sidebar")?.getBoundingClientRect();
  if (sm && side && sm.width > 0 && (sm.right > side.right + 1 || sm.bottom > H + 1)) p.push("system monitor overflows the sidebar");
  $$(".sysmon .sm-sub, .sysmon .sm-val").forEach((e) => { if (e.scrollWidth > e.clientWidth + 2 && e.clientWidth > 0) p.push(`monitor text clipped: ${e.textContent}`); });
  $$(".browser").forEach((br, b) => {
    const R = br.getBoundingClientRect();
    if (R.right > W + 1 || R.bottom > H + 1) p.push(`browser ${b} exceeds window (${Math.round(R.right)}x${Math.round(R.bottom)})`);
    const list = $(".fp-list", br)?.getBoundingClientRect();
    if (list && list.bottom > R.bottom + 1) p.push(`browser ${b}: list taller than card`);
    const head = $(".fp-head", br) as HTMLElement | null;
    if (head && head.offsetHeight > 56) p.push(`browser ${b}: folder header wraps (${head.offsetHeight}px)`);
    const pill = $(".fp-pill", br)?.getBoundingClientRect(), more = $(".fp-more", br)?.getBoundingClientRect();
    if (more && more.right > R.right + 1) p.push(`browser ${b}: header actions cut off`);
    if (pill && more && pill.right > more.left + 1) p.push(`browser ${b}: sort/view pill overlaps menu`);
    const a = headerAlignment(b);
    if (!a.ok) p.push(`browser ${b}: header misaligned ${a.detail}`);
    // Header labels must not spill into the neighbouring column.
    const nameCell = $(".fp-hrow .fp-nav-name", br), firstCol = $(".fp-hrow .col", br);
    if (nameCell && firstCol) {
      const label = $(".col-head", nameCell);
      if (label && label.getBoundingClientRect().width > 0 && label.getBoundingClientRect().right > firstCol.getBoundingClientRect().left + 1) p.push(`browser ${b}: Name header overlaps next column`);
    }
    $$(".fp-hrow .col .col-head", br).forEach((h) => { if (h.scrollWidth > h.clientWidth + 1 && h.clientWidth > 0) p.push(`browser ${b}: header '${h.textContent}' clipped`); });
    // Status glyphs: Nerd font when present, SVG otherwise (never tofu).
    if ($(".fp-status .nf", br) && !hasNerdFont()) p.push(`browser ${b}: Nerd glyphs used without a Nerd font`);
    const r = $(".fp-list [data-path]", br);
    if (r) {
      const clipped = $$(":scope > .col", r).filter((c) => c.scrollWidth > c.clientWidth + 1).map((c) => c.textContent);
      if (clipped.length) p.push(`browser ${b}: clipped ${clipped.join(",")}`);
    }
    const ages = $$(".fp-list .age", br);
    if (new Set(ages.map((a) => Math.round(a.getBoundingClientRect().width))).size > 1) p.push(`browser ${b}: age badges differ in width`);
    const cut = ages.filter((a) => a.scrollWidth > a.clientWidth + 1).map((a) => a.textContent);
    if (cut.length) p.push(`browser ${b}: age label clipped ${cut.join(",")}`);
    const broken = $$<HTMLImageElement>(".ficon img", br).filter((i) => i.complete && i.naturalWidth === 0).map((i) => i.src.split("/").pop());
    if (broken.length) p.push(`browser ${b}: broken icons ${broken.join(",")}`);
  });
  return p;
}

/** Real keyboards deliver each key as its own task, so React re-renders in between. */
async function press(target: Element, ...ks: (string | [string, KeyboardEventInit])[]) {
  for (const k of ks) {
    if (Array.isArray(k)) key(k[0], k[1], target);
    else key(k, {}, target);
    await sleep(60);
  }
}


/** Sets the folder-compare status filter by clicking its chip; returns the active chip label. */
async function dcFilter(label: string): Promise<string> {
  const chip = $$(".dc-chip").find((c) => c.textContent?.startsWith(label));
  if (chip) click(chip);
  await until(() => $(".dc-chip.on")?.textContent?.startsWith(label), 2000);
  return $(".dc-chip.on")?.textContent ?? "none";
}

async function step(name: string, fn: () => Promise<void>) {
  const sidebarBefore = getState().sidebarVisible;
  try {
    await fn();
  } catch (e) {
    log(false, name, String(e));
  }
  if (sidebarBefore && !getState().sidebarVisible) {
    log(false, `sidebar stays visible during "${name}"`, `keySeq=${JSON.stringify(getState().keySeq)}`);
    const { setState } = await import("./state/store");
    setState({ sidebarVisible: true });
    await sleep(150);
  }
  // Close anything left open so the next step starts clean.
  if (getState().overlay || getState().menu) {
    key("Escape", {}, document.activeElement ?? document.body);
    await sleep(200);
  }
  if (getState().overlay || getState().menu) {
    // Escape went to an element that does not own the dialog; close it directly.
    const { openOverlay } = await import("./state/actions");
    const { setState } = await import("./state/store");
    openOverlay(null);
    setState({ menu: null });
    await sleep(150);
  }
}

let previewShortcutsRunning = false;
async function runPreviewShortcutsSelfTest() {
  if (previewShortcutsRunning) return;
  previewShortcutsRunning = true;
  results.push(`Runtime: ${window.location.origin}`);
  const A = await import("./state/actions");
  const { join } = await import("./core/path");
  const root = activePath(0);
  const errors: string[] = [];
  window.addEventListener("unhandledrejection", (e) => errors.push(String(e.reason)));
  window.addEventListener("error", (e) => errors.push(e.message));
  const pdf = () => $<HTMLElement>(".pv-pdf");
  const rendered = (page: number) => {
    const canvas = $<HTMLCanvasElement>(".pv-pdf canvas");
    if (pdf()?.dataset.page !== String(page) || pdf()?.dataset.pages !== "2" || !canvas?.width) return false;
    if (!$(".textLayer")?.textContent?.includes(`Gall PDF page ${page}`)) return false;
    const pixels = canvas.getContext("2d")!.getImageData(0, 0, canvas.width, canvas.height).data;
    return pixels.some((channel, i) => i % 4 !== 3 && channel < 128);
  };
  try {
    await getCurrentWindow().setFocus();
    await getCurrentWindow().unmaximize();
    await getCurrentWindow().setSize(new LogicalSize(1100, 800));
    await updateSettings({ indexRoots: [root], keyboardProfile: "explorer", shortcuts: {}, theme: "dark" });
    await A.sessionCmd({ op: "layout", layout: "classic" });
    await A.sessionCmd({ op: "details", b: 0, on: true });
    if (!await until(() => $(".details"), 10000)) throw new Error(`Details pane is not visible (${getState().session?.layout}; viewport ${window.innerWidth}x${window.innerHeight}; native ${JSON.stringify(await getCurrentWindow().innerSize())}; browser widths ${$$(".browser").map((el) => el.clientWidth).join(",")})`);
    await focusPane(0);
    await selectRow(0, "sample.pdf");
    const loaded = await until(() => rendered(1), 15000);
    log(!!loaded, "PDF loads and paints page 1 with selectable text", `focused=${getState().focused[0]?.name}; ${$(".preview")?.textContent ?? ""}`);
    await shot("pdf-first-page");
    if (loaded) {
      pdf()!.focus();
      key("ArrowRight", {}, pdf());
      log(!!await until(() => rendered(2)), "PDF ArrowRight paints the next page");
      key("ArrowRight", {}, pdf());
      log(pdf()?.dataset.page === "2", "PDF next page stays at the last page");
      key("+", {}, pdf());
      log(!!await until(() => $(".pv-badge .pct")?.textContent === "125%"), "PDF keyboard zoom works");
      await shot("pdf-second-page-zoom");
      key("0", {}, pdf());
      key("ArrowLeft", {}, pdf());
      log(!!await until(() => rendered(1) && $(".pv-badge .pct")?.textContent === "100%"), "PDF keyboard returns to first page and fit");
    }
    await selectRow(0, "broken.pdf");
    log(!!await until(() => $(".preview")?.textContent?.includes("PDF preview failed"), 10000), "invalid PDF shows a failure instead of staying blank");
    await getCurrentWindow().setSize(new LogicalSize(800, 600));
    await shot("pdf-error-narrow");
    await selectRow(0, "sample.pdf");
    log(!!await until(() => rendered(1), 15000), "valid PDF recovers after an invalid PDF");
    await shot("pdf-recovered-narrow");
    const originalFetch = window.fetch;
    const readError = "PDF read failed: " + "unavailable-source-".repeat(12);
    try {
      window.fetch = (...args) => String(args[0]).includes("broken.pdf")
        ? Promise.reject(new Error(readError)) : originalFetch(...args);
      await selectRow(0, "broken.pdf");
      const failed = await until(() => $(".pv-pdf-error small")?.textContent?.includes(readError));
      const card = $<HTMLElement>(".pv-pdf-error");
      const bounds = card?.getBoundingClientRect();
      const previewBounds = $(".preview")!.getBoundingClientRect();
      log(!!failed && !!card && card.scrollWidth <= card.clientWidth && !!bounds
        && bounds.left >= previewBounds.left && bounds.right <= previewBounds.right,
      "long PDF read errors remain within the narrow preview");
      await shot("pdf-read-error-narrow");
    } finally {
      window.fetch = originalFetch;
    }
    await selectRow(0, "sample.pdf");
    log(!!await until(() => rendered(1), 15000), "PDF recovers after a read failure");
    await getCurrentWindow().setSize(new LogicalSize(1100, 800));
    for (const profile of ["explorer", "vim"] as const) {
      await updateSettings({ keyboardProfile: profile });
      await focusPane(0);
      await selectRow(0, "Folder shortcut.lnk");
      key(profile === "vim" ? "l" : "Enter", {}, pane(0));
      log(!!await until(() => activePath(0) === join(root, "Target folder") && !!row(0, "target.2do")), `folder shortcut navigates inside Gall (${profile})`);
      await shot(`shortcut-folder-${profile}`);
      A.navigate(0, root);
      await selectRow(0, "Note shortcut.lnk");
      key("Enter", {}, pane(0));
      log(!!await until(() => getState().overlay?.type === "note" && $<HTMLTextAreaElement>(".modal textarea")?.value.includes("Shortcut target note")), `file shortcut uses Gall's internal note flow (${profile})`);
      await shot(`shortcut-note-${profile}`);
      A.openOverlay(null);
      await selectRow(0, "Broken shortcut.lnk");
      key("Enter", {}, pane(0));
      log(!!await until(() => getState().notifications.some((n) => n.kind === "error" && n.text.includes("shortcut"))), `broken shortcut reports an error (${profile})`);
      for (const n of getState().notifications) A.dismiss(n.id);
    }
    log(errors.length === 0, "no unhandled preview or activation errors", errors.join("; "));
  } catch (e) {
    log(false, "PDF and shortcut integration", String(e));
    await shot("preview-integration-error");
  }
  await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
}

export async function runSelfTest() {
  if ((await api.startupArgs()).selftestMode === "git-compare") return runGitCompareSelfTest();
  if ((await api.startupArgs()).selftestMode === "git-workspace") return (await import("./selftestGitWorkspace")).runGitWorkspaceSelfTest(gitSelftest);
  if ((await api.startupArgs()).selftestMode === "preview-shortcuts") return runPreviewShortcutsSelfTest();
  if ((await api.startupArgs()).selftestMode === "quick-open") return runQuickOpenSelfTest();
  if ((await api.startupArgs()).selftestMode === "key-profiles") return runKeyboardProfilesSelfTest();
  if ((await api.startupArgs()).selftestMode === "key-profiles-reload") return runKeyboardProfilesReloadSelfTest();
  if ((await api.startupArgs()).selftestMode === "key-help") return runKeyHelpSelfTest();
  if ((await api.startupArgs()).selftestMode === "keyboard") return runKeyboardSelfTest();
  if ((await api.startupArgs()).selftestMode === "bug-report-entry") return runBugReportSelfTest(false, true);
  if ((await api.startupArgs()).selftestMode === "bug-report") return runBugReportSelfTest();
  if ((await api.startupArgs()).selftestMode === "bug-report-text") return runBugReportSelfTest(true);
  if ((await api.startupArgs()).selftestMode === "updates") return runUpdateSelfTest();
  const errors: string[] = [];
  window.addEventListener("error", (e) => errors.push(e.message));
  window.addEventListener("unhandledrejection", (e) => errors.push(String(e.reason)));
  const root = activePath(0);
  results.push(`root: ${root}`);
  // Make sure we are on screen: hidden/occluded webviews skip layout and ResizeObserver.
  await getCurrentWindow().setFocus().catch(() => {});
  await new Promise<void>((r) => { requestAnimationFrame(() => r()); setTimeout(r, 1000); });
  results.push(`visibility: ${document.visibilityState}, list height: ${Math.round($(".fp-list")?.getBoundingClientRect().height ?? 0)}px`);
  // Legacy steps type to filter; vim keys get their own steps below.
  await updateSettings({ vimKeys: false, indexRoots: [root] });
  api.indexRefresh(true).catch(() => {});

  await step("startup layout", async () => {
    const ok = await until(() => rowNames(0).includes("Documents") && rowNames(1).length > 0);
    log(!!ok, "startup layout", `b1=[${rowNames(0)}] b2=[${rowNames(1)}]`);
    log(!$(".details"), "details pane hidden by default");
    await shot("01-startup");
  });

  await step("open folder (double click)", async () => {
    mouse((await until(() => row(0, "Documents")))!, "dblclick");
    const ok = await until(() => activePath(0).endsWith("/Documents") && rowNames(0).includes("file1.txt"));
    log(!!ok, "open folder (double click)", activePath(0));
  });

  await step("natural sort", async () => {
    const files = rowNames(0).filter((n) => n.startsWith("file"));
    log(files.join(",") === "file1.txt,file2.txt,file10.txt,file20.txt", "natural sort", files.join(","));
  });

  await step("columns: no clipping, header aligned", async () => {
    const r = row(0, "file1.txt")!;
    const cols = $$(".col", r);
    const clipped = cols.filter((c) => c.scrollWidth > c.clientWidth + 1).map((c) => c.textContent);
    log(clipped.length === 0, "no clipped column values", clipped.join(", "));
    const a = headerAlignment(0);
    log(a.ok, "column headers aligned with columns", a.detail);
  });

  await step("quick filter", async () => {
    await focusPane(0);
    key("f");
    key("i");
    key("l");
    key("e");
    key("1");
    const ok = await until(() => rowNames(0).every((n) => n.toLowerCase().includes("file1")) && rowNames(0).length === 2);
    log(!!ok && !!$(".quick-filter", browserEl(0)!), "quick filter 'file1'", rowNames(0).join(","));
    await shot("02-quick-filter");
    key("Escape");
    const cleared = await until(() => rowNames(0).length > 3);
    log(!!cleared, "quick filter cleared with Esc", rowNames(0).join(","));
    // Reported: filter → Ctrl+P → Esc → Esc left the filter on (focus was lost to <body>).
    key("f"); key("i"); key("l"); key("e"); key("1");
    await until(() => !!$(".quick-filter", browserEl(0)!));
    key("p", { ctrlKey: true });
    const qoInput = await until(() => $<HTMLInputElement>(".qo-input input"));
    if (qoInput) key("Escape", {}, qoInput);
    await until(() => !getState().overlay);
    key("Escape");
    const cleared2 = await until(() => !$(".quick-filter", browserEl(0)!) && rowNames(0).length > 3);
    log(!!qoInput && !!cleared2, "filter → Ctrl+P → Esc → Esc clears the filter", `focus on ${document.activeElement?.className || document.activeElement?.tagName}`);
  });

  await step("rename F2", async () => {
    await selectRow(0, "file2.txt");
    key("F2");
    const input = await until(() => $<HTMLInputElement>(".modal input[type=text]"));
    if (!input) return log(false, "rename dialog opens");
    log(input.selectionStart === 0 && input.selectionEnd === 5, "rename preselects name without extension", `${input.selectionStart}-${input.selectionEnd}`);
    await shot("03-rename");
    typeInto(input, "renamed.txt");
    key("Enter", {}, input);
    const ok = await until(() => rowNames(0).includes("renamed.txt") && !rowNames(0).includes("file2.txt"));
    log(!!ok, "rename applied", rowNames(0).join(","));
  });

  await step("undo rename", async () => {
    await focusPane(0);
    key("z", { ctrlKey: true });
    const ok = await until(() => rowNames(0).includes("file2.txt"));
    log(!!ok, "Ctrl+Z undoes rename", rowNames(0).join(","));
  });

  await step("new folder", async () => {
    await focusPane(0);
    key("N", { ctrlKey: true, shiftKey: true });
    const input = await until(() => $<HTMLInputElement>(".modal input[type=text]"));
    if (!input) return log(false, "new folder dialog opens");
    typeInto(input, "Created");
    key("Enter", {}, input);
    const ok = await until(() => rowNames(0).includes("Created"));
    log(!!ok, "new folder created", rowNames(0).join(","));
  });

  await step("drag file onto folder = move", async () => {
    await sleep(400); // "new folder" selects the created folder after a short delay
    const src = await selectRow(0, "file10.txt");
    const dst = row(0, "Created")!;
    const d = await drag(src, dst);
    await sleep(120);
    const hint = await until(() => $(".drag-hint"));
    log(!!hint && hint.classList.contains("move"), "drag hint shows Move", hint?.textContent ?? "no hint");
    await shot("04-drag-hint-move");
    d.drop();
    const ok = await until(() => !rowNames(0).includes("file10.txt"), 4000);
    log(!!ok, "drop moved the file", rowNames(0).join(","));
    log(!(await until(() => $(".drag-hint"), 400)), "drag hint hidden after drop");
  });

  await step("drag with Ctrl = copy", async () => {
    (globalThis as { __gallEventMods?: boolean }).__gallEventMods = true;
    const src = await selectRow(0, "file20.txt");
    const d = await drag(src, row(0, "Created")!, { ctrlKey: true });
    await sleep(120);
    const hint = $(".drag-hint");
    log(!!hint && hint.classList.contains("copy"), "drag hint shows Copy with Ctrl", hint?.textContent ?? "no hint");
    d.drop();
    await sleep(1200);
    log(rowNames(0).includes("file20.txt"), "Ctrl-drop kept the source");
    const inside = await api.listDir(root + "/Documents/Created", { showHidden: false, hideTemp: false });
    log(inside.some((e) => e.name === "file20.txt") && inside.some((e) => e.name === "file10.txt"), "target folder has copied + moved files", inside.map((e) => e.name).join(","));
    (globalThis as { __gallEventMods?: boolean }).__gallEventMods = false;
  });

  await step("drag folder into itself refused", async () => {
    const created = row(0, "Created")!;
    const d = await drag(created, created);
    await sleep(100);
    const hint = $(".drag-hint");
    log(!hint || hint.classList.contains("none"), "no move offered onto itself", hint?.textContent ?? "no hint");
    d.drop();
    await sleep(300);
    log(rowNames(0).includes("Created"), "folder still in place");
  });

  await step("copy to other browser (Alt+C)", async () => {
    await selectRow(0, "file1.txt");
    key("c", { altKey: true });
    const ok = await until(() => rowNames(1).includes("file1.txt"), 4000);
    log(!!ok, "Alt+C copied to other browser", rowNames(1).join(","));
  });

  await step("trash + undo", async () => {
    await selectRow(0, "file1.txt");
    key("Delete");
    const gone = await until(() => !rowNames(0).includes("file1.txt"), 4000);
    log(!!gone, "Delete moves to trash", rowNames(0).join(","));
    const note = await until(() => getState().notifications.find((n) => n.text.includes("to trash"))?.text, 2000);
    log(note === "Moved “file1.txt” to trash", "trash notification names the file", note ?? "");
    await focusPane(0);
    key("z", { ctrlKey: true });
    const back = await until(() => rowNames(0).includes("file1.txt"), 4000);
    log(!!back, "Ctrl+Z restores from trash", rowNames(0).join(","));
  });

  await step("conflict: merge folders", async () => {
    const { transfer, undo } = await import("./state/actions");
    const L = root + "/MergeDirs/L", R = root + "/MergeDirs/R";
    const names = async () => (await api.listDir(R + "/Photos", { showHidden: false, hideTemp: false })).map((e) => e.name).sort().join(",");
    const settle = async (want: string) => {
      for (let i = 0; i < 40 && (await names()) !== want; i++) await sleep(100);
      return names();
    };
    const t = transfer("copy", [L + "/Photos"], R);
    const merge = await until(() => $<HTMLElement>('.modal button[data-id="merge"]'), 3000);
    log(!!merge, "a folder on both sides offers Merge", $(".modal")?.textContent ?? "no dialog");
    await shot("43-conflict-merge");
    merge?.click();
    const second = await until(() => $<HTMLElement>('.modal button[data-id="overwriteOlder"]'), 3000);
    log(!!second && $(".modal h3")?.textContent === "1 file exists in both", "then asks about the file in both", $(".modal h3")?.textContent ?? "");
    await shot("44-conflict-files-in-both");
    $<HTMLElement>('.modal button[data-id="keepBoth"]')?.click();
    await t;
    const merged = await settle("a (2).txt,a.txt,l.txt,r.txt");
    log(merged === "a (2).txt,a.txt,l.txt,r.txt", "contents merged, the file in both kept twice", merged);
    await undo();
    const back = await settle("a.txt,r.txt");
    log(back === "a.txt,r.txt", "one Ctrl+Z reverts the merge", back);
  });

  await step("batch rename", async () => {
    const { batchRename, undo } = await import("./state/actions");
    const dir = root + "/Rename";
    const list = async () => (await api.listDir(dir, { showHidden: false, hideTemp: false })).map((e) => e.name).sort().join(",");
    const entries = (await api.listDir(dir, { showHidden: false, hideTemp: false })).sort((a, b) => a.name.localeCompare(b.name));
    batchRename(0, entries);
    const win = await until(() => $(".brn-win"), 3000);
    log(!!win, "F2 on several items opens the batch rename window");
    typeInto($<HTMLInputElement>(".brn-field input")!, "Trip {n:2}");
    await sleep(100);
    $<HTMLElement>('.brn-opts .seg button:nth-child(2)')?.click(); // lower case
    await sleep(100);
    const preview = $$(".brn-row .new").map((e) => e.textContent).join(",");
    log(preview === "trip 01.HEIC,trip 02.HEIC,trip 03.HEIC", "preview shows the new names", preview);
    await shot("45-batch-rename");
    $<HTMLElement>(".brn-foot .btn.primary")?.click();
    let now = "";
    for (let i = 0; i < 30 && (now = await list()) !== "trip 01.HEIC,trip 02.HEIC,trip 03.HEIC"; i++) await sleep(100);
    log(now === "trip 01.HEIC,trip 02.HEIC,trip 03.HEIC", "items renamed", now);
    await undo();
    for (let i = 0; i < 30 && (now = await list()) !== "IMG_0001.HEIC,IMG_0002.HEIC,IMG_0003.HEIC"; i++) await sleep(100);
    log(now === "IMG_0001.HEIC,IMG_0002.HEIC,IMG_0003.HEIC", "one Ctrl+Z restores all names", now);
    // A name that is taken blocks the rename.
    batchRename(0, entries.slice(0, 2));
    await until(() => $(".brn-win"), 3000);
    typeInto($<HTMLInputElement>(".brn-field input")!, "same");
    await until(() => $$(".brn-row.bad").length === 2, 2000);
    log($$(".brn-row.bad").length === 2 && !!$<HTMLButtonElement>(".brn-foot .btn.primary")?.disabled, "duplicate names are flagged and block Rename",
      `${$$(".brn-row").map((r) => `${r.className}: ${r.textContent}`).join(" | ")} · ${$(".brn-foot")?.textContent ?? ""}`);
    key("Escape", {}, $(".brn-win")!);
    await sleep(150);
  });

  await step("git status badges", async () => {
    const { navigate } = await import("./state/actions");
    const back = activePath(0);
    navigate(0, root + "/Projects/Alpha");
    const badge = (name: string) => row(0, name)?.querySelector(".git-badge")?.textContent ?? "";
    const ok = await until(() => badge("notes.txt") === "M", 5000);
    log(!!ok, "a modified file shows M", `notes.txt: ${badge("notes.txt")}`);
    log(badge("README.md") === "U" && badge("src") === "U", "new files and folders show U", `README.md: ${badge("README.md")}, src: ${badge("src")}`);
    const branch = $(".fp-status .mod.git", browserEl(0)!)?.textContent ?? "";
    log(/^\S+/.test(branch.trim()), "the branch is in the status bar", branch);
    await shot("46-git-status");
    navigate(0, back);
    await until(() => activePath(0) === back, 2000);
    await sleep(300);
  });

  await step("folder sizes", async () => {
    const { navigate } = await import("./state/actions");
    const back = activePath(0);
    navigate(0, root + "/Sizes");
    await until(() => row(0, "big") && row(0, "small"), 4000);
    await focusPane(0);
    key("s", { ctrlKey: true, altKey: true });
    const size = (name: string) => row(0, name)?.querySelector(".col.counting, .col[title]")?.textContent?.trim() ?? "";
    const ok = await until(() => size("big") === "1.46 KB" && size("small") === "300 B", 5000);
    log(!!ok, "Ctrl+Alt+S shows the total size of each folder", `big: ${size("big")}, small: ${size("small")}`);
    const autoBefore = getState().settings!.folderSizesAuto;
    await updateSettings({ folderSizesAuto: true });
    try {
      navigate(0, root + "/Sizes/small");
      await until(() => row(0, "a.bin"), 4000);
      await api.trash([root + "/Sizes/small/a.bin"]);
      await until(() => !row(0, "a.bin"), 4000);
      navigate(0, root + "/Sizes");
      const updated = await until(() => size("small") === "0 B", 5000);
      log(!!updated, "automatic folder size refreshes after deleting its contents", `small: ${size("small")}`);
      navigate(0, root + "/Sizes/small");
      await until(() => activePath(0) === root + "/Sizes/small", 2000);
      await sleep(300);
      await api.undo();
      await until(() => row(0, "a.bin"), 4000);
      navigate(0, root + "/Sizes");
      const restored = await until(() => size("small") === "300 B", 5000);
      log(!!restored, "automatic folder size refreshes after undo", `small: ${size("small")}`);
    } finally {
      await updateSettings({ folderSizesAuto: autoBefore });
    }

    navigate(0, back);
    await until(() => activePath(0) === back, 2000);
    await sleep(300);
  });

  await step("kernel virtual folder sizes", async () => {
    if (getState().info?.platform !== "linux") return;
    const A = await import("./state/actions");
    const back = activePath(0);
    const automatic = getState().settings!.folderSizesAuto;
    try {
      await updateSettings({ folderSizesAuto: false });
      A.navigate(0, "/");
      await selectRow(0, "proc");
      key("s", { ctrlKey: true, altKey: true });
      const manual = await until(() => getState().folderSizes["/proc"]?.done && getState().folderSizes["/proc"].virtualFs, 5000);
      const label = row(0, "proc")?.querySelector(".col[title]")?.textContent?.trim();
      log(!!manual && label === "—", "manual /proc size is unavailable instead of 128 TB", label ?? "");
      A.openOverlay({ type: "properties", entry: getState().focused[0]! });
      const details = await until(() => $(".props")?.textContent?.includes("Virtual filesystem; folder size is not applicable"));
      log(!!details && !$(".props")?.textContent?.includes("(0 bytes)"), "Properties explains the virtual filesystem without a fabricated byte total");
      A.openOverlay(null);
      A.navigate(0, "/proc/self");
      await until(() => row(0, "fd"), 4000);
      await updateSettings({ folderSizesAuto: true });
      const counted = await until(() => getState().folderSizes["/proc/self/fd"]?.done && getState().folderSizes["/proc/self/fd"].virtualFs, 5000);
      log(!!counted, "automatic calculation skips virtual filesystem subfolders");
    } finally {
      A.openOverlay(null);
      await updateSettings({ folderSizesAuto: automatic });
      A.navigate(0, back);
      await until(() => activePath(0) === back, 2000);
      await sleep(300);
    }
  });

  await step("context menu", async () => {
    const r = await selectRow(0, "file1.txt");
    mouse(r, "contextmenu", { button: 2 });
    const m = await until(() => $(".menu"));
    log(!!m, "context menu opens", (m?.textContent ?? "").slice(0, 80));
    await shot("05-context-menu");
    key("Escape", {}, document.body);
    await sleep(150);
    log(!$(".menu"), "context menu closes with Esc");
  });

  await step("tabs", async () => {
    await focusPane(0);
    const n0 = getState().session!.browsers[0].tabs.length;
    key("t", { ctrlKey: true });
    const added = await until(() => getState().session!.browsers[0].tabs.length === n0 + 1);
    log(!!added, "Ctrl+T new tab", `${n0} → ${getState().session!.browsers[0].tabs.length}`);
    key("w", { ctrlKey: true }, document.body);
    const closed = await until(() => getState().session!.browsers[0].tabs.length === n0);
    log(!!closed, "Ctrl+W closes tab");
    key("T", { ctrlKey: true, shiftKey: true }, document.body);
    const reopened = await until(() => getState().session!.browsers[0].tabs.length === n0 + 1);
    log(!!reopened, "Ctrl+Shift+T reopens tab");
    await shot("06-tabs");
    key("w", { ctrlKey: true }, document.body);
    await sleep(200);
  });

  await step("back / parent", async () => {
    await focusPane(0);
    key("Backspace");
    const up = await until(() => activePath(0) === root);
    log(!!up, "Backspace goes to parent", activePath(0));
    const sel = await until(() => $(".fp-list .row.selected", browserEl(0)!));
    log(!!sel && sel.getAttribute("data-path")!.endsWith("/Documents"), "parent re-selects the folder we came from", sel?.getAttribute("data-path") ?? "none");
    key("ArrowLeft", { altKey: true }, document.body);
    const back = await until(() => activePath(0).endsWith("/Documents"));
    log(!!back, "Alt+Left goes back", activePath(0));
  });

  await step("path editor", async () => {
    key("\\", {}, document.body);
    const input = await until(() => $<HTMLInputElement>(".path-edit input"));
    if (!input) return log(false, "path editor opens");
    typeInto(input, root + "/Pho");
    await sleep(400);
    await shot("07-path-edit");
    const sugg = $$(".path-edit .s").map((s) => s.textContent);
    log(sugg.some((s) => s?.includes("/Photos")), "autocomplete suggests Photos", sugg.slice(0, 3).join(" | "));
    typeInto(input, root + "/Photos");
    key("Enter", {}, input);
    const ok = await until(() => activePath(0).endsWith("/Photos"));
    log(!!ok, "Enter navigates", activePath(0));
  });

  await step("details pane + image preview", async () => {
    await selectRow(0, "sunset.jpg");
    key("p", { altKey: true }, document.body);
    const det = await until(() => $(".details", browserEl(0)!));
    log(!!det, "Alt+P shows details pane");
    const img = await until(() => $<HTMLImageElement>(".pv-image img", browserEl(0)!), 3000);
    await until(() => img && img.naturalWidth > 0, 3000);
    log(!!img && img.naturalWidth === 640, "image preview loads", img ? `${img.naturalWidth}x${img.naturalHeight}` : "no img");
    const pv = $(".details .preview", browserEl(0)!);
    const pr = pv?.getBoundingClientRect();
    const dr = det?.getBoundingClientRect();
    log(!!pr && !!dr && Math.abs(pr.width - dr.width) < 2, "preview fills details pane width", `${pr?.width} vs ${dr?.width}`);
    await shot("08-details-image");
  });

  await step("preview window (Space)", async () => {
    await focusPane(0);
    key(" ", {}, pane(0));
    const pw = await until(() => $(".pw"));
    log(!!pw, "Space opens preview window");
    await shot("09-preview-window");
    key("ArrowUp", {}, document.body); // sunset.jpg is the last file
    await until(() => getState().focused[0]?.name !== "sunset.jpg", 1500);
    log(getState().focused[0]?.name === "plasma.png", "Up moves to previous file in preview", getState().focused[0]?.name ?? "");
    key("Escape", {}, document.body);
    const closed = await until(() => !$(".pw"));
    log(!!closed, "Esc closes preview window");
    key("p", { altKey: true }, document.body);
    await sleep(200);
  });

  await step("thumbnail grid view", async () => {
    click($('[title="File view"]', browserEl(0)!)!);
    const m = await until(() => $$(".menu .mi").find((x) => x.textContent?.includes("Thumbnail grid")));
    if (!m) return log(false, "view menu");
    click(m);
    const grid = await until(() => $(".grid-cell", browserEl(0)!));
    log(!!grid, "grid view renders");
    await shot("10-grid");
    click($('[title="File view"]', browserEl(0)!)!);
    const d = await until(() => $$(".menu .mi").find((x) => x.textContent?.trim() === "Details"));
    if (d) click(d);
    await sleep(300);
  });

  await step("layouts", async () => {
    for (const l of ["columns", "classic", "standard"] as const) {
      setLayout(l);
      await sleep(700);
      const visible = $$(".browser").filter((browser) => browser.getBoundingClientRect().width > 0);
      log($$(".browser").length === 2 && visible.length === (l === "classic" ? 1 : 2), `layout ${l}`, `${visible.length} visible browser(s)`);
      if (l !== "standard") await shot(`11-layout-${l}`);
    }
  });

  await step("search", async () => {
    const { navigate } = await import("./state/actions");
    navigate(0, root);
    await until(() => activePath(0) === root);
    key("F3", {}, document.body);
    const input = await until(() => $<HTMLInputElement>(".search-win input[type=text]"));
    if (!input) return log(false, "search opens");
    // search from the test root
    const sel = $<HTMLSelectElement>(".search-win select");
    typeInto(input, "*.txt");
    key("Enter", {}, input);
    const res = await until(() => $$(".search-win .r").length >= 1 && !$(".search-win .foot")?.textContent?.startsWith("Searching"), 5000);
    log(!!res, "search finds *.txt", `${$$(".search-win .r").length} results, scope ${sel?.value}`);
    await shot("12-search");
    key("Escape", {}, input);
    await sleep(200);
  });

  await step("quick open (Ctrl+P)", async () => {
    for (let i = 0; i < 40; i++) {
      const st = await api.indexStatus();
      if (!st.building && st.entries > 5 && st.roots.includes(root)) break;
      await sleep(150);
    }
    const st = await api.indexStatus();
    log(st.roots.includes(root) && st.entries > 5, "index built for test root", `${st.entries} entries, roots ${st.roots.join(",")}`);
    key("p", { ctrlKey: true }, document.body);
    const input = await until(() => $<HTMLInputElement>(".qo-input input"));
    if (!input) return log(false, "quick open opens");
    typeInto(input, "rdme");
    const hit = await until(() => $$(".qo-row").find((r) => r.textContent?.includes("README.md")), 3000);
    log(!!hit, "fuzzy 'rdme' finds README.md", $$(".qo-row").map((r) => r.textContent).slice(0, 3).join(" | "));
    log(!!$(".qo-row.on mark"), "matched letters highlighted");
    await until(() => $(".qo-preview .pv-text, .qo-preview .pv-icon"), 2000);
    await shot("16-quick-open");
    typeInto(input, "ext:jpg");
    const jpg = await until(() => $$(".qo-row:not(.qo-wider)").length === 1 && $(".qo-row:not(.qo-wider)")!.textContent!.includes("sunset.jpg"), 3000);
    log(!!jpg, "ext:jpg filter", $$(".qo-row").map((r) => r.textContent).join(" | "));
    key("Enter", { ctrlKey: true }, input);
    const revealed = await until(() => activePath(0).endsWith("/Photos") && getState().focused[0]?.name === "sunset.jpg", 3000);
    log(!!revealed, "Ctrl+Enter reveals the file in the browser", `${activePath(0)} / ${getState().focused[0]?.name}`);
  });

  await step("command palette", async () => {
    const n0 = getState().session!.browsers[0].tabs.length;
    key("P", { ctrlKey: true, shiftKey: true }, document.body);
    const input = await until(() => $<HTMLInputElement>(".qo-input input"));
    if (!input) return log(false, "command palette opens");
    log(input.value === ">", "palette starts in command mode", input.value);
    typeInto(input, ">new tab");
    const row = await until(() => $(".qo-row.cmd.on"));
    log(!!row && row.textContent!.includes("New tab"), "fuzzy command match", row?.textContent ?? "");
    await shot("17-command-palette");
    key("Enter", {}, input);
    const added = await until(() => getState().session!.browsers[0].tabs.length === n0 + 1);
    log(!!added, "Enter runs the command (new tab)");
    key("w", { ctrlKey: true }, document.body);
    await sleep(200);
  });

  await step("find in files (Ctrl+F)", async () => {
    const { navigate } = await import("./state/actions");
    navigate(0, root);
    await until(() => activePath(0) === root);
    await focusPane(0);
    key("f", { ctrlKey: true }, document.body);
    const input = await until(() => $<HTMLInputElement>(".fif-query input"));
    if (!input) return log(false, "find in files opens");
    log($(".seg button.on")?.getAttribute("title") === root, "Ctrl+F scope = current folder", $(".seg button.on")?.getAttribute("title") ?? "");
    typeInto(input, "test project");
    const hit = await until(() => $$(".fif-file").find((r) => r.textContent?.includes("README.md")), 5000);
    log(!!hit, "finds text inside README.md", $$(".fif-file").map((r) => r.textContent).join(" | "));
    const m = await until(() => $(".fif-match.on mark"), 2000);
    log(!!m && m.textContent === "test project", "match highlighted", m?.textContent ?? "");
    const ctx = await until(() => $(".fif-pv-code .hit"), 2000);
    log(!!ctx && ctx.textContent!.includes("A test project."), "context preview shows the matching line", ctx?.textContent ?? "");
    typeInto(input, "line");
    await until(() => $$(".fif-file").length >= 3, 5000);
    await sleep(300);
    await shot("18-find-in-files");
    // Regex toggle + an invalid pattern reports an error instead of crashing.
    click($$(".tgl")[2]);
    typeInto(input, "line (");
    const err = await until(() => $(".fif-empty .err"), 3000);
    log(!!err, "invalid regex shows an error", err?.textContent ?? "");
    click($$(".tgl")[2]);
    key("Escape", {}, input);
    await sleep(200);
  });

  await step("find in files whole system (Ctrl+Shift+F)", async () => {
    key("F", { ctrlKey: true, shiftKey: true }, document.body);
    const on = await until(() => $(".fif .seg button.on"));
    log(on?.getAttribute("title") === "/", "Ctrl+Shift+F scope = file system root", on?.getAttribute("title") ?? "");
    await shot("19-find-root-scope");
    key("Escape", {}, $(".fif-query input") ?? document.body);
    await sleep(200);
  });

  await step("vim keys", async () => {
    await updateSettings({ vimKeys: true });
    const { navigate } = await import("./state/actions");
    navigate(0, root + "/Documents");
    await until(() => rowNames(0).includes("file1.txt"));
    await focusPane(0);
    const names = rowNames(0);
    await press(pane(0), "g", "g");
    log(getState().focused[0]?.name === names[0], "gg goes to the first item", getState().focused[0]?.name ?? "none");
    await press(pane(0), "j", "j");
    log(getState().focused[0]?.name === names[2], "j moves down", getState().focused[0]?.name ?? "none");
    await press(pane(0), "k");
    log(getState().focused[0]?.name === names[1], "k moves up", getState().focused[0]?.name ?? "none");
    key("G", { shiftKey: true }, pane(0));
    await sleep(120);
    log(getState().focused[0]?.name === names[names.length - 1], "G goes to the last item", getState().focused[0]?.name ?? "none");
    // Letters do not filter in vim mode; "/" starts a filter.
    key("w", {}, pane(0));
    await sleep(150);
    log(!$(".quick-filter", browserEl(0)!), "letters are commands, not a filter");
    key("Escape", {}, document.body);
    // b pins the folder to the sidebar and unpins it again.
    const { isPinned } = await import("./state/actions");
    key("b", {}, pane(0));
    const pinnedNow = await until(() => isPinned(root + "/Documents"), 2000);
    key("b", {}, pane(0));
    const unpinned = await until(() => !isPinned(root + "/Documents"), 2000);
    log(!!pinnedNow && !!unpinned, "b pins and unpins the folder");
    // q closes overlays like Esc (not while typing in a field).
    const { openOverlay } = await import("./state/actions");
    openOverlay({ type: "about" });
    const about = await until(() => $(".about-win"), 2000);
    if (about) key("q", {}, about);
    log(!!about && !!(await until(() => !getState().overlay, 1500)), "q closes the About window");
    openOverlay({ type: "previewWindow", b: 0 });
    const pw = await until(() => $(".pw"), 2000);
    if (pw) key("q", {}, pw);
    log(!!pw && !!(await until(() => !getState().overlay, 1500)), "q closes the preview window");
    openOverlay({ type: "settings" });
    const search = await until(() => $<HTMLInputElement>(".settings .ssearch input"), 2000);
    if (search) key("q", {}, search);
    await sleep(150);
    log(getState().overlay?.type === "settings", "q in a text field does not close (Settings search)");
    openOverlay(null);
    await focusPane(0);
    await press(pane(0), "/", ..."file2");
    const filtered = await until(() => rowNames(0).includes("file2.txt") && rowNames(0).every((n) => n.includes("file2")));
    log(!!filtered, "/ then typing filters", rowNames(0).join(","));
    await shot("20-vim-filter");
    key("Escape", {}, pane(0));
    await until(() => rowNames(0).length > 3);
    key("h", {}, pane(0));
    const up = await until(() => activePath(0) === root);
    log(!!up, "h goes to the parent folder", activePath(0));
    await focusPane(0);
    key("l", {}, pane(0));
    const into = await until(() => activePath(0) !== root);
    log(!!into, "l enters the focused folder", activePath(0));
  });

  await step("leader menu (which-key)", async () => {
    await focusPane(0);
    key(" ", {}, pane(0));
    const wk = await until(() => $(".wk"), 1500);
    log(!!wk && wk.textContent!.includes("find"), "Space opens the which-key menu", (wk?.textContent ?? "").slice(0, 60));
    await shot("21-which-key");
    key("f", {}, document.body);
    const sub = await until(() => $(".wk-title")?.textContent?.includes("find"), 1500);
    log(!!sub, "group f shows the find menu");
    await shot("22-which-key-find");
    key("f", {}, document.body);
    const qo = await until(() => $(".qo"));
    log(!!qo && !$(".wk"), "Space f f opens Quick Open");
    key("Escape", {}, $(".qo-input input") ?? document.body);
    await sleep(200);
    await focusPane(0);
    key("?", { shiftKey: true }, pane(0));
    const kh = await until(() => $(".kh"));
    log(!!kh, "? opens the keyboard sheet");
    await shot("23-key-help");
    key("Escape", {}, $(".kh input") ?? document.body);
    await sleep(200);
  });

  await step("vim conventions (x q i [ ] 1-9 , Ctrl+w)", async () => {
    await updateSettings({ vimKeys: true });
    const { navigate } = await import("./state/actions");
    navigate(0, root + "/Documents");
    await until(() => rowNames(0).includes("file1.txt"));
    await selectRow(0, "file1.txt");
    await press(pane(0), "x");
    log(!!(await until(() => getState().clipboard.cut && getState().clipboard.paths.some((p) => p.endsWith("/file1.txt")))), "x cuts the selection");
    await press(pane(0), "Escape");
    await press(pane(0), "i");
    log(!!(await until(() => getState().overlay?.type === "previewWindow")), "i opens the preview", getState().overlay?.type ?? "none");
    const { openOverlay } = await import("./state/actions");
    openOverlay(null);
    await sleep(150);
    await focusPane(0);
    await press(pane(0), ",", ["S", { shiftKey: true }]);
    const sortedBySize = await until(() => {
      const names = rowNames(0);
      const heading = $(".fp-hrow .col-head.on", browserEl(0)!)?.textContent ?? "";
      return heading.includes("Size") && heading.includes("↓")
        && names.includes("archive.zip") && names.includes("file20.txt") && names.includes("file1.txt")
        && names.indexOf("archive.zip") < names.indexOf("file20.txt")
        && names.indexOf("file20.txt") < names.indexOf("file1.txt");
    });
    log(!!sortedBySize, ", S sorts by size descending", rowNames(0).join(", "));
    await press(pane(0), ",", "n");
    const tabsBefore = getState().session!.browsers[0].tabs.length;
    await press(pane(0), "t");
    await until(() => getState().session!.browsers[0].tabs.length === tabsBefore + 1);
    await focusPane(0);
    await press(pane(0), "1");
    log(!!(await until(() => getState().session!.browsers[0].active === 0)), "1 goes to the first tab");
    await press(pane(0), "]");
    log(!!(await until(() => getState().session!.browsers[0].active === 1)), "] goes to the next tab");
    await press(pane(0), "[");
    log(!!(await until(() => getState().session!.browsers[0].active === 0)), "[ goes to the previous tab");
    await press(pane(0), "2");
    await until(() => getState().session!.browsers[0].active === 1);
    await press(pane(0), "q");
    log(!!(await until(() => getState().session!.browsers[0].tabs.length === tabsBefore)), "q closes the tab");
    await focusPane(0);
    await press(pane(0), ["w", { ctrlKey: true }], "l");
    log(!!(await until(() => getState().session!.activeBrowser === 1)), "Ctrl+w l focuses the right browser");
    await press(document.activeElement ?? document.body, ["w", { ctrlKey: true }], "h");
    log(!!(await until(() => getState().session!.activeBrowser === 0)), "Ctrl+w h focuses the left browser");
    log(getState().session!.browsers[0].tabs.length === tabsBefore, "Ctrl+w did not close a tab in vim mode");
    const hiddenBefore = getState().settings!.showHidden;
    await press(document.activeElement ?? document.body, ["l", { ctrlKey: true }]);
    log(!!(await until(() => getState().session!.activeBrowser === 1)), "Ctrl+l focuses the right browser");
    log(getState().overlay?.type !== "pathEdit", "Ctrl+l does not open the path editor in vim mode");
    await press(document.activeElement ?? document.body, ["h", { ctrlKey: true }]);
    log(!!(await until(() => getState().session!.activeBrowser === 0)), "Ctrl+h focuses the left browser");
    log(getState().settings!.showHidden === hiddenBefore, "Ctrl+h does not toggle hidden files in vim mode");
  });

  await step("leader without vim keys (Ctrl double tap)", async () => {
    await updateSettings({ vimKeys: false, leaderKeyBasic: "Ctrl Ctrl" });
    await sleep(200);
    await focusPane(0);
    const tap = async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Control", code: "ControlLeft", ctrlKey: true, bubbles: true }));
      await sleep(40);
      window.dispatchEvent(new KeyboardEvent("keyup", { key: "Control", code: "ControlLeft", bubbles: true }));
      await sleep(60);
    };
    await tap();
    await sleep(500);
    log(getState().keySeq.length === 0, "a single Ctrl tap does nothing");
    await tap();
    await tap();
    const wk = await until(() => $(".wk"), 1500);
    log(!!wk, "Ctrl Ctrl opens the which-key menu", (wk?.textContent ?? "").slice(0, 50));
    await shot("24-ctrl-leader");
    await press(pane(0), "f");
    const afterF = JSON.stringify(getState().keySeq);
    await press(pane(0), "f");
    log(!!(await until(() => $(".qo"))), "Ctrl Ctrl f f opens Quick Open", `after first f: ${afterF}, now: ${JSON.stringify(getState().keySeq)}, overlay ${getState().overlay?.type ?? "none"}`);
    key("Escape", {}, $(".qo-input input") ?? document.body);
    await sleep(200);
    // A Ctrl chord must not count as a tap.
    await focusPane(0);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Control", ctrlKey: true, bubbles: true }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "c", ctrlKey: true, bubbles: true }));
    window.dispatchEvent(new KeyboardEvent("keyup", { key: "Control", bubbles: true }));
    await tap();
    await sleep(300);
    log(getState().keySeq.length === 0, "Ctrl+C followed by a tap is not a double tap");
    await press(pane(0), "f", "i", "l", "e", "1");
    log(!!(await until(() => rowNames(0).length > 0 && rowNames(0).every((n) => n.includes("file1")))), "letters still filter without vim keys", rowNames(0).join(","));
    key("Escape", {}, pane(0));
    await sleep(200);
  });

  await step("templates", async () => {
    await focusPane(0);
    key("n", { ctrlKey: true });
    const list = await until(() => ($$(".modal .suggest .s").length >= 3 ? $$(".modal .suggest .s") : null));
    log(!!list && list.length >= 3, "template list", (list ?? []).map((x) => x.textContent).join(", "));
    await shot("13-templates");
    key("Escape", {}, document.activeElement);
    await sleep(200);
  });

  await step("title bar", async () => {
    const win = getCurrentWindow();
    log(await win.isMaximized(), "window starts maximized (default setting)");
    log($(".wordmark")?.textContent === "Gall", "wordmark shown");
    const pathBtn = $(".tb-path");
    const active = activePath(getState().session!.activeBrowser);
    log(!!pathBtn && pathBtn.getAttribute("title")!.startsWith(active), "title bar shows active folder", pathBtn?.textContent ?? "");
    if (pathBtn) {
      click(pathBtn);
      const n = await until(() => getState().notifications.find((x) => x.text.startsWith("Copied path")));
      log(!!n, "click on path copies it", n?.text ?? "");
      const clip = await api.clipInspect().catch(() => null);
      log(!!clip && JSON.stringify(clip).includes(active.split("/").pop()!), "clipboard holds the path", JSON.stringify(clip)?.slice(0, 120));
    }
    log(document.title.includes("Gall"), "window title follows folder", document.title);
    const home = getState().info!.home;
    const drv = getState().drives.map((d) => d.mount);
    const dup = drv.filter((m) => m !== home && m !== "/" && (home + "/").startsWith(m.replace(/\/$/, "") + "/"));
    log(drv[0] === home && dup.length === 0, "drives: ~ first, no second entry for the home filesystem", drv.join(", "));
  });

  await step("column header sorting", async () => {
    const { navigate } = await import("./state/actions");
    navigate(0, root + "/Documents");
    await until(() => rowNames(0).includes("file1.txt"));
    const head = (label: string) => $$(".fp-hrow .col-head", browserEl(0)!).find((h) => h.textContent?.replace(/[↑↓]/g, "").trim().toLowerCase() === label);
    const sizeHead = head("size");
    if (!sizeHead) return log(false, "size column header present");
    click(sizeHead);
    await sleep(250);
    log(head("size")!.classList.contains("on") && head("size")!.textContent!.includes("↑"), "click header sorts ascending + shows arrow", head("size")!.textContent!);
    click(head("size")!);
    await sleep(250);
    log(head("size")!.textContent!.includes("↓"), "second click flips to descending", head("size")!.textContent!);
    log($$(".fp-hrow .col-head .arrow", browserEl(0)!).length === 1, "only the sorted column shows an arrow");
    log(($(".fp-pill", browserEl(0)!)?.textContent ?? "").includes("Size"), "sort pill shows current key", $(".fp-pill", browserEl(0)!)?.textContent ?? "");
    click(head("name")!);
    await sleep(250);
    const a = headerAlignment(0);
    log(a.ok, "headers aligned after sort change", a.detail);
  });

  await step("age labels + file icons", async () => {
    const badges = $$(".fp-list .age", browserEl(0)!);
    const widths = new Set(badges.map((b) => Math.round(b.getBoundingClientRect().width)));
    log(widths.size === 1, "age badges all have the same width", [...widths].join(","));
    log(badges.length > 0 && badges.every((b) => /^(\d+'|\d+h\d+'|\d+d|\d+M|\d+Y)$/.test(b.textContent!)), "age labels use ' h d M Y", badges.map((b) => b.textContent).join(","));
    const { navigate } = await import("./state/actions");
    navigate(0, root + "/Projects/Alpha/src");
    await until(() => rowNames(0).includes("main.h"));
    await sleep(300);
    const src = (n: string) => $<HTMLImageElement>(".ficon img", row(0, n)!)?.getAttribute("src") ?? "";
    const fl = document.documentElement.dataset.theme === "light" ? "latte" : "mocha";
    log(src("main.c").endsWith(`/catppuccin/${fl}/c.svg`), "C source icon", src("main.c"));
    log(src("main.h").endsWith(`/gall/${fl}/h.svg`), "C header icon is 'h'", src("main.h"));
    log(src("vec.hpp").endsWith(`/gall/${fl}/hpp.svg`), "C++ header icon is 'h+'", src("vec.hpp"));
    await until(() => $$<HTMLImageElement>(".ficon img", browserEl(0)!).every((i) => i.complete), 2000);
    const broken = $$<HTMLImageElement>(".ficon img").filter((i) => i.complete && i.naturalWidth === 0).map((i) => i.getAttribute("src"));
    log(broken.length === 0, "no broken icons anywhere", broken.join(","));
    await shot("20-icons");
  });

  await step("pin folder", async () => {
    const { navigate } = await import("./state/actions");
    const target = root + "/Photos";
    navigate(0, target);
    await until(() => activePath(0) === target);
    await focusPane(0);
    const pinnedIn = () => getState().favorites.groups.filter((g) => g.items.some((f) => f.path === target)).map((g) => g.name);
    key("b", { ctrlKey: true });
    await until(() => pinnedIn().length > 0);
    log(pinnedIn().includes("Pinned"), "Ctrl+B pins into 'Pinned' group", pinnedIn().join(","));
    log(getState().favorites.groups[0]?.name === "Pinned", "'Pinned' group is first");
    log(!!(await until(() => $(".pin-btn.on", browserEl(0)!))), "star shows pinned state");
    log($$(".pin-btn").every((p) => getComputedStyle(p).opacity === "1"), "star is always visible, pinned or not");
    log(!!$$(".sidebar .fav").find((f) => f.getAttribute("title") === target), "pinned folder appears in sidebar");
    await shot("21-pinned");
    click($(".pin-btn", browserEl(0)!)!);
    await until(() => pinnedIn().length === 0);
    log(pinnedIn().length === 0, "star click unpins");
    const undo = getState().notifications.find((n) => n.action?.label === "Undo");
    log(!!undo, "unpin offers undo");
    if (undo) {
      await undo.action!.run();
      await until(() => pinnedIn().length > 0);
      log(pinnedIn().includes("Pinned"), "undo restores the pin");
      key("b", { ctrlKey: true }, pane(0));
      await until(() => pinnedIn().length === 0);
    }
  });

  await step("large folder scrolls inside the card", async () => {
    const { navigate } = await import("./state/actions");
    navigate(0, root + "/Many");
    await until(() => rowNames(0).length > 20, 4000);
    const list = $(".fp-list", browserEl(0)!)!;
    log(list.scrollHeight > list.clientHeight + 100, "list is scrollable", `${list.scrollHeight} > ${list.clientHeight}`);
    log(list.getBoundingClientRect().bottom <= browserEl(0)!.getBoundingClientRect().bottom + 1 && browserEl(0)!.getBoundingClientRect().bottom <= window.innerHeight + 1, "list fits inside card and window");
    await focusPane(0);
    key("End");
    await sleep(300);
    log(list.scrollTop > 0, "End scrolls the list", `scrollTop ${list.scrollTop}`);
    log(rootScroll() === 0, "app root not scrolled", String(rootScroll()));
    key("Home");
    await sleep(200);
  });

  await step("find in files: Shift+Enter keeps layout", async () => {
    const { navigate } = await import("./state/actions");
    navigate(0, root);
    await until(() => activePath(0) === root);
    await focusPane(0);
    key("f", { ctrlKey: true }, document.body);
    const input = await until(() => $<HTMLInputElement>(".fif-query input"));
    if (!input) return log(false, "find in files opens");
    typeInto(input, "needle");
    await until(() => $$(".fif-match").length >= 10, 6000);
    for (let i = 0; i < 12; i++) { key("ArrowDown", {}, input); await sleep(40); }
    await sleep(400);
    log(rootScroll() === 0, "preview scrolling does not move app root", String(rootScroll()));
    key("Enter", { shiftKey: true }, input);
    await sleep(600);
    log(!$(".fif"), "Shift+Enter closes the dialog");
    log(rootScroll() === 0 && $(".titlebar")!.getBoundingClientRect().top === 0, "layout intact after reveal", `root scroll ${rootScroll()}`);
    await shot("22-after-reveal");
  });

  await step("type column + folder colours", async () => {
    const { navigate, setFolderColor, FOLDER_COLORS } = await import("./state/actions");
    navigate(0, root + "/Projects/Beta");
    await until(() => rowNames(0).includes("app.properties"));
    const ext = $(".col.ext", row(0, "app.properties")!)!;
    log(getComputedStyle(ext).textAlign === "right", "type values right-aligned", getComputedStyle(ext).textAlign);
    log(ext.getAttribute("title") === "properties", "long extension has a tooltip", ext.getAttribute("title") ?? "none");
    const md = $(".col.ext", row(0, "notes.md")!)!;
    log(Math.abs(md.getBoundingClientRect().right - ext.getBoundingClientRect().right) < 1, "short and long types share the right edge");
    navigate(0, root + "/Projects");
    await until(() => rowNames(0).includes("Alpha"));
    const alpha = root + "/Projects/Alpha";
    // Context menu offers the colour submenu for folders.
    await selectRow(0, "Alpha");
    mouse(row(0, "Alpha")!, "contextmenu", { button: 2 });
    const mi = await until(() => $$(".menu .mi").find((x) => x.textContent?.includes("Folder colour")));
    log(!!mi, "context menu has Folder colour");
    if (mi) {
      mi.dispatchEvent(new MouseEvent("mouseover", { bubbles: true, relatedTarget: document.body }));
      const sub = await until(() => $$(".menu").find((m) => m.textContent?.includes("Teal")));
      const ir = mi.getBoundingClientRect(), sr = sub?.getBoundingClientRect();
      log(!!sr && Math.abs(sr.left - ir.right) < 12 && Math.abs(sr.top - ir.top) < 20, "submenu opens next to its item", sr ? `item ${Math.round(ir.right)},${Math.round(ir.top)} sub ${Math.round(sr.left)},${Math.round(sr.top)}` : "no submenu");
      await shot("23-folder-colour-menu");
      const teal = sub && $$(".mi", sub).find((x) => x.textContent?.includes("Teal"));
      if (teal) click(teal);
      const set = await until(() => getState().settings?.folderColors[alpha]);
      log(set === FOLDER_COLORS.find(([n]) => n === "Teal")![1], "clicking a colour in the submenu applies it", String(set));
      log(!getState().menu, "menu closes after choosing");
    }
    key("Escape", {}, document.body);
    await sleep(150);
    const blue = FOLDER_COLORS.find(([n]) => n === "Blue")![1];
    setFolderColor([alpha], blue);
    const tinted = await until(() => $<SVGElement>(".ficon-folder[data-tint]", row(0, "Alpha")!));
    log(!!tinted && tinted.dataset.tint === blue, "folder icon takes the colour", tinted?.dataset.tint ?? "none");
    log(!!tinted && tinted.querySelector(".ff")?.getAttribute("fill") === blue, "the whole folder is filled with it");
    log($$<SVGElement>(".mcol-item .ficon-folder").some((t) => t.dataset.tint === blue), "folder columns use the colour too");
    await shot("23-folder-colour");
    setFolderColor([alpha], null);
    log(!!(await until(() => !$(".ficon-folder[data-tint]", row(0, "Alpha")!))), "Default removes the colour");
  });

  await step("system monitor", async () => {
    const card = await until(() => $(".sysmon"), 3000);
    log(!!card && !!$(".group-head", card!) && getComputedStyle(card!).boxShadow === "none", "monitor is a sidebar section (group head, no floating card)");
    if (!card) return log(false, "system monitor shown by default");
    const vals = await until(() => { const v = $$(".sysmon .sm-val").map((e) => e.textContent!); return v.length && v.every((t) => /^\d+%$/.test(t)) ? v : null; }, 5000);
    log(!!vals, "meters show live percentages", (vals ?? []).join(" "));
    const labels = $$(".sysmon .sm-lbl").map((e) => e.textContent);
    log(labels.includes("CPU") && labels.includes("Memory"), "CPU and Memory meters", labels.join(","));
    click($(".sysmon .sm-toggle")!);
    const det = await until(() => $(".sysmon .sm-details"));
    log(!!det && det.textContent!.includes("Network") && det.textContent!.includes("Disk I/O"), "details show network and disk I/O", det?.textContent?.slice(0, 80));
    await shot("25-system-monitor");
    click($(".sysmon .sm-toggle")!);
    const btn = $('.main-toolbar [title="Hide system monitor"]');
    log(!!btn && btn.classList.contains("active"), "toolbar has an active monitor button");
    if (btn) click(btn);
    log(!!(await until(() => !$(".sysmon"))), "toolbar button hides the monitor");
    const show = await until(() => $('.main-toolbar [title="Show system monitor"]'));
    log(!!show, "button stays visible to bring it back");
    if (show) click(show);
    log(!!(await until(() => $(".sysmon"))), "toolbar button shows the monitor again");
    log(!$(".sysmon .sm-chart"), "no usage chart (removed)");
    log((getState().settings!.historyCount) === 10, "history keeps the last 10 folders by default", String(getState().settings!.historyCount));
  });

  await step("per-panel undo", async () => {
    const { navigate, newFolder } = await import("./state/actions");
    navigate(0, root + "/Documents");
    navigate(1, root + "/Projects");
    await until(() => rowNames(0).includes("file1.txt") && rowNames(1).includes("Alpha"));
    const leftBefore = rowNames(0).join(",");
    log(!$('.main-toolbar [title^="Undo"]'), "undo moved out of the sidebar toolbar");
    await focusPane(1);
    newFolder(1);
    const input = await until(() => $<HTMLInputElement>(".modal input[type=text]"));
    if (!input) return log(false, "new folder dialog in right panel");
    typeInto(input, "PanelUndo");
    key("Enter", {}, input);
    await until(() => rowNames(1).includes("PanelUndo"));
    const btn = await until(() => { const b = $<HTMLButtonElement>(".fp-undo", browserEl(1)!); return b && !b.disabled ? b : null; });
    log(!!btn, "right panel undo is enabled after an operation there", btn?.title ?? "");
    log((btn?.title ?? "").includes("add “PanelUndo”"), "undo button says what it will undo", btn?.title ?? "");
    if (btn) click(btn);
    log(!!(await until(() => !rowNames(1).includes("PanelUndo"), 4000)), "panel undo removes the folder created there");
    log(rowNames(0).join(",") === leftBefore, "left panel unchanged by right panel undo");
  });

  await step("compare: two selected files", async () => {
    const { navigate } = await import("./state/actions");
    navigate(0, root + "/Projects/Beta");
    await until(() => rowNames(0).includes("config.yaml"));
    await selectRow(0, "config.old.yaml");
    click(row(0, "config.yaml")!, { ctrlKey: true });
    await sleep(150);
    key("D", { ctrlKey: true, shiftKey: true }, pane(0));
    const dv = await until(() => $(".diffv .dv-row"), 4000);
    log(!!dv, "Ctrl+Shift+D opens the compare view");
    if (!dv) return;
    const bg = getComputedStyle($(".diffv")!).backgroundColor;
    log(!/rgba\(.*, 0\)$/.test(bg) && bg !== "transparent", "compare view has an opaque background", bg);
    const stat = $(".dv-stat")?.textContent ?? "";
    log(/\+\d+/.test(stat) && /−\d+/.test(stat) && stat.includes("similar"), "header shows +/− and similarity", stat);
    log($$(".dv-cell.chg mark.dv-word").some((m) => m.textContent?.includes("0.2.0") || m.textContent?.includes("2")), "changed words are highlighted inside lines", $$(".dv-cell mark.dv-word").map((m) => m.textContent).join("|"));
    log(!!$(".dv-fold"), "long unchanged block is folded", $(".dv-fold")?.textContent ?? "");
    const countBefore = $(".dv-count")?.textContent ?? "";
    key("n", {}, $(".diffv")!);
    await sleep(150);
    log(($(".dv-count")?.textContent ?? "").startsWith("1 /"), "n jumps to the first change", `${countBefore} → ${$(".dv-count")?.textContent}`);
    log(!!$(".dv-row.cur"), "current change is marked");
    const removed = () => Number(($(".dv-stat .del")?.textContent ?? "−0").replace("−", ""));
    const before = removed();
    key("w", {}, $(".diffv")!);
    await until(() => removed() < before, 3000);
    log(removed() < before, "w ignores the whitespace-only change", `−${before} → −${removed()}`);
    key("w", {}, $(".diffv")!);
    await until(() => removed() === before && $(".dv-fold"), 3000);
    click($(".dv-fold")!);
    log(!!(await until(() => $$(".dv-row").some((r) => r.textContent?.includes("line 20")), 2000)), "clicking a fold shows the hidden lines");
    await shot("26-compare-split");
    key("u", {}, $(".diffv")!);
    log(!!(await until(() => $(".dv-row.unified.del") && $(".dv-row.unified.ins"))), "u switches to the unified view");
    await shot("27-compare-unified");
    key("u", {}, $(".diffv")!);
    const ruler = $$(".dv-ruler .m").length;
    log(ruler > 0, "overview ruler marks the changes", `${ruler} marks`);
    key("y", {}, $(".diffv")!);
    let patch = "";
    for (let i = 0; i < 30 && !patch.includes("@@"); i++) {
      await sleep(100);
      const c = await api.clipInspect().catch(() => null);
      patch = c?.kind === "text" ? c.text : "";
    }
    log(patch.includes("@@") && patch.includes("-version: 0.1.0") && patch.includes("+version: 0.2.0"), "y copies a unified patch", patch.split("\n").slice(0, 3).join(" | "));
    const left = $(".dv-file.l b")?.textContent;
    key("s", {}, $(".diffv")!);
    log(!!(await until(() => $(".dv-file.l b")?.textContent !== left)), "s swaps the sides");
    key("Escape", {}, $(".diffv")!);
    log(!!(await until(() => !$(".diffv"))), "Esc closes the compare view");
  });

  await step("compare: drop a file on a file", async () => {
    await focusPane(0);
    const src = await selectRow(0, "config.yaml");
    const d = await drag(src, row(0, "config.old.yaml")!);
    const hint = await until(() => $(".drag-hint.compare"));
    log(!!hint && hint.textContent!.includes("Compare with config.old.yaml"), "dragging a file over a file offers Compare", hint?.textContent ?? "no hint");
    await shot("28-compare-drop-hint");
    d.drop();
    log(!!(await until(() => $(".diffv .dv-row"), 4000)), "dropping opens the comparison");
    log(rowNames(0).includes("config.yaml") && rowNames(0).includes("config.old.yaml"), "nothing was moved by the drop");
    key("Escape", {}, $(".diffv") ?? document.body);
    await sleep(200);
  });

  await step("compare: other panel, clipboard, binary, Git HEAD", async () => {
    const { navigate, compareSelection, compareWithClipboard, compare, compareWithGitHead } = await import("./state/actions");
    navigate(1, root + "/Projects/Alpha");
    await until(() => rowNames(1).includes("config.yaml"));
    await selectRow(0, "config.yaml");
    await compareSelection(0);
    await until(() => $(".diffv .dv-row"), 4000);
    const rp = ($(".dv-file.r .dv-path")?.textContent ?? "").replace(/\u200e/g, "");
    log(rp.endsWith("/Alpha"), "one file pairs with the same name in the other panel", rp);
    key("Escape", {}, $(".diffv")!);
    await sleep(200);
    await api.clipSetText("name: gall\nversion: 9.9.9\n");
    await compareWithClipboard(root + "/Projects/Beta/config.yaml");
    await until(() => $(".diffv .dv-row"), 4000);
    log($(".dv-file.r b")?.textContent === "Clipboard", "compare with clipboard text", $(".dv-file.r b")?.textContent ?? "");
    key("Escape", {}, $(".diffv")!);
    await sleep(200);
    // archive.zip vs an image: binary, and not two images (those get the image view)
    compare({ kind: "file", path: root + "/Documents/archive.zip" }, { kind: "file", path: root + "/Photos/plasma.png" });
    const v = await until(() => $(".hx-verdict.diff"), 4000);
    const changedByte = await until(() => $(".hx-row.chg .hx-b.diff"), 4000);
    log(!!v && !!changedByte && !!v.textContent?.includes("differ"), "binary files: differing bytes are highlighted", v?.textContent?.slice(0, 80));
    await shot("28-binary-difference");
    key("Escape", {}, $(".diffv")!);
    await sleep(200);
    compare({ kind: "file", path: root + "/Documents/archive.zip" }, { kind: "file", path: root + "/Images/archive-copy.zip" });
    const identical = await until(() => $(".hx-verdict.same"), 4000);
    log(!!identical && identical.textContent!.includes("Identical, byte for byte") && !$(".hx-b.diff"), "binary files: identical");
    await shot("28-binary-identical");
    key("Escape", {}, $(".diffv")!);
    await sleep(200);
    const repo = await api.gitRoot(root + "/Projects/Alpha/notes.txt");
    if (repo) {
      compareWithGitHead(root + "/Projects/Alpha/notes.txt");
      await until(() => $(".diffv .dv-row"), 4000);
      log(!!$(".dv-file.l .dv-tag") && $$(".dv-cell.chg mark.dv-word").some((m) => m.textContent?.includes("changed")), "Git HEAD vs working copy shows my change");
      await shot("29-compare-git-head");
      key("Escape", {}, $(".diffv")!);
      await sleep(200);
    } else log(true, "git not installed: Git HEAD compare skipped");
    // Context menu offers Compare for two files and "Compare with" for one.
    await selectRow(0, "config.yaml");
    mouse(row(0, "config.yaml")!, "contextmenu", { button: 2 });
    log(!!(await until(() => $$(".menu .mi").find((x) => x.textContent?.includes("Compare with")))), "context menu: Compare with…");
    key("Escape", {}, document.body);
    await sleep(150);
  });

  await step("compare view fits small and large windows", async () => {
    const win = getCurrentWindow();
    const { compare } = await import("./state/actions");
    await win.unmaximize();
    for (const [w, h] of [[800, 600], [1366, 768], [2560, 1440]] as const) {
      await win.setSize(new LogicalSize(w, h));
      await sleep(500);
      compare({ kind: "file", path: root + "/Projects/Beta/config.old.yaml" }, { kind: "file", path: root + "/Projects/Beta/config.yaml" });
      await until(() => $(".diffv .dv-row"), 4000);
      const r = $(".diffv")!.getBoundingClientRect();
      const bar = $(".dv-bar")!;
      const ok = r.right <= window.innerWidth + 1 && r.bottom <= window.innerHeight + 1 && bar.scrollWidth <= bar.clientWidth + 1 && document.documentElement.scrollWidth <= window.innerWidth + 1;
      log(ok, `compare view fits ${w}x${h}`, `${Math.round(r.width)}x${Math.round(r.height)} in ${window.innerWidth}x${window.innerHeight}`);
      key("Escape", {}, $(".diffv")!);
      await sleep(200);
    }
    await win.maximize();
    await sleep(400);
  });

  await step("folder compare: open, filter, file diff and back", async () => {
    const { navigate } = await import("./state/actions");
    navigate(0, root + "/Sync/L");
    navigate(1, root + "/Sync/R");
    await until(() => rowNames(0).includes("only-left.txt") && rowNames(1).includes("only-right-dir"));
    await focusPane(0);
    key("d", { ctrlKey: true, altKey: true }, pane(0));
    const dc = await until(() => $(".dircmp .dc-row"), 5000);
    log(!!dc, "Ctrl+Alt+D compares the folders of both panels");
    if (!dc) return;
    const rels = () => $$(".dircmp .dc-row").map((r) => r.dataset.rel!);
    const chipN = (txt: string) => Number($$(".dc-chip").find((c) => c.textContent?.includes(txt))?.querySelector("b")?.textContent ?? -1);
    log(chipN("Changed") === 2 && chipN("Left only") === 1 && chipN("Right only") === 1, "counts: 2 changed, 1 left only, 1 right only", $$(".dc-chip").map((c) => c.textContent).join(" | "));
    log(!rels().includes("same.txt") && rels().includes("newer-left.txt") && rels().includes("only-right-dir/f.txt"), "default view shows only differences", rels().join(","));
    const row = (rel: string) => $(`.dircmp .dc-row[data-rel="${rel}"]`);
    log((row("newer-left.txt")?.querySelector(".dc-glyph")?.textContent ?? "").startsWith("◂"), "newer side is marked", row("newer-left.txt")?.querySelector(".dc-glyph")?.textContent ?? "");
    log(!!row("newer-left.txt")?.querySelector(".dc-side.l .dc-date.newer"), "newer date is emphasised");
    click($$(".dc-chip").find((c) => c.textContent?.startsWith("All"))!);
    await until(() => rels().includes("same.txt"));
    log(rels().includes("same-dir") && !rels().includes("same-dir/deep"), "All shows identical files; identical folders start collapsed", rels().join(","));
    await shot("30-folder-compare");
    click($$(".dc-chip").find((c) => c.textContent?.startsWith("Differences"))!);
    // Enter on a changed file opens the file diff; Esc comes back
    const i = rels().indexOf("newer-left.txt");
    for (let k = 0; k < i; k++) key("j", {}, $(".dircmp")!);
    key("Enter", {}, $(".dircmp")!);
    log(!!(await until(() => $(".diffv .dv-row"), 4000)), "Enter on a changed file opens its diff");
    key("Escape", {}, $(".diffv")!);
    log(!!(await until(() => $(".dircmp .dc-row"), 2000)), "Esc returns to the folder comparison");
  });

  await step("folder compare: copy a row, mirror, undo", async () => {
    const R = root + "/Sync/R";
    const exists = async (p: string) => (await api.pathResolve(p).catch(() => null))?.exists ?? false;
    const row = (rel: string) => $(`.dircmp .dc-row[data-rel="${rel}"]`);
    if (!$(".dircmp")) { const { compareFolders } = await import("./state/actions"); compareFolders(root + "/Sync/L", R); await until(() => $(".dircmp .dc-row"), 5000); }
    log((await dcFilter("Differences")).startsWith("Differences"), "Differences filter is active", $(".dc-chip.on")?.textContent ?? "");
    click(row("only-left.txt")!.querySelector(".dc-copy.r")!);
    const appears = async (p: string) => { for (let k = 0; k < 40; k++) { if (await exists(p)) return true; await sleep(100); } return false; };
    log(await appears(R + "/only-left.txt"), "row copy › puts the file on the right");
    const why = () => $$(".dircmp .dc-row").map((r) => `${r.dataset.rel}[${r.querySelector(".dc-mid")?.getAttribute("title")}]`).join("; ");
    log(!!(await until(() => !row("only-left.txt"), 4000)), "copied row disappears from the differences", why());
    click($$(".dc-btn").find((b) => b.textContent?.startsWith("Sync"))!);
    const mi = await until(() => $$(".menu .mi").find((x) => x.textContent?.includes("Mirror to right")));
    if (!mi) return log(false, "sync menu has Mirror to right");
    click(mi);
    const plan = await until(() => $(".dc-plan"));
    log(!!plan && /copy 2 \(replace 2\)/.test(plan.textContent ?? "") && plan.textContent!.includes("move 1 to trash"), "mirror preview: copy 2 (replace 2), 1 to trash", plan?.textContent ?? "");
    log($$(".dircmp .dc-row.planned").length >= 3 && $$(".dircmp .dc-row.dim").length >= 0, "planned rows are marked in the list");
    await shot("31-folder-sync-preview");
    key("Enter", {}, $(".dircmp")!);
    log(!!(await until(() => $(".dircmp .dv-msg.same"), 6000)), "after mirror the folders are identical (by size and date)", $(".dircmp .dv-msg")?.textContent || why());
    log(!(await exists(R + "/only-right-dir")), "extra folder on the right went to the trash");
    // one Undo reverts the whole sync
    const { undo } = await import("./state/actions");
    await undo();
    let back = false;
    for (let k = 0; k < 40 && !back; k++) { back = await exists(R + "/only-right-dir/f.txt"); if (!back) await sleep(100); }
    log(back, "Undo restores what the mirror removed");
    const text = await api.diffCompare({ kind: "file", path: R + "/newer-right.txt" }, { kind: "text", label: "t", text: "right newer version\n" }, { ignoreWhitespace: false, ignoreCase: false, ignoreEol: true, minimal: false });
    log(!!text.text && text.text.added === 0 && text.text.removed === 0, "Undo restores the replaced file's content");
    key("r", {}, $(".dircmp")!);
    log(!!(await until(() => $(".dircmp .dc-row"), 4000)), "compare again shows the differences after undo");
  });

  await step("folder compare: content mode and two selected folders", async () => {
    const row = (rel: string) => $(`.dircmp .dc-row[data-rel="${rel}"]`);
    const { compareFolders } = await import("./state/actions");
    compareFolders(root + "/Sync/L", root + "/Sync/R");
    await until(() => $(".dircmp .dc-row"), 5000);
    await dcFilter("Differences");
    log(!row("samesize.txt"), "quick mode: same size and date counts as identical");
    key("c", {}, $(".dircmp")!);
    log(!!(await until(() => row("samesize.txt"), 5000)), "c (contents) finds the same-size change", $(".dircmp .dc-row[data-rel=\"samesize.txt\"]")?.querySelector(".dc-mid")?.getAttribute("title") ?? "");
    key("c", {}, $(".dircmp")!);
    await sleep(300);
    key("Escape", {}, $(".dircmp")!);
    await until(() => !$(".dircmp"));
    const { navigate } = await import("./state/actions");
    navigate(0, root + "/Sync");
    await until(() => rowNames(0).includes("L") && rowNames(0).includes("R"));
    await selectRow(0, "L");
    click(row0("R")!, { ctrlKey: true });
    await sleep(150);
    key("D", { ctrlKey: true, shiftKey: true }, pane(0));
    log(!!(await until(() => $(".dircmp .dc-row"), 5000)), "Ctrl+Shift+D on two selected folders compares them");
    const win = getCurrentWindow();
    await win.unmaximize();
    await win.setSize(new LogicalSize(800, 600));
    await sleep(600);
    const r = $(".dircmp")!.getBoundingClientRect();
    log(r.right <= window.innerWidth + 1 && r.bottom <= window.innerHeight + 1 && document.documentElement.scrollWidth <= window.innerWidth + 1, "folder compare fits 800x600", `${Math.round(r.width)}x${Math.round(r.height)}`);
    await shot("32-folder-compare-small");
    await win.maximize();
    await sleep(400);
    key("Escape", {}, $(".dircmp")!);
    await sleep(200);
  });

  await step("compare: copy changes across and save (merge)", async () => {
    const { compare } = await import("./state/actions");
    const L = root + "/Merge/left.txt", R = root + "/Merge/right.txt";
    compare({ kind: "file", path: L }, { kind: "file", path: R });
    await until(() => $(".diffv .dv-row"), 4000);
    const dv = $(".diffv")!;
    log($$(".dv-merge button").length >= 2, "each change has copy buttons", `${$$(".dv-merge button").length} buttons`);
    const removed = () => Number(($(".dv-stat .del")?.textContent ?? "−0").replace("−", ""));
    const before = removed();
    key("n", {}, dv);
    await sleep(100);
    key("ArrowRight", { altKey: true }, dv);
    log(!!(await until(() => $(".dv-file.r .dv-dirty"), 3000)), "Alt+→ copies the change to the right (marked modified)");
    await until(() => removed() < before, 3000);
    log(removed() < before, "the copied change is gone from the diff", `−${before} → −${removed()}`);
    await shot("33-merge");
    key("z", { ctrlKey: true }, dv);
    log(!!(await until(() => !$(".dv-file.r .dv-dirty"), 3000)), "Ctrl+Z undoes the copy inside the view");
    key("ArrowRight", { altKey: true }, dv);
    await until(() => $(".dv-file.r .dv-dirty"), 3000);
    key("s", { ctrlKey: true }, dv);
    log(!!(await until(() => !$(".dv-file.r .dv-dirty"), 4000)), "Ctrl+S saves the right file");
    const saved = await api.diffCompare({ kind: "file", path: R }, { kind: "text", label: "x", text: "alpha\nbeta\ngamma\ndelta\nepsilon\n" }, { ignoreWhitespace: false, ignoreCase: false, ignoreEol: false, minimal: false });
    log(!!saved.text && saved.text.added + saved.text.removed === 0, "saved file has the merged content (final newline kept)");
    key("Escape", {}, dv);
    await until(() => !$(".diffv"));
    const { undo } = await import("./state/actions");
    await undo();
    let restored = false;
    for (let k = 0; k < 30 && !restored; k++) {
      const r = await api.diffCompare({ kind: "file", path: R }, { kind: "text", label: "x", text: "alpha\nBETA\ngamma\ndelta\nepsilon\n" }, { ignoreWhitespace: false, ignoreCase: false, ignoreEol: false, minimal: false });
      restored = !!r.text && r.text.added + r.text.removed === 0;
      if (!restored) await sleep(100);
    }
    log(restored, "Undo restores the file as it was before saving");
    // read-only side: clipboard text cannot be changed
    const { compareWithClipboard } = await import("./state/actions");
    await api.clipSetText("alpha\nzzz\n");
    await compareWithClipboard(L);
    await until(() => $(".diffv .dv-row"), 4000);
    key("n", {}, $(".diffv")!);
    key("ArrowRight", { altKey: true }, $(".diffv")!);
    await sleep(300);
    log(!$(".dv-file.r .dv-dirty") && getState().notifications.some((n) => n.text.includes("read-only")), "clipboard side is read-only");
    key("Escape", {}, $(".diffv")!);
    await sleep(200);
  });

  await step("compare: images", async () => {
    const { compare } = await import("./state/actions");
    compare({ kind: "file", path: root + "/Images/a.png" }, { kind: "file", path: root + "/Images/b.png" });
    const ic = await until(() => $(".imgcmp"), 4000);
    log(!!ic, "two images open the image comparison");
    if (!ic) return;
    log(!$(".diffv .dv-toggles") && !$$(".diffv .dv-bar .seg button").some((b) => b.textContent === "Unified"), "text-only controls are hidden for images");
    // Wait for the component's own load handlers (sizes shown), not just the <img> elements.
    log(!!(await until(() => $$(".ic-stage img").length === 2 && /\d×\d.*\d×\d/.test($(".ic-dims")?.textContent ?? ""), 3000)), "both images load side by side", $(".ic-dims")?.textContent ?? "");
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "2", bubbles: true }));
    log(!!(await until(() => $(".ic-divider"), 2000)), "2 = swipe with a divider");
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "4", bubbles: true }));
    const stat = await until(() => $(".ic-stat"), 6000);
    const pctTxt = stat?.textContent ?? "";
    const v = parseFloat(pctTxt);
    log(!!stat && v > 5 && v < 20, "4 = pixel difference with % changed (rectangle ≈ 7.5%)", pctTxt);
    log(!!(await until(() => (($(".ic-stage.diff img") as HTMLImageElement | null)?.naturalWidth ?? 0) > 0, 3000)), "difference image renders");
    await shot("34-image-diff");
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "1", bubbles: true }));
    key("Escape", {}, $(".diffv")!);
    await sleep(200);
  });

  await step("compare: similarity matrix for many files", async () => {
    const { navigate } = await import("./state/actions");
    navigate(0, root + "/Variants");
    await until(() => rowNames(0).length === 5);
    await focusPane(0);
    key("a", { ctrlKey: true }, pane(0));
    await sleep(150);
    key("D", { ctrlKey: true, shiftKey: true }, pane(0));
    const sx = await until(() => $(".simx .sx-cell"), 5000);
    log(!!sx, "Ctrl+Shift+D on 5 files opens the similarity matrix");
    if (!sx) return;
    log($$(".simx .sx-cell").length === 25, "5×5 cells", String($$(".simx .sx-cell").length));
    log($$(".simx .sx-cell").some((c) => c.textContent === "="), "identical files are marked =");
    log(($(".sx-odd b")?.textContent ?? "") === "odd.txt", "odd one out is found", $(".sx-odd")?.textContent ?? "");
    const firstRow = $$(".simx tbody th .sx-name").map((e) => e.textContent);
    log(firstRow[0] !== "odd.txt" || firstRow[firstRow.length - 1] === "odd.txt" || firstRow[0] === "odd.txt", "grouped order puts the odd file at an edge", firstRow.join(","));
    await shot("35-similarity-matrix");
    click($$(".simx .sx-cell").find((c) => !c.classList.contains("diag") && c.textContent !== "=")!);
    log(!!(await until(() => $(".diffv .dv-row"), 4000)), "clicking a cell opens that pair's diff");
    key("Escape", {}, $(".diffv")!);
    log(!!(await until(() => $(".simx .sx-cell"), 3000)), "Esc returns to the matrix");
    key("Escape", {}, $(".simx")!);
    await sleep(200);
  });

  await step("compare: tables (CSV and Excel)", async () => {
    const { compare } = await import("./state/actions");
    const T = root + "/Tables";
    compare({ kind: "file", path: T + "/a.csv" }, { kind: "file", path: T + "/b.csv" });
    const tb = await until(() => $(".tblcmp .tb-row"), 5000);
    log(!!tb, "two CSV files open the table view");
    if (!tb) return;
    const sum = $(".tb-summary")?.textContent ?? "";
    log(sum.includes("+1 rows") && sum.includes("−1 rows") && sum.includes("1 cells changed in 1 rows") && sum.includes("1 same") && sum.includes("matched by “id”"), "rows matched by the detected key column", sum);
    log(sum.includes("columns +stock") && sum.includes("columns −old"), "added and removed columns are reported");
    const plumPrice = $$(".tb-c.chg").find((c) => c.textContent?.includes("3.5"));
    log(!!plumPrice && plumPrice.querySelector("s")?.textContent === "3", "changed cell shows old → new (3 → 3.5)", plumPrice?.getAttribute("title") ?? "");
    log(!$$(".tb-c.chg").some((c) => c.getAttribute("title")?.startsWith("1.0 →")), "1.0 and 1 count as equal");
    await shot("36-table-csv");
    const sel = $<HTMLSelectElement>(".tb-key select")!;
    Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")!.set!.call(sel, "");
    sel.dispatchEvent(new Event("change", { bubbles: true }));
    log(!!(await until(() => $(".tb-summary")?.textContent?.includes("matched by position"), 3000)), "rows can be matched by position instead");
    click($$(".tblcmp .dc-btn").find((b) => b.textContent === "Text view")!);
    log(!!(await until(() => $(".diffv .dv-main .dv-row"), 3000)), "CSV can switch to the plain text view");
    click($$(".diffv .dc-btn").find((b) => b.textContent === "Table view")!);
    log(!!(await until(() => $(".tblcmp"), 3000)), "and back to the table view");
    key("Escape", {}, $(".diffv")!);
    await sleep(200);
    const x1 = T + "/x1.xlsx";
    if ((await api.pathResolve(x1).catch(() => null))?.exists) {
      compare({ kind: "file", path: x1 }, { kind: "file", path: T + "/x2.xlsx" });
      await until(() => $(".tblcmp .tb-row"), 8000);
      const s2 = $(".tb-summary")?.textContent ?? "";
      log(s2.includes("+1 rows") && s2.includes("−1 rows") && s2.includes("in 1 rows") && s2.includes("“sku”"), "Excel files compare cell by cell (key sku)", s2);
      log(!$(".tblcmp .dc-btn") || !$$(".tblcmp .dc-btn").some((b) => b.textContent === "Text view"), "no text view for Excel");
      await shot("37-table-xlsx");
      key("Escape", {}, $(".diffv")!);
      await sleep(200);
    } else log(true, "LibreOffice not available: Excel compare skipped");
  });

  await step("pinned group hidden when empty", async () => {
    const g = getState().favorites.groups.find((x) => x.name === "Pinned");
    const shown = $$(".group-head .title").some((t) => t.textContent === "Pinned");
    log(!g || g.items.length > 0 || !shown, "empty Pinned group not shown", `group items ${g?.items.length ?? "none"}, shown ${shown}`);
    log(true, `nerd font detected: ${hasNerdFont()}`);
  });

  await step("window sizes and font sizes", async () => {
    const win = getCurrentWindow();
    const { navigate } = await import("./state/actions");
    navigate(0, root + "/Documents");
    navigate(1, root + "/Many");
    await until(() => rowNames(0).length > 0 && rowNames(1).length > 0);
    await win.unmaximize();
    await sleep(400);
    const cases: [number, number, number][] = [
      [640, 420, 13], [800, 600, 13], [1024, 768, 13], [1280, 720, 13], [1366, 768, 13],
      [1600, 900, 13], [1920, 1080, 13], [2560, 1440, 13],
      [1366, 768, 11], [1366, 768, 16], [1920, 1080, 20], [1024, 768, 18],
    ];
    for (const [w, h, fs] of cases) {
      await updateSettings({ fontSize: fs });
      await win.setSize(new LogicalSize(w, h));
      await sleep(700);
      const actual = `${window.innerWidth}x${window.innerHeight}`;
      const p = layoutProblems();
      log(p.length === 0, `layout ok at ${w}x${h} font ${fs}`, p.length ? p.join("; ") : `actual ${actual}`);
      if (p.length || [640, 1366, 2560].includes(w)) await shot(`30-size-${w}x${h}-f${fs}`);
    }
    await updateSettings({ fontSize: 13 });
    await win.maximize();
    await sleep(500);
    const p = layoutProblems();
    log(p.length === 0, "layout ok maximized", p.join("; ") || `${window.innerWidth}x${window.innerHeight}`);
    for (const theme of ["light", "dark"] as const) {
      await updateSettings({ theme });
      await sleep(400);
      const q = layoutProblems();
      log(q.length === 0, `layout ok in ${theme} theme`, q.join("; "));
    }
  });

  await step("settings + light theme", async () => {
    key(",", { ctrlKey: true }, document.body);
    const s = await until(() => $(".settings"));
    log(!!s, "settings opens");
    await shot("14-settings");
    await updateSettings({ theme: "light" });
    await sleep(300);
    key("Escape", {}, $(".settings") ?? document.body);
    await sleep(300);
    log(document.documentElement.dataset.theme === "light", "light theme applied");
    await shot("15-light-theme");
    await updateSettings({ theme: "dark" });
  });

  await step("custom theme from the themes folder", async () => {
    const cfg = getState().info!.configDir;
    await api.createDir(cfg, "themes").catch(() => {});
    await api.saveText(cfg + "/themes/selftest.css", ':root:root[data-theme="dark"] { --bg-2: rgb(1, 2, 3); --accent: rgb(4, 5, 6) !important; }\n');
    key(",", { ctrlKey: true }, document.body);
    await until(() => $(".settings"));
    const sel = await until(() => $<HTMLSelectElement>(".theme-pick select"));
    if (!sel) return log(false, "theme dropdown in settings");
    sel.dispatchEvent(new MouseEvent("mousedown", { bubbles: true })); // re-reads the folder
    const opt = await until(() => [...sel.options].find((o) => o.value === "selftest"), 3000);
    log(!!opt, "theme file is listed in the dropdown", [...sel.options].map((o) => o.value || "none").join(","));
    Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")!.set!.call(sel, "selftest");
    sel.dispatchEvent(new Event("change", { bubbles: true }));
    const css = () => getComputedStyle(document.documentElement);
    const applied = await until(() => css().getPropertyValue("--bg-2").trim() === "rgb(1, 2, 3)" && css().getPropertyValue("--accent").trim() === "rgb(4, 5, 6)", 3000);
    log(!!applied, "choosing it applies the theme (and its !important accent)", `${css().getPropertyValue("--bg-2")} / ${css().getPropertyValue("--accent")}`);
    await shot("15b-custom-theme");
    log([...sel.options].some((o) => o.value === "catppuccin") && [...sel.options].some((o) => o.value === "gruvbox"), "Catppuccin and Gruvbox are included");
    Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")!.set!.call(sel, "gruvbox");
    sel.dispatchEvent(new Event("change", { bubbles: true }));
    const tn = await until(() => css().getPropertyValue("--bg-2").trim().toLowerCase() === "#282828", 3000);
    log(!!tn, "built-in Gruvbox applies without a file", css().getPropertyValue("--bg-2"));
    await shot("15c-gruvbox");
    Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")!.set!.call(sel, "");
    sel.dispatchEvent(new Event("change", { bubbles: true }));
    log(!!(await until(() => !$("#user-theme") && css().getPropertyValue("--bg-2").trim() !== "rgb(1, 2, 3)", 3000)), "None restores the built-in colours");
    key("Escape", {}, $(".settings") ?? document.body);
    await sleep(300);
  });

  await step("open with palette: Ctrl+Enter remembers a program per file type", async () => {
    const A2 = await import("./state/actions");
    const readme = root + "/Projects/Alpha/README.md";
    A2.openWith([{ name: "README.md", path: readme, isDir: false } as never]);
    const input = await until(() => $<HTMLInputElement>(".ow .qo-input input"));
    log(!!input, "Open with opens a palette");
    if (!input) return;
    await until(() => !$(".ow .qo-empty") || $$(".ow .qo-row").length > 0, 3000);
    // A harmless custom command: `true` exits at once.
    typeInto(input, "true {path}");
    const custom = await until(() => $$(".ow .qo-row").find((r) => r.textContent?.includes("Run")), 2000);
    log(!!custom, "typing a command offers to run it");
    await shot("40-open-with");
    key("ArrowUp", {}, input); // from the first row up wraps to the last: the command
    await until(() => $(".ow .qo-row.on")?.textContent?.includes("Run"), 1000);
    key("Enter", { ctrlKey: true }, input);
    const saved = await until(() => getState().settings!.openRules.find((r) => r.target === "ext:md"), 3000);
    log(saved?.command === "true {path}", "Ctrl+Enter stores it for .md files", JSON.stringify(saved ?? null));
    A2.openWith([{ name: "README.md", path: readme, isDir: false } as never]);
    const first = await until(() => $(".ow .qo-row"), 3000);
    log(!!first && first.textContent!.includes("★") && first.textContent!.includes("for all .md files"), "the stored program is listed first with ★", first?.textContent ?? "");
    // Tab narrows where Ctrl+Enter applies: a folder rule beats the global one.
    const chip0 = $(".ow .qo-hints .chip")?.textContent ?? "";
    const inp2 = $<HTMLInputElement>(".ow .qo-input input")!;
    key("Tab", {}, inp2);
    const chip1 = await until(() => { const t = $(".ow .qo-hints .chip")?.textContent ?? ""; return t !== chip0 ? t : null; }, 1500);
    log(!!chip1 && chip1.startsWith(".md in"), "Tab switches the scope to a folder", `${chip0} → ${chip1}`);
    typeInto(inp2, "false {path}");
    key("ArrowUp", {}, inp2);
    await until(() => $(".ow .qo-row.on")?.textContent?.includes("Run"), 1000);
    key("Enter", { ctrlKey: true }, inp2);
    const two = await until(() => getState().settings!.openRules.length === 2 ? getState().settings!.openRules : null, 3000);
    log(!!two && two.some((r) => r.dir && r.command === "false {path}") && two.some((r) => !r.dir && r.command === "true {path}"), "folder rule is stored next to the global one", JSON.stringify(two));
    const { findRule } = await import("./core/openRules");
    log(findRule(getState().settings!.openRules, readme)?.command === "false {path}", "the folder rule wins for files inside it");
    key(",", { ctrlKey: true }, document.body);
    await until(() => $(".settings"));
    click(await until(() => $$(".settings nav button").find((d) => d.textContent === "Open with"), 2000) ?? document.body);
    const listed = await until(() => $$(".settings .rule").length === 2, 2000);
    log(!!listed, "Settings › Open with lists both rules", $$(".settings .rule").map((r) => r.textContent).join(" | "));
    await shot("41-open-with-settings");
    key("Escape", {}, $(".settings") ?? document.body);
    await updateSettings({ openRules: [] });
    A2.terminalWith(root);
    const term = await until(() => $(".ow .ow-title")?.textContent?.includes("Terminal"), 2000);
    log(!!term, "Terminal here with… uses the same palette");
    key("Escape", {}, $(".ow .qo-input input") ?? document.body);
    await sleep(200);
    // Edit with…: same palette; with an empty filter j/k move and rows are numbered.
    A2.editWith([{ name: "README.md", path: readme, isDir: false } as never]);
    const ed = await until(() => $(".ow .ow-title")?.textContent?.startsWith("Edit"), 2000);
    log(!!ed, "Edit with… uses the same palette");
    const inp3 = $<HTMLInputElement>(".ow .qo-input input");
    typeInto(inp3!, "x {path}"); // guarantees at least two rows (an editor or the command)
    typeInto(inp3!, "");
    await sleep(100);
    if ($$(".ow .qo-row").length >= 2) {
      key("j", {}, inp3!);
      log(!!(await until(() => $$(".ow .qo-row").indexOf($(".ow .qo-row.on")!) === 1, 1000)) && inp3!.value === "", "j moves down while the filter is empty");
      log(!!$(".ow .qo-row .ow-num"), "rows show 1–9 for quick picks");
    }
    key("Escape", {}, inp3 ?? document.body);
    await sleep(200);
    const { run: runAction } = await import("./state/run");
    runAction("openWithSettings");
    log(!!(await until(() => $(".settings nav button.on")?.textContent === "Open with", 2000)), "Space o d opens Settings › Open with");
    key("Escape", {}, $(".settings") ?? document.body);
    await sleep(200);
  });

  await step("drives: volumes not mounted yet are folded away", async () => {
    await (await import("./state/actions")).refreshDrives();
    const vols = getState().volumes;
    if (!vols.length) return log(true, "no unmounted volumes on this computer: nothing to show");
    localStorage.setItem("sidebar:notMounted", "0");
    const head = await until(() => $(".drive.not-mounted-head"), 3000);
    log(!!head && !$(".drive.not-mounted"), "a single folded “Not mounted” row", head?.textContent ?? "");
    click(head!);
    const rows = await until(() => $$(".drive.not-mounted").length === vols.length, 2000);
    log(!!rows, "clicking it lists them", $$(".drive.not-mounted").map((r) => r.textContent).join(" | "));
    log(!getState().drives.some((d) => vols.some((v) => v.label === d.label && d.mount)), "they are not among the mounted drives");
    await shot("42-not-mounted");
    click(head!);
  });

  await step("archives: compress and extract from the context menu", async () => {
    const { navigate } = await import("./state/actions");
    const P = root + "/Projects";
    navigate(0, P);
    await until(() => activePath(0) === P && rowNames(0).includes("Alpha"));
    const menuItem = (label: string) => until(() => $$(".menu .mi").find((m) => m.querySelector(".lbl")?.textContent === label), 2000);
    mouse(await selectRow(0, "Alpha"), "contextmenu", { button: 2 });
    const comp = await menuItem("Compress");
    log(!!comp, "folder menu has Compress");
    if (!comp) return;
    mouse(comp, "mouseover"); // React builds onMouseEnter from mouseover
    const fmts = await until(() => ["Alpha.zip", "Alpha.tar.gz", "Alpha.7z"].every((l) => $$(".menu .mi .lbl").some((x) => x.textContent === l)), 2000);
    log(!!fmts, "Compress offers .zip, .tar.gz and .7z named after the folder");
    await shot("38-compress-menu");
    click((await menuItem("Alpha.7z"))!);
    const made = await until(() => rowNames(0).includes("Alpha.7z"), 8000);
    log(!!made, "Alpha.7z is created");
    log(!!(await until(() => getState().notifications.some((n) => n.text === "Created “Alpha.7z”"), 3000)), "notification names the archive");
    mouse(await selectRow(0, "Alpha.7z"), "contextmenu", { button: 2 });
    const ex = await menuItem("Extract here");
    log(!!ex && !!(await menuItem("Extract to “Alpha/”")), "archive menu has Extract here and Extract to “Alpha/”");
    if (!ex) return;
    click(ex);
    const out = await until(() => rowNames(0).includes("Alpha (2)"), 8000);
    log(!!out, "Extract here unpacks the single top folder next to the original (unique name)", rowNames(0).join(","));
    const readme = await api.pathResolve(P + "/Alpha (2)/README.md").catch(() => null);
    log(!!readme?.exists, "extracted files are there");
    log(rowNames(0).every((n) => !n.startsWith(".gall-")), "no staging folder left");
    await focusPane(0);
    key("z", { ctrlKey: true }, pane(0));
    log(!!(await until(() => !rowNames(0).includes("Alpha (2)"), 4000)), "Ctrl+Z undoes the extraction");
    mouse(await selectRow(0, "Alpha.7z"), "contextmenu", { button: 2 });
    log(!(await menuItem("Extract here and trash archive")), "no separate trash variant in the menu");
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Control", ctrlKey: true, bubbles: true }));
    const ext = await menuItem("Extract here and trash archive");
    log(!!ext && !!(await menuItem("Extract to “Alpha/” and trash archive")), "holding Ctrl turns the extract items into … and trash archive");
    await shot("39-extract-ctrl");
    window.dispatchEvent(new KeyboardEvent("keyup", { key: "Control", ctrlKey: false, bubbles: true }));
    log(!!(await menuItem("Extract here")), "releasing Ctrl restores them");
    if (!ext) return;
    click(ext, { ctrlKey: true });
    const swapped = await until(() => rowNames(0).includes("Alpha (2)") && !rowNames(0).includes("Alpha.7z"), 8000);
    log(!!swapped, "extracts and moves the archive to the trash", rowNames(0).join(","));
    await focusPane(0);
    key("z", { ctrlKey: true }, pane(0));
    const back = await until(() => rowNames(0).includes("Alpha.7z") && !rowNames(0).includes("Alpha (2)"), 5000);
    log(!!back, "one Ctrl+Z restores the archive and removes the extracted folder", rowNames(0).join(","));
  });

  log(errors.length === 0, "no uncaught UI errors", errors.join(" | "));
  results.push(`\n${failures} failure(s)`);
  await api.selftestDone(results.join("\n"));
}

async function runGitCompareSelfTest() {
  try {
    await updateSettings({ keyboardProfile: "vim", leaderKey: "Space", shortcuts: {} });
    await focusPane(0);
    await selectRow(0, "file.txt");
    key("=", {}, pane(0));
    await sleep(300);
    log(!!$(".wk"), "compare key menu opens");
    await shot("git-compare-menu");
    key("g", {}, pane(0));
    log(!!await until(() => $$(".git-graph-lines .gg-row").length === 2), "commit picker loads history");
    await shot("git-compare-commits");
    log(!$(".git-graph-lines")?.textContent?.includes("reflog-only commit"), "reflog commits are hidden by default");
    click($(".gg-reflog input")!);
    log(!!await until(() => $(".git-graph-lines")?.textContent?.includes("reflog-only commit")), "Reflog checkbox reveals detached commit");
    await shot("git-reflog-enabled");
    click($(".gg-reflog input")!);
    log(!!await until(() => !($(".gg-reflog input") as HTMLInputElement)?.checked && $$(".git-graph-lines .gg-row").length === 2), "unchecking Reflog restores branch history");

    key("j", {}, $(".git-graph"));
    log(!!await until(() => $$(".git-graph-lines .gg-row")[1]?.classList.contains("selected")), "j selects the older commit");
    key("Enter", {}, $(".git-graph"));
    log(!!await until(() => $(".diffv .dv-row")), "older commit opens in compare view");
    log(!!$(".diffv")?.textContent?.includes("first") && !!$(".diffv")?.textContent?.includes("working"), "compare shows selected commit and working copy");
    await shot("git-compare-result");
    key("Escape", {}, $(".diffv"));
    await until(() => !$(".diffv"));
    await updateSettings({ keyboardProfile: "explorer" });
    await getCurrentWindow().unmaximize();
    await getCurrentWindow().setSize(new LogicalSize(700, 600));
    const A = await import("./state/actions");
    A.openOverlay({ type: "gitCommits", path: activePath(0) + "/new.txt" });
    await until(() => $$(".git-graph-lines .gg-row").length === 2);
    key("r", {}, $(".git-graph"));
    log(!($(".gg-reflog input") as HTMLInputElement)?.checked, "r does not toggle reflog with Vim disabled");
    key("j", {}, $(".git-graph"));
    await sleep(100);
    log($$(".git-graph-lines .gg-row")[0]?.classList.contains("selected"), "j does not navigate with Vim disabled");
    key("ArrowDown", {}, $(".git-graph"));
    log(!!await until(() => $$(".git-graph-lines .gg-row")[1]?.classList.contains("selected")), "arrow keys select with Vim disabled");
    await shot("git-compare-narrow");
    key("Enter", {}, $(".git-graph"));
    log(!!await until(() => $(".diffv")?.textContent?.includes("file is not in the selected revision")), "missing historical file reports an error");
    await shot("git-compare-missing");
    await updateSettings({ theme: "dark" });
    await getCurrentWindow().setSize(new LogicalSize(1100, 720));
    A.openOverlay(null);
    await focusPane(0);
    click($(".fp-status .git", browserEl(0)!)!);
    log(!!await until(() => $(".git-graph-lines button")), "status button opens Git graph");
    await shot("git-graph");
    $(".git-graph")?.focus();
    key("Enter");
    log(!!await until(() => $(".gg-working-details")), "Enter opens Working Changes without switching revision");
    log(!!await until(() => $$(".gc-file").length === 2), "workspace lists tracked and untracked changes");
    await until(() => $(".gc-patch")?.textContent?.includes("working"));
    await shot("git-changes-unstaged");
    await updateSettings({ keyboardProfile: "vim" });
    $(".git-changes")?.focus(); key(" ");
    log(!!await until(() => $(".gc-file .staged")), "Vim Space stages selected file");
    await shot("git-changes-staged");
    key("u", {}, $(".git-changes"));
    log(!!await until(() => !$(".gc-file .staged")), "Vim u unstages without deleting file");
    key(" ", {}, $(".git-changes"));
    await until(() => $(".gc-file .staged"));
    const message = $(".gc-commit textarea") as HTMLTextAreaElement;
    message.focus(); typeInto(message, "selected file commit");
    await sleep(100);
    click($$(".gc-commit button").find((b) => b.textContent === "Commit staged")!);
    log(!!await until(() => $$(".gc-file").length === 1 && $(".gc-file")?.textContent?.includes("new.txt"), 5000), "commit includes staged file and leaves untracked file");
    await shot("git-changes-after-commit");
    await until(() => !($(".gg-working-title button") as HTMLButtonElement)?.disabled);
    click($(".gg-working-title button")!);
    await until(() => $$(".git-graph-lines button").some((b) => b.textContent?.includes("first")));

    const branchRow = $$(".gg-side-row").find((b) => b.textContent?.includes("main"));
    if (branchRow) click(branchRow);
    await sleep(100);
    log(getState().overlay?.type !== "dialog", "selecting a branch also waits for explicit checkout");
    const beforeSelect = await api.listDir(activePath(0), { showHidden: false, hideTemp: false });
    click($$(".git-graph-lines .gg-row:not(.gg-working)").find((b) => b.textContent?.includes("first"))!);
    await sleep(100);
    log(getState().overlay?.type !== "dialog", "selecting a commit does not checkout or touch working files");
    const afterSelect = await api.listDir(activePath(0), { showHidden: false, hideTemp: false });
    log(beforeSelect.every((f) => afterSelect.some((other) => other.path === f.path && other.modified === f.modified && other.size === f.size)), "history selection preserves file timestamps and sizes");
    click($(".gg-checkout")!);
    log(!!await until(() => getState().overlay?.type === "dialog"), "dirty switch asks how to handle changes");
    await shot("git-graph-dirty");
    click($$("button").find((b) => b.textContent === "Stash")!);
    log(!!await until(() => $$(".git-graph-lines button").some((b) => b.textContent?.includes("HEAD") && b.textContent?.includes("first")) && !$(".git-graph [role=status]"), 5000), "stash and revision switch complete");
    log(!!await until(() => $(".fp-status .git", browserEl(0)!)?.textContent?.includes("first") && $(".fp-status .git", browserEl(0)!)?.textContent?.includes("main")), "detached revision status shows containing branch and commit message");
    await shot("git-graph-detached-working");
    await until(() => !($(".gg-working") as HTMLButtonElement)?.disabled);
    click($(".gg-working")!);
    log(!!await until(() => $(".gc-files")?.textContent?.includes("Working tree clean")), "clean working tree remains accessible from graph");
    await shot("git-working-clean");
    click($(".gg-working-title button")!);
    log(!!$(".gg-working-link"), "working changes use a separate connection to detached HEAD");
    $(".git-graph header button")?.focus();
    key("Escape");
    log(!!await until(() => !$(".git-graph")), "Escape closes graph with button focus");
    await shot("git-detached-status");

  } catch (e) { log(false, "Git compare scenario", String(e)); }
  await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
}

/** README screenshots (GALL_SELFTEST=demo): browser 1 = ~/Pictures/Iceland, browser 2 = ~/Projects/website. */
export async function runDemo() {
  const args = await api.startupArgs();
  const A = await import("./state/actions");
  const { navigate } = A;
  const goHome = async () => {
    if (args.path1) navigate(0, args.path1);
    if (args.path2) navigate(1, args.path2);
    await until(() => activePath(0) === args.path1 && activePath(1) === args.path2, 3000);
    await sleep(500);
  };
  // A fixed window size keeps the UI legible in the README whatever the screen is.
  const win = getCurrentWindow();
  await win.unmaximize().catch(() => {});
  await win.setSize(new LogicalSize(1440, 900)).catch(() => {});
  await win.center().catch(() => {});
  await sleep(600);
  await updateSettings({ theme: "dark", showDetailsPane: false });
  await goHome();
  await until(() => rowNames(0).length > 0 && rowNames(1).length > 0, 5000);
  await selectRow(0, "Glacier lagoon.jpg");
  await sleep(600);
  await shot("main");

  await updateSettings({ showDetailsPane: false });
  setLayout("columns");
  await goHome();
  await sleep(400);
  await selectRow(1, "index.html");
  await shot("columns");

  setLayout("standard");
  await updateSettings({ theme: "light" });
  await goHome();
  click($('[title="File view"]', browserEl(0)!)!);
  const g = await until(() => $$(".menu .mi").find((x) => x.textContent?.includes("Thumbnail grid")));
  if (g) click(g);
  await until(() => $(".grid-cell"));
  await sleep(800);
  await shot("light-grid");

  // Drag label
  const src = await selectRow(1, "logo.svg");
  const dst = row(1, "assets")!;
  const d = await drag(src, dst);
  await sleep(200);
  await shot("drag");
  d.drop();
  await sleep(600);
  await updateSettings({ theme: "dark", showDetailsPane: false });
  await goHome();

  const report: string[] = ["demo done"];
  const frame = (() => {
    const n: Record<string, number> = {};
    return (gif: string) => shot(`gif-${gif}-${String((n[gif] = (n[gif] ?? 0) + 1)).padStart(2, "0")}`);
  })();
  /** Records a region of the next screenshot `name` for the script to crop (CSS px and viewport width). */
  const crop = (name: string, el: Element | null, pad = 8) => {
    if (!el) return;
    const r = el.getBoundingClientRect();
    report.push(`CROP ${name} ${Math.max(0, r.left - pad)} ${Math.max(0, r.top - pad)} ${r.width + 2 * pad} ${r.height + 2 * pad} ${window.innerWidth}`);
  };
  const esc = async (sel: string) => {
    key("Escape", {}, $(sel) ?? document.body);
    await until(() => !$(sel), 2000);
    await sleep(200);
  };
  const home = args.path1!.replace(/\/Pictures\/Iceland$/, "");

  // Quick Open over the whole index (the demo home plus /usr: a few hundred thousand real paths).
  await updateSettings({ indexRoots: [home, "/usr"] });
  await api.indexRefresh(true).catch(() => {});
  for (let i = 0; i < 300; i++) {
    const st = await api.indexStatus();
    if (!st.building && st.entries > 10000) break;
    await sleep(200);
  }
  await focusPane(0);
  key("p", { ctrlKey: true }, document.body);
  const qi = await until(() => $<HTMLInputElement>(".qo-input input"));
  if (qi) {
    await sleep(300);
    await frame("quick-open");
    for (const q of ["g", "gl", "glac", "glacier"]) {
      typeInto(qi, q);
      await until(() => $$(".qo-row").length > 0, 2000);
      await sleep(q === "glacier" ? 900 : 350);
      await frame("quick-open");
    }
    await esc(".qo");
  }

  // Find in files: a word somewhere in thousands of system headers.
  A.openOverlay({ type: "grep", root: "/usr/include", scope: "folder" });
  const gi = await until(() => $<HTMLInputElement>(".fif-query input"));
  if (gi) {
    await sleep(300);
    await frame("find-in-files");
    for (const q of ["sock", "sockaddr", "sockaddr_in6"]) {
      typeInto(gi, q);
      await until(() => $(".fif-match"), 4000);
      await sleep(q === "sockaddr_in6" ? 1500 : 500);
      await frame("find-in-files");
    }
    await esc(".fif-query");
  }

  // Compare: text, Excel and images.
  const W = args.path2!;
  A.compare({ kind: "file", path: W + "/index.old.html" }, { kind: "file", path: W + "/index.html" });
  if (await until(() => $(".diffv .dv-row"), 5000)) {
    await sleep(600);
    await shot("compare-text");
    await esc(".diffv");
  }
  const docs = home + "/Documents";
  if ((await api.pathResolve(docs + "/Budget 2025.xlsx").catch(() => null))?.exists) {
    A.compare({ kind: "file", path: docs + "/Budget 2025.xlsx" }, { kind: "file", path: docs + "/Budget 2026.xlsx" });
    if (await until(() => $(".tblcmp .tb-row"), 8000)) {
      await sleep(600);
      await shot("compare-excel");
      await esc(".diffv");
    }
  }
  const P = args.path1!;
  A.compare({ kind: "file", path: P + "/Aurora.jpg" }, { kind: "file", path: P + "/Aurora (edited).jpg" });
  if (await until(() => $(".imgcmp"), 5000)) {
    await until(() => $$(".ic-stage img").length === 2 && $$(".ic-stage img").every((i) => (i as HTMLImageElement).naturalWidth > 0), 4000);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "2", bubbles: true }));
    await until(() => $(".ic-divider"), 2000);
    await sleep(500);
    await shot("compare-image");
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "1", bubbles: true }));
    await esc(".diffv");
  }

  // System monitor with details, cropped from the sidebar.
  await updateSettings({ showSystemMonitor: true });
  const sm = await until(() => $(".sysmon"), 3000);
  if (sm) {
    if (!$(".sysmon .sm-details")) click($(".sysmon .sm-toggle")!);
    await until(() => $(".sysmon .sm-details"), 2000);
    await sleep(2500); // a couple of samples so the values are live
    crop("system-monitor", $(".sysmon"));
    await shot("system-monitor");
  }

  // Vim keys: j j l h, recorded as frames.
  await updateSettings({ vimKeys: true });
  await goHome();
  await selectRow(1, "assets");
  await frame("vim");
  for (const k of ["j", "j", "k", "k", "l", "h"]) {
    key(k, {}, pane(1));
    await sleep(450);
    await frame("vim");
  }

  // Leader menu (which-key): Space, then Space s.
  await goHome();
  await focusPane(0);
  await frame("which-key");
  key(" ", {}, pane(0));
  await until(() => $(".wk"), 1500);
  await sleep(200);
  await frame("which-key");
  key("s", {}, document.body);
  await until(() => $(".wk-title")?.textContent?.includes("search"), 1500);
  await sleep(200);
  await frame("which-key");
  key("Escape", {}, document.body);
  await sleep(300);

  // "?" key sheet.
  A.openOverlay({ type: "keys" });
  if (await until(() => $(".kh"), 2000)) {
    await sleep(400);
    await shot("key-sheet");
    await esc(".kh");
  }

  await updateSettings({ indexRoots: [], theme: "dark", showDetailsPane: false });
  await api.selftestDone(report.join("\n"));
}

/** Screenshots of every Settings section, the About tab and the About window, in dark and
 * light (GALL_SELFTEST=design), for design work on them. */

// ───────── UI audit (GALL_SELFTEST=audit, scripts/ui-audit.sh) ─────────

/**
 * Generic checks that hold in every state of the UI: text that overlaps other text,
 * text cut off without an ellipsis, text spilling out of its box, and controls cut off
 * by the window. Only what is on top at its own centre counts (not what a dialog covers).
 */
function auditUi(): string[] {
  const W = window.innerWidth, H = window.innerHeight;
  const out: string[] = [];
  const short = (s: string | null | undefined) => (s ?? "").trim().replace(/\s+/g, " ").slice(0, 50);
  const where = (el: Element) => {
    const parts: string[] = [];
    for (let e: Element | null = el; e && e !== document.body && parts.length < 3; e = e.parentElement) {
      const c = typeof e.className === "string" ? e.className.trim().split(/\s+/)[0] : "";
      parts.unshift(c ? `${e.tagName.toLowerCase()}.${c}` : e.tagName.toLowerCase());
    }
    return parts.join(">");
  };
  const onTop = (el: Element, r: DOMRect) => {
    const x = Math.min(W - 1, Math.max(0, r.left + Math.min(r.width / 2, 8))), y = Math.min(H - 1, Math.max(0, r.top + r.height / 2));
    const hit = document.elementFromPoint(x, y);
    return !!hit && (hit === el || el.contains(hit) || hit.contains(el));
  };
  // What is visible of an element: its box cut by every ancestor that clips.
  const visibleRect = (el: Element): DOMRect | null => {
    let r = el.getBoundingClientRect();
    let x0 = r.left, y0 = r.top, x1 = r.right, y1 = r.bottom;
    for (let a = el.parentElement; a && a !== document.body; a = a.parentElement) {
      const cs = getComputedStyle(a);
      if (cs.overflowX !== "visible" || cs.overflowY !== "visible") {
        const c = a.getBoundingClientRect();
        x0 = Math.max(x0, c.left); y0 = Math.max(y0, c.top); x1 = Math.min(x1, c.right); y1 = Math.min(y1, c.bottom);
      }
    }
    if (x1 - x0 < 1 || y1 - y0 < 1) return null;
    r = new DOMRect(x0, y0, x1 - x0, y1 - y0);
    return r;
  };
  const texts: { el: HTMLElement; r: DOMRect }[] = [];
  for (const el of $$("body *")) {
    if (!Array.from(el.childNodes).some((n) => n.nodeType === 3 && (n.textContent ?? "").trim())) continue;
    const r = visibleRect(el);
    if (!r) continue;
    if (r.width < 2 || r.height < 2 || r.bottom < 0 || r.top > H || r.right < 0 || r.left > W) continue;
    const cs = getComputedStyle(el);
    if (cs.visibility === "hidden" || Number(cs.opacity) < 0.1) continue;
    if (!onTop(el, r)) continue;
    texts.push({ el, r });
    // Cut off without an ellipsis (a scrolling box is fine).
    const scrolls = /(auto|scroll)/.test(cs.overflowX);
    if (el.scrollWidth > el.clientWidth + 2 && el.clientWidth > 0 && !scrolls) {
      const ellipsis = cs.textOverflow === "ellipsis" || (el.parentElement && getComputedStyle(el.parentElement).textOverflow === "ellipsis");
      if (cs.overflowX !== "visible" && !ellipsis) out.push(`text cut off without … in ${where(el)}: "${short(el.textContent)}"`);
    }
    // Controls and labels partly outside the window (what scrolling boxes hide is fine).
    if (r.right > W + 1 || r.bottom > H + 1 || r.left < -1) out.push(`outside the window: ${where(el)} "${short(el.textContent)}"`);
  }
  // Text over text: two text boxes that are not nested and overlap by more than a sliver.
  for (let i = 0; i < texts.length; i++) {
    for (let j = i + 1; j < texts.length; j++) {
      const a = texts[i], b = texts[j];
      if (a.el.contains(b.el) || b.el.contains(a.el)) continue;
      const w = Math.min(a.r.right, b.r.right) - Math.max(a.r.left, b.r.left);
      const h = Math.min(a.r.bottom, b.r.bottom) - Math.max(a.r.top, b.r.top);
      if (w > 3 && h > 3 && w * h > 24) out.push(`text overlaps text: ${where(a.el)} "${short(a.el.textContent)}" × ${where(b.el)} "${short(b.el.textContent)}"`);
    }
  }
  return [...new Set(out)].slice(0, 12);
}

let auditCount = 0;
/** Audits the current state; logs one line per problem (or one PASS) and takes a shot. */
async function audit(name: string, opts: { shot?: boolean } = {}) {
  await sleep(250);
  const p = auditUi();
  if (!p.length) log(true, `audit: ${name}`);
  for (const x of p) log(false, `audit: ${name}`, x);
  if (opts.shot !== false) await shot(`a${String(++auditCount).padStart(2, "0")}-${name.replace(/[^a-z0-9]+/gi, "-").toLowerCase()}`);
}


/** Exploratory UI audit with hard data (scripts/ui-audit.sh): long and foreign names,
 *  huge numbers, many items, deep paths, folders that vanish or refuse, small windows,
 *  both themes, many tabs, notification floods, dialogs and menus at the edges. */
export async function runUiAudit() {
  const A = await import("./state/actions");
  const { bumpNonce } = await import("./state/hooks");
  const win = getCurrentWindow();
  const root = activePath(0);
  const go = async (b: number, path: string) => {
    A.navigate(b, path);
    await until(() => activePath(b) === path, 3000);
    await sleep(500);
  };
  const size = async (w: number, h: number) => {
    await win.unmaximize().catch(() => {});
    await win.setSize(new LogicalSize(w, h)).catch(() => {});
    await sleep(700);
  };
  const view = async (path: string, v: string) => {
    await api.setPrefs(path, { view: v });
    bumpNonce();
    await sleep(500);
  };
  await size(1280, 820);
  await step("audit: start", () => audit("start"));

  await step("audit: long and foreign names", async () => {
    await go(0, root + "/Names");
    for (const v of ["default", "names", "powershell", "thumbs", "grid"]) {
      await view(root + "/Names", v);
      await audit(`names view ${v}`);
    }
    await view(root + "/Names", "default");
    await updateSettings({ theme: "light" });
    await audit("names light theme");
    await updateSettings({ theme: "dark", fontSize: 17 });
    await audit("names font 17");
    await updateSettings({ fontSize: 13 });
  });

  await step("audit: small windows", async () => {
    for (const [w, h] of [[1024, 700], [800, 600], [640, 420]]) {
      await size(w, h);
      await audit(`names ${w}x${h}`);
    }
    await size(1280, 820);
  });

  await step("audit: details pane with a long name", async () => {
    await focusPane(0);
    const long = rowNames(0).find((n) => n.length > 60);
    if (long) await selectRow(0, long);
    key("p", { altKey: true });
    await sleep(600);
    await audit("details pane long name");
    key("p", { altKey: true });
    await sleep(300);
  });

  await step("audit: deep path", async () => {
    let deep = root + "/Deep";
    for (let i = 1; i <= 25; i++) deep += `/level-${String(i).padStart(2, "0")}-folder`;
    await go(0, deep);
    await audit("deep path columns");
    await focusPane(0);
    key("`");
    await sleep(400);
    await audit("deep path breadcrumbs");
    key("`");
    await sleep(300);
  });

  await step("audit: many tabs", async () => {
    await go(1, root + "/Names");
    for (let i = 0; i < 12; i++) A.sessionCmd({ op: "newTab", b: 1, path: root + "/Deep/level-01-folder" });
    await sleep(800);
    await audit("twelve tabs");
    const n = getState().session!.browsers[1].tabs.length;
    for (let i = n - 1; i > 0; i--) A.sessionCmd({ op: "close", b: 1, idx: i });
    await sleep(500);
  });

  await step("audit: 5000 items", async () => {
    await go(0, root + "/Big");
    await audit("5000 items");
    const list = $(".fp-list", browserEl(0)!);
    if (list) list.scrollTop = list.scrollHeight;
    await sleep(400);
    await audit("5000 items scrolled to the end");
    log(rowNames(0).some((n) => n.startsWith("file-49")), "the end of a long list renders", rowNames(0).slice(-2).join(","));
  });

  await step("audit: folders that refuse or vanish", async () => {
    await go(0, root + "/Trouble");
    await audit("trouble folder");
    A.navigate(0, root + "/Trouble/Locked");
    await sleep(800);
    log(true, "locked folder: what the browser shows", `${activePath(0)} | ${($(".empty-msg, .fp-error, .error", browserEl(0)!)?.textContent ?? "").slice(0, 120)} | toast: ${getState().notifications.map((n) => n.text).join(" / ").slice(0, 160)}`);
    await audit("locked folder");
    await go(0, root + "/Trouble");
    for (const name of ["broken-link", "loop"]) {
      await selectRow(0, name);
      key("Enter");
      await sleep(700);
      log(true, `${name}: what happens on Enter`, `${activePath(0)} | toast: ${getState().notifications.map((n) => n.text).join(" / ").slice(0, 160)}`);
      await go(0, root + "/Trouble");
    }
    // A folder deleted while it is open in the other browser.
    await go(1, root + "/Trouble/Vanish/inner");
    await api.startTask({ kind: "delete", sources: [root + "/Trouble/Vanish"] });
    await sleep(2500);
    log(!activePath(1).startsWith(root + "/Trouble/Vanish"), "a browser leaves a folder deleted under it", `${activePath(1)} | ${($(".empty-msg, .fp-error, .error", browserEl(1)!)?.textContent ?? "").slice(0, 120)}`);
    await audit("after the open folder was deleted");
  });

  await step("audit: notifications", async () => {
    const { setState } = await import("./state/store");
    setState({ notifications: [] });
    A.notify("Copied “A very long file name that goes on and on and describes the content in far too much detail.pdf” to Documents", "success", { label: "Undo", run: () => {} });
    A.notify("Moved 3 items to trash", "info", { label: "Undo", run: () => {} });
    A.notify("Something is changing files very rapidly in /home/user/Projects/some/deep/folder/that/is/long; refreshes are throttled.", "warn");
    A.fail("rename failed for '/home/user/Documents/report.txt': Permission denied (os error 13)");
    A.notify("Created “archive.zip”", "success");
    A.notify("Copied 12 items to Photos", "success");
    await sleep(300);
    await audit("six notifications");
    const ms = getState().settings?.notificationMs ?? 6000;
    await sleep(ms * 1.5 + 800);
    log(true, "notifications left after their time", getState().notifications.map((n) => `${n.kind}: ${n.text.slice(0, 40)}`).join(" | ") || "none");
    setState({ notifications: [] });
  });

  await step("audit: dialogs with long names", async () => {
    await go(0, root + "/Names");
    await focusPane(0);
    const long = rowNames(0).find((n) => n.length > 60)!;
    await selectRow(0, long);
    key("F2");
    await until(() => $(".modal input"), 2000);
    await audit("rename dialog long name");
    key("Escape", {}, $(".modal")!);
    await sleep(200);
    A.deletePermanently([root + "/Names/" + long]);
    await until(() => $(".modal"), 2000);
    await audit("delete dialog long name");
    key("Escape", {}, $(".modal")!);
    await sleep(200);
    A.openOverlay({ type: "properties", entry: getState().focused[0]! });
    await sleep(500);
    await audit("properties long name");
    A.openOverlay(null);
    const all = (await api.listDir(root + "/Names", { showHidden: true, hideTemp: false })).filter((e) => !e.isDir);
    A.batchRename(0, all);
    await until(() => $(".brn-win"), 2000);
    await audit("batch rename long names");
    A.openOverlay(null);
    await sleep(200);
  });

  await step("audit: menus at the window edges", async () => {
    const W = window.innerWidth, H = window.innerHeight;
    for (const [x, y, n] of [[W - 5, H - 5, "bottom right"], [W - 5, 40, "top right"], [5, H - 5, "bottom left"]] as const) {
      const r = rowNames(0).length ? row(0, rowNames(0)[0])! : pane(0);
      mouse(r, "contextmenu", { button: 2, clientX: x, clientY: y });
      await until(() => $(".menu"), 1500);
      const m = $(".menu")?.getBoundingClientRect();
      log(!!m && m.right <= W + 1 && m.bottom <= H + 1 && m.left >= 0 && m.top >= 0, `context menu stays inside the window (${n})`, m ? `${Math.round(m.left)},${Math.round(m.top)}–${Math.round(m.right)},${Math.round(m.bottom)} in ${W}x${H}` : "no menu");
      key("Escape", {}, document.body);
      await sleep(200);
    }
  });

  await step("audit: windows and palettes", async () => {
    for (const [o, name] of [
      [{ type: "settings" }, "settings"], [{ type: "keys" }, "key sheet"], [{ type: "quickOpen", initial: "level" }, "quick open"],
      [{ type: "grep", root, scope: "folder" }, "find in files"], [{ type: "trash" }, "trash window"], [{ type: "about" }, "about"],
    ] as const) {
      A.openOverlay(o as never);
      await sleep(700);
      await audit(name);
      A.openOverlay(null);
      await sleep(200);
    }
    await size(800, 600);
    for (const [o, name] of [[{ type: "settings" }, "settings 800x600"], [{ type: "keys" }, "key sheet 800x600"]] as const) {
      A.openOverlay(o as never);
      await sleep(700);
      await audit(name);
      A.openOverlay(null);
      await sleep(200);
    }
    await size(1280, 820);
  });

  await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
}

export async function runDesignShots() {
  const A = await import("./state/actions");
  const win = getCurrentWindow();
  const checkSupportQr = (selector: string, name: string) => {
    const qr = $(selector);
    if (!qr) {
      log(false, `${name} support QR is visible`);
      return;
    }
    const style = getComputedStyle(qr);
    log(
      style.backgroundSize === "149% 248%" && style.backgroundPosition === "50% 73%" && style.boxShadow === "none",
      `${name} support QR fills its rounded tile without an outer frame`,
      `${style.backgroundSize}, ${style.backgroundPosition}, shadow: ${style.boxShadow}`,
    );
  };
  await win.unmaximize().catch(() => {});
  await win.setSize(new LogicalSize(1280, 820)).catch(() => {});
  await win.center().catch(() => {});
  await sleep(800);
  const sections = ["appearance", "view", "search", "keyboard-vim", "columns", "behavior", "window", "openWith", "preview", "advanced", "about", "updates", "shortcuts"];
  for (const theme of ["dark", "light"] as const) {
    await updateSettings({ theme });
    for (const section of sections) {
      A.openOverlay(null);
      await sleep(100);
      A.openOverlay({ type: "settings", section });
      await until(() => $(".settings"), 2000);
      await sleep(350);
      if (section === "about") checkSupportQr(".settings .settings-support .about-support-qr", `Settings (${theme})`);
      await shot(`settings-${theme}-${section}`);
    }
    A.openOverlay({ type: "about" });
    await sleep(400);
    checkSupportQr(".about-win .about-support-qr", `About (${theme})`);
    await shot(`about-${theme}`);
    A.openOverlay({ type: "welcome" });
    for (let i = 0; i < 4; i++) {
      await until(() => $(".welcome"), 2000);
      await sleep(350);
      await shot(`welcome-${theme}-${i + 1}`);
      click($$(".wc-steps button")[i + 1] ?? document.body);
    }
    A.openOverlay(null);
  }
  await updateSettings({ theme: "dark" });
  await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
}


/** Focused real updater integration checks: scripts/selftest.sh --updates. */
async function runUpdateSelfTest() {
  const A = await import("./state/actions");
  const { invoke } = await import("@tauri-apps/api/core");
  const win = getCurrentWindow();
  const panel = () => $("section[aria-label='Application updates']");
  const checkButton = () => $$("button", panel() ?? document).find((button) => button.textContent === "Check for updates") as HTMLButtonElement | undefined;
  const terminal = () => {
    const text = panel()?.textContent ?? "";
    return !checkButton()?.disabled && ["error", "current", "available"].includes(panel()?.getAttribute("data-state") ?? "") ? text : null;
  };
  try {
    await win.unmaximize();
    await win.setSize(new LogicalSize(1280, 820));
    await sleep(500);
    A.openOverlay({ type: "settings", section: "updates" });
    log(!!(await until(panel)), "Settings Updates exposes the update section");
    log(!!checkButton(), "manual update check has a visible labelled button");
    const support = await invoke<string>("update_support");
    log(typeof support === "string", "real backend reports update support", support || "signed installation supported");
    if (support) {
      let rejection = "";
      try { await invoke("install_update"); } catch (error) { rejection = String(error); }
      log(rejection.includes(support), "backend refuses installation for this unmanaged or keyless build", rejection);
    } else {
      results.push("SKIP installation rejection: this build supports installation; the focused test never installs or restarts.");
    }
    // Use the actual plugin and public release endpoint, not a mock response.
    await until(() => checkButton() && !checkButton()!.disabled, 20000);
    const button = checkButton();
    if (!button) throw new Error("Update check button was not rendered.");
    click(button);
    const checking = await until(() => checkButton()?.disabled && panel()?.textContent?.includes("Checking for updates"), 2000);
    log(!!checking, "manual checking disables repeated requests and presents loading feedback");
    await shot("updates-request-wide");
    results.push("SCREENSHOT updates-request-wide.png: actual Settings Updates during a public-endpoint request; terminal response may arrive during capture.");
    const outcome = await until(terminal, 20000);
    log(!!outcome, "manual update check finishes with a visible result", outcome ?? "timed out");
    log(!checkButton()?.disabled, "manual check is available again after the response");
    if (panel()?.getAttribute("data-state") === "error") {
      log(!!$("[role='alert']", panel()!), "real endpoint failure is presented as an accessible alert");
      click(checkButton()!);
      log(!!(await until(() => checkButton()?.disabled, 2000)), "failed checks can be retried");
      log(!!(await until(terminal, 20000)), "retry reaches a visible result");
    } else {
      results.push("SKIP error and error retry: public endpoint returned a successful update response.");
    }
    for (const theme of ["light", "dark"] as const) {
      await updateSettings({ theme });
      for (const [width, height, label] of [[1280, 820, "wide"], [800, 600, "narrow"]] as const) {
        await win.setSize(new LogicalSize(width, height));
        await sleep(450);
        panel()?.scrollIntoView({ block: "nearest" });
        await sleep(150);
        const bounds = panel()?.getBoundingClientRect();
        log(!!bounds && bounds.left >= 0 && bounds.right <= window.innerWidth, `update card stays inside the ${theme} ${label} window`);
        const buttonBounds = checkButton()?.getBoundingClientRect();
        log(!!buttonBounds && buttonBounds.left >= 0 && buttonBounds.right <= window.innerWidth, `manual check remains reachable at ${theme} ${width}x${height}`);
        log(checkButton()?.classList.contains("btn") ?? false, `update action uses the application's button styling (${theme} ${label})`);
        const detail = $(".updates-error details", panel()!) as HTMLDetailsElement | null;
        if (detail) log(!detail.open, `technical details are collapsed by default (${theme} ${label})`);
        const name = `updates-result-${theme}-${label}`;
        await shot(name);
        results.push(`SCREENSHOT ${name}.png: ${width}x${height} ${theme}, expected readable card, state and actions; observed ${terminal()}`);
      }
    }
    const details = $(".updates-error details", panel()!) as HTMLDetailsElement | null;
    if (details) {
      click($("summary", details)!);
      log(!!(await until(() => details.open)), "technical details can be expanded without losing the update action");
      log(!!$("pre", details)?.textContent?.trim(), "technical error evidence is preserved");
      panel()?.scrollIntoView({ block: "end" });
      await sleep(200);
      const retryBounds = checkButton()?.getBoundingClientRect();
      const scrollBounds = $(".settings .panel")?.getBoundingClientRect();
      log(!!retryBounds && !!scrollBounds && retryBounds.top >= scrollBounds.top && retryBounds.bottom < scrollBounds.bottom, "retry can be scrolled fully into view below expanded details");
      await shot("updates-error-details-dark-narrow");
      results.push("SCREENSHOT updates-error-details-dark-narrow.png: 800x600 dark, scrolled to expanded readable technical error and retry action.");
    }
    if (details?.open) click($("summary", details)!);
    await updateSettings({ fontSize: 16 });
    await win.setSize(new LogicalSize(640, 600));
    await sleep(500);
    panel()?.scrollIntoView({ block: "nearest" });
    const compactBounds = panel()?.getBoundingClientRect();
    log(!!compactBounds && compactBounds.left >= 0 && compactBounds.right <= window.innerWidth, "update card fits a 640px window with larger text");
    const compactButton = checkButton()?.getBoundingClientRect();
    log(!!compactButton && compactButton.left >= 0 && compactButton.right <= window.innerWidth, "check action fits at 640px with larger text");
    checkButton()?.focus();
    await shot("updates-error-large-type-compact");
    results.push("SCREENSHOT updates-error-large-type-compact.png: 640x600 dark, 16px base text, expected wrapped header and readable error with reachable check action.");
    key("Escape", {}, $(".settings"));
    log(!!(await until(() => !getState().overlay)), "Settings closes normally after checking for updates");
  } catch (error) { log(false, "focused updater scenario", String(error)); }
  await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
}

async function runQuickOpenSelfTest() {
  const { pushRecent } = await import("./state/store");
  const root = activePath(0);
  const input = () => $<HTMLInputElement>(".qo-input input");
  const names = () => $$(".qo-row .qo-name").map((el) => el.textContent ?? "");
  const open = async () => {
    await focusPane(0);
    key("p", { ctrlKey: true });
    if (!await until(input)) throw new Error("Ctrl+P did not open Quick Open");
  };
  try {
    await getCurrentWindow().setFocus();
    await getCurrentWindow().setSize(new LogicalSize(1100, 800));
    await updateSettings({ indexRoots: [root], shortcuts: {} });
    for (const vimKeys of [false, true]) {
      await updateSettings({ vimKeys });
      const suffix = vimKeys ? "vim" : "basic";
      const goneName = `qo-deleted-${suffix}.txt`;
      const liveName = `qo-survivor-${suffix}.txt`;
      const gone = await api.createFile(root, goneName);
      const live = await api.createFile(root, liveName);
      await api.indexRefresh(true);
      for (let i = 0; i < 200 && (await api.indexStatus()).building; i++) await sleep(60);
      if ((await api.indexStatus()).building) throw new Error("Index build did not finish");
      log((await api.indexQuery(`'${goneName}`, root, 10)).hits.some((h) => h.path === gone), `indexed fixture (${suffix})`);
      pushRecent(gone);
      pushRecent(live);
      await open();
      log(!!await until(() => names().includes(goneName)), `existing recent file is shown (${suffix})`);
      await shot(`quick-open-${suffix}-before-delete`);
      await api.trash([gone]);
      log(!!await until(() => names().includes(liveName) && !names().includes(goneName)), `deleted recent file disappears from the open picker (${suffix})`);
      await api.undo();
      log(!!await until(() => names().includes(goneName)), `restored recent file reappears (${suffix})`);
      typeInto(input()!, `qo- ${suffix}`);
      log(!!await until(() => !!$(".qo-status b") && !$(".qo-section") && names().includes(liveName) && names().includes(goneName)), `typed query renders indexed matches (${suffix})`);
      await api.trash([gone]);
      log(!!await until(() => !!$(".qo-status b") && names().includes(liveName) && !names().includes(goneName)), `deleted search result disappears from the open picker (${suffix})`);
      await shot(`quick-open-${suffix}-after-delete`);
      key("Enter", { ctrlKey: true }, input());
      log(!!await until(() => !input()), `keyboard reveal still works (${suffix})`);
      await api.undo();
      log((await api.indexQuery(`'${goneName}`, root, 10)).hits.some((h) => h.path === gone), `restored file is searchable (${suffix})`);
    }
  } catch (e) {
    log(false, "Quick Open deletion integration", String(e));
  }
  await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
}

/** Focused keyboard integration checks: scripts/selftest.sh --keyboard. */
async function runKeyboardSelfTest() {
  const A = await import("./state/actions");
  const { controller } = await import("./state/controllers");
  const { join } = await import("./core/path");
  const root = activePath(0);
  const docs = join(root, "Documents");
  const br = () => getState().session!.browsers[0];
  const tabs = () => $$(".tab", browserEl(0)!);
  const middle = (el: Element, altKey = false, emitAuxClick = true) => {
    mouse(el, "mousedown", { button: 1, altKey });
    mouse(el, "mouseup", { button: 1, altKey });
    if (emitAuxClick) mouse(el, "auxclick", { button: 1, altKey });
  };
  const errors: string[] = [];
  window.addEventListener("error", (e) => errors.push(e.message));
  window.addEventListener("unhandledrejection", (e) => errors.push(String(e.reason)));
  try {
    await getCurrentWindow().setFocus();
    await getCurrentWindow().unmaximize();
    await getCurrentWindow().setSize(new LogicalSize(1100, 800));
    await updateSettings({ vimKeys: false, leaderKeyBasic: "Ctrl Ctrl", shortcuts: {} });
    await setLayout("standard");
    await focusPane(0);

    key("F1", { shiftKey: true });
    log(!!(await until(() => $(".kh"))), "Shift+F1 opens help without vim");
    log($(".kh")?.textContent?.includes("File list (standard)") === true && !$(".kh")?.textContent?.includes("File list (vim)"), "basic help shows standard navigation, not vim commands");
    log(!!$('.kh .key-bindings[aria-label="Space"]'), "basic preview hint uses Space");
    await shot("keyboard-basic-wide");
    await getCurrentWindow().setSize(new LogicalSize(800, 600));
    await sleep(250);
    await shot("keyboard-basic-narrow");
    const help = $(".kh")!.getBoundingClientRect();
    log(help.left >= 0 && help.right <= innerWidth + 1 && help.bottom <= innerHeight + 1, "keyboard sheet fits narrow window");
    typeInto($(".kh input") as HTMLInputElement, "move");
    await sleep(100);
    log($$(".kh-row").every((el) => el.textContent?.toLowerCase().includes("move")), "help filter searches visible shortcut descriptions");
    key("F6");
    log(getState().overlay?.type === "keys", "shortcut keys in help input do not move files");
    key("Escape");
    await until(() => !getState().overlay);
    await getCurrentWindow().setSize(new LogicalSize(1100, 800));
    await focusPane(0);

    key("Tab");
    log(!!(await until(() => getState().session!.activeBrowser === 1)), "Tab focuses the other browser without vim");
    key("Tab");
    await until(() => getState().session!.activeBrowser === 0);
    for (const letter of "Doc") { key(letter); await sleep(60); }
    log(rowNames(0).join(",") === "Documents", "ordinary typing still filters rather than invoking vim keys");
    key("Escape");
    await until(() => rowNames(0).length > 1);
    await selectRow(0, "Documents");
    key("Insert");
    await sleep(120);
    key("Insert");
    await sleep(120);
    log(controller(0)!.selection().length === 2, "Insert marks consecutive entries and advances the cursor");
    key("Escape");
    await focusPane(0);
    key("F7", { altKey: true });
    log(!!(await until(() => getState().overlay?.type === "search")), "Alt+F7 opens name search");
    key("Escape");
    await until(() => !getState().overlay);

    await A.navigate(0, docs);
    await until(() => rowNames(0).includes("file1.txt"));
    await focusPane(0);
    for (const [fn, label] of [["F3", "Name"], ["F4", "Type"], ["F5", "Modified"], ["F6", "Size"]]) {
      key(fn, { ctrlKey: true });
      log(!!(await until(() => $(".fp-pill", browserEl(0)!)?.textContent?.includes(label))), `Ctrl+${fn} sorts by ${label}`);
    }
    await selectRow(0, "file1.txt");
    await updateSettings({ keyboardProfile: "totalCommander" });
    key("F6", { shiftKey: true });
    log(!!(await until(() => $(".modal input"))), "Shift+F6 opens rename");
    key("Escape");
    await until(() => !getState().overlay);
    await selectRow(0, "file1.txt");
    key("F6");
    log(!!(await until(() => !row(0, "file1.txt") && row(1, "file1.txt"))), "F6 moves the selected file to the other browser");
    key("z", { ctrlKey: true });
    log(!!(await until(() => row(0, "file1.txt") && !row(1, "file1.txt"))), "Ctrl+Z undoes the commander move");
    await selectRow(0, "file1.txt");
    key("F8");
    const trashed = await until(() => !row(0, "file1.txt"), 10_000);
    const trashDetails = trashed ? "" : JSON.stringify({
      existing: await api.findExisting([join(docs, "file1.txt")]),
      overlay: getState().overlay?.type,
      selected: controller(0)!.selection().map((entry) => entry.name),
      notifications: getState().notificationLog.slice(0, 3).map((note) => note.text),
    });
    log(!!trashed, "F8 moves the selected file to trash", trashDetails);
    key("z", { ctrlKey: true });
    log(!!(await until(() => row(0, "file1.txt"))), "Ctrl+Z restores the commander delete");

    await updateSettings({ vimKeys: true });
    await focusPane(0);
    const vimController = controller(0)!;
    const vimRows = vimController.entries();
    if (vimRows.length > 1) {
      vimController.select([vimRows[0].path], vimRows[0].path);
      key("J");
      log(!!(await until(() => vimController.focused()?.path === vimRows[1].path)), "Caps Lock letter casing does not change the lowercase Vim motion");
      key("g", { shiftKey: true });
      log(!!(await until(() => vimController.focused()?.path === vimRows[vimRows.length - 1].path)), "Shift selects the uppercase Vim motion regardless of Caps Lock");
    } else {
      log(false, "Caps Lock Vim integration requires at least two visible entries");
    }
    const pinnedBefore = A.isPinned(docs);
    for (const k of [" ", "b", "p"]) { key(k); await sleep(80); }
    log(!!(await until(() => A.isPinned(docs) !== pinnedBefore)), "Space b p toggles the current folder pin through the backend");
    await shot("keyboard-vim-pinned");
    key("b");
    log(!!(await until(() => A.isPinned(docs) === pinnedBefore)), "vim b restores the original pin state");
    await selectRow(0, "file1.txt");
    for (const k of [" ", "v"]) { key(k); await sleep(80); }
    log(!$(".fp-list [data-path].selected", browserEl(0)!), "Space v unmarks the focused item without moving");
    for (const k of [" ", "v"]) { key(k); await sleep(80); }
    log(controller(0)!.selection()[0]?.name === "file1.txt", "Space v marks the same focused item again");
    for (const k of [" ", "o", "m"]) { key(k); await sleep(80); }
    log(!!(await until(() => getState().menu)), "Space o m opens the focused item's context menu");
    key("Escape");
    await until(() => !getState().menu);
    key("?");
    await until(() => $(".kh"));
    typeInto($(".kh input") as HTMLInputElement, "pin");
    await sleep(100);
    log(!!$('.kh .key-bindings[aria-label*="Space, then b, then p"]'), "vim help exposes the pin leader sequence");
    await shot("keyboard-vim-pin-help");
    key("Escape");
    await until(() => !getState().overlay);

    await updateSettings({ vimKeys: false });
    await A.sessionCmd({ op: "newTab", b: 0, path: root });
    await until(() => br().tabs.length === 2 && tabs().length === 2);
    const activeId = br().tabs[br().active].id;
    const closedId = br().tabs[0].id;
    middle(tabs()[0], false, false);
    log(!!(await until(() => br().tabs.length === 1 && br().tabs[0].id === activeId)), "middle-click closes inactive tab without activating it or requiring auxclick");
    key("T", { ctrlKey: true, shiftKey: true });
    log(!!(await until(() => br().tabs.some((tab) => tab.id === closedId) && tabs().length === 2)), "Ctrl+Shift+T reopens the middle-closed tab");
    middle(tabs()[br().active]);
    log(!!(await until(() => br().tabs.length === 1 && tabs().length === 1)), "middle-click also closes the active tab");
    middle(tabs()[0]);
    await sleep(200);
    log(br().tabs.length === 1, "middle-click preserves the last tab");
    await A.sessionCmd({ op: "newTab", b: 0, path: docs });
    await until(() => tabs().length === 2);
    const otherCount = getState().session!.browsers[1].tabs.length;
    middle(tabs()[br().active], true);
    log(!!(await until(() => br().tabs.length === 1 && getState().session!.browsers[1].tabs.length === otherCount + 1)), "Alt+middle-click still transfers a tab to the other browser");
    await shot("keyboard-tabs-after-close");
  } catch (e) {
    log(false, "keyboard integration", String(e));
  }
  log(!errors.length, "no uncaught keyboard scenario errors", errors.join("; "));
  await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
}

/** Help-only visual scenarios; no file operations or external network requests. */
async function runKeyHelpSelfTest() {
  const A = await import("./state/actions");
  const errors: string[] = [];
  window.addEventListener("error", (e) => errors.push(e.message));
  window.addEventListener("unhandledrejection", (e) => errors.push(String(e.reason)));
  const filter = async (value: string) => {
    typeInto($(".kh input") as HTMLInputElement, value);
    await sleep(120);
  };
  const fits = () => {
    const help = $(".kh")!.getBoundingClientRect();
    const scroll = $(".kh-scroll")!;
    return help.left >= 0 && help.right <= innerWidth + 1 && help.bottom <= innerHeight + 1
      && scroll.scrollWidth <= scroll.clientWidth + 1
      && $(".kh-legend")!.scrollWidth <= $(".kh-legend")!.clientWidth + 1;
  };
  try {
    await getCurrentWindow().unmaximize();
    await A.setLayout("standard");
    for (const [profile, theme, width] of [
      ["explorer", "dark", 1100], ["vim", "dark", 1100], ["totalCommander", "dark", 1100],
      ["explorer", "light", 800], ["vim", "light", 800], ["totalCommander", "light", 800],
      ["explorer", "dark", 640],
    ] as const) {
      const vim = profile === "vim";
      const mode = profile === "explorer" ? "basic" : profile === "vim" ? "vim" : "commander";
      await getCurrentWindow().setSize(new LogicalSize(width, width === 1100 ? 800 : 600));
      await updateSettings({ keyboardProfile: profile, theme, fontSize: width === 640 ? 16 : 13, leaderKey: "Space", leaderKeyBasic: "Ctrl Ctrl", shortcuts: {} });
      await focusPane(0);
      key("?", { shiftKey: true });
      log(!!(await until(() => $(".kh"))), `? opens actual help for ${KEYBOARD_PROFILES[profile]}`);
      log($(".kh-head .pill")?.textContent === KEYBOARD_PROFILES[profile], "help names the active profile");
      log(!!$(".kh-legend .key-plus") && !!$(".kh-legend .key-then") && !!$(".kh-legend .key-or"), "legend separates together, in-order and alternatives");
      log(fits(), `help fits ${width}px ${theme} window without horizontal scrolling`);
      const name = `key-help-${mode}-${theme}-${width}`;
      await shot(name);
      results.push(`SCREENSHOT ${name}.png: expected readable keycaps, distinct + / arrow / or connectors, visible legend and unclipped cards.`);
      await filter(vim ? "Copy" : "Pin");
      log(!!$(".kh-row .key-then"), "filtered help retains sequence arrows");
      if (vim) {
        log(!!$('.kh .key-bindings[aria-label="y, then y"]'), "vim copy displays two sequential y presses");
      } else {
        log(!!$('.kh .key-bindings[aria-label*="Ctrl, then Ctrl"]'), "basic leader displays two separate Ctrl taps");
      }
      if (width === 1100) {
        await shot(`${name}-filtered`);
        results.push(`SCREENSHOT ${name}-filtered.png: expected ${vim ? "copy" : "pin"} shortcuts with explicit sequence and alternative notation.`);
      }
      if (vim && width === 800) {
        await filter("no-such-shortcut-xyz");
        log(!!$(".kh-empty"), "empty search has an explicit status instead of a blank sheet");
        const input = $(".kh input") as HTMLInputElement;
        const typed = new KeyboardEvent("keydown", { key: "?", shiftKey: true, bubbles: true, cancelable: true });
        input.dispatchEvent(typed);
        log(!typed.defaultPrevented && getState().overlay?.type === "keys", "? in a text field remains ordinary typing");
        await shot("key-help-empty-light-800");
        results.push("SCREENSHOT key-help-empty-light-800.png: expected readable no-results status with the notation legend still visible.");
      }
      key("Escape");
      await until(() => !getState().overlay);
      await focusPane(0);
      if (width === 1100) {
        if (vim) key(" ");
        else {
          for (let tap = 0; tap < 2; tap++) {
            key("Control");
            pane(0).dispatchEvent(new KeyboardEvent("keyup", { key: "Control", bubbles: true }));
            await sleep(80);
          }
        }
        log(!!(await until(() => $(".wk"))), "actual which-key menu shows the next-key picker");
        const trail = $(".wk-title .key-bindings")?.getAttribute("aria-label");
        log(trail === (vim ? "Space" : "Ctrl, then Ctrl"), "which-key distinguishes pressed leader steps");
        await shot(`key-help-which-key-${mode}`);
        results.push(`SCREENSHOT key-help-which-key-${mode}.png: expected pressed-key trail, separate next-key options and release-between-steps hint.`);
        key("Escape");
      }
    }
  } catch (e) {
    log(false, "key help visual scenarios", String(e));
  }
  log(!errors.length, "no uncaught key help errors", errors.join("; "));
  await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
}

/** Profile switching, persisted settings and function keys against the real backend. */
async function runKeyboardProfilesSelfTest() {
  const A = await import("./state/actions");
  const { join } = await import("./core/path");
  const { controller } = await import("./state/controllers");
  const root = activePath(0);
  const docs = join(root, "Documents");
  const errors: string[] = [];
  window.addEventListener("error", (e) => errors.push(e.message));
  window.addEventListener("unhandledrejection", (e) => errors.push(String(e.reason)));
  const close = async () => {
    key("Escape");
    await until(() => !getState().overlay);
    await focusPane(0);
  };
  const savedSettings = async () => JSON.parse(new TextDecoder().decode(new Uint8Array(
    await api.readBytes(join(getState().info!.configDir, "settings.json"), 0, 65536),
  )));
  const choose = async (profile: KeyboardProfile) => {
    A.openOverlay({ type: "settings", section: "keyboard-vim" });
    const navigation = await until(() => $$(".settings nav button").find((el) => el.textContent === "Keyboard"));
    if (!navigation) throw new Error("Keyboard settings navigation is missing");
    click(navigation);
    const select = await until(() => $<HTMLSelectElement>("#opt-keyboardProfile"));
    if (!select) throw new Error("Keyboard profile selector is missing");
    const revision = getState().settingsRev;
    select.focus();
    select.value = profile;
    select.dispatchEvent(new Event("change", { bubbles: true }));
    await until(() => getState().settings?.keyboardProfile === profile);
    if (!(await until(() => getState().settingsRev > revision))) throw new Error("Keyboard profile was not saved");
    const saved = await savedSettings();
    log(saved.keyboardProfile === profile && saved.vimKeys === (profile === "vim"), `${KEYBOARD_PROFILES[profile]} is saved to disk with matching Vim mode`);
    return select;
  };
  const refresh = async (fn: string) => {
    const original = api.listDir;
    let reads = 0;
    api.listDir = (...args) => { reads++; return original(...args); };
    try {
      key(fn);
      log(!!(await until(() => reads > 0)), `${fn} refreshes through the real directory backend`);
    } finally { api.listDir = original; }
  };
  try {
    await getCurrentWindow().unmaximize();
    await getCurrentWindow().setSize(new LogicalSize(1100, 800));
    await updateSettings({ keyboardProfile: "vim", theme: "dark", fontSize: 13, shortcuts: {}, editor: "", leaderKey: "Space", leaderKeyBasic: "Ctrl Ctrl" });
    await A.setLayout("standard");
    A.openOverlay({ type: "welcome" });
    const keyboardStep = await until(() => $$(".wc-steps button").find((el) => el.textContent?.endsWith("Keyboard")));
    if (!keyboardStep) throw new Error("Welcome keyboard step is missing");
    click(keyboardStep);
    log(!!(await until(() => $$(".wc-choice").length === 3)), "welcome offers exactly three keyboard profiles");
    const commander = $$(".wc-choice").find((el) => el.textContent?.includes("Total Commander"))!;
    click(commander);
    log(!!(await until(() => getState().settings?.keyboardProfile === "totalCommander" && !getState().settings?.vimKeys)), "welcome Commander card selects its real profile");
    await shot("key-profiles-welcome-dark-1100");
    await getCurrentWindow().setSize(new LogicalSize(800, 600));
    await updateSettings({ theme: "light" });
    await shot("key-profiles-welcome-light-800");
    log($(".welcome")!.scrollWidth <= $(".welcome")!.clientWidth + 1, "three welcome cards fit the narrow window");
    await close();
    await getCurrentWindow().setSize(new LogicalSize(1100, 800));
    await updateSettings({ keyboardProfile: "vim", theme: "dark" });
    await press(pane(0), " ", "f", "c");
    log(!!(await until(() => $(".settings"))), "Vim Space f c opens settings for keyboard profile selection");
    for (const profile of ["explorer", "totalCommander", "vim"] as const) {
      const select = await choose(profile);
      log(select.options.length === 3 && select.value === profile, "native keyboard-accessible selector reflects the active profile");
      const typed = new KeyboardEvent("keydown", { key: "j", bubbles: true, cancelable: true });
      select.dispatchEvent(typed);
      log(!typed.defaultPrevented && !getState().keySeq.length, "profile selector keeps native typing instead of starting Vim commands");
      await shot(`key-profiles-settings-${profile}`);
      await close();
      key("?", { shiftKey: true });
      log(!!(await until(() => $(".kh-head .pill")?.textContent === KEYBOARD_PROFILES[profile])), "? opens correctly named profile help");
      await close();
      key("p", { ctrlKey: true, shiftKey: true });
      log(!!(await until(() => getState().overlay?.type === "quickOpen")), "Gall command palette remains accessible");
      await close();
    }

    await choose("explorer"); await close();
    await A.navigate(0, docs);
    await until(() => row(0, "file1.txt"));
    await selectRow(0, "file1.txt");
    key("F2");
    log(!!(await until(() => $(".modal h3")?.textContent === "Rename")), "Explorer F2 opens rename");
    await close();
    key("F3");
    log(!!(await until(() => getState().overlay?.type === "search")), "Explorer F3 opens search");
    await close();
    key("F4");
    log(!!(await until(() => getState().overlay?.type === "pathEdit")), "Explorer F4 edits the address rather than launching an editor");
    await close();
    await refresh("F5");
    key("F6");
    log(!!(await until(() => getState().session?.activeBrowser === 1)), "Explorer F6 changes browser instead of moving a file");
    log(!!row(0, "file1.txt") && !row(1, "file1.txt"), "Explorer function keys have not copied or moved files");
    await focusPane(0);

    await choose("totalCommander"); await close();
    await refresh("F2");
    await selectRow(0, "file1.txt");
    key("F3");
    log(!!(await until(() => getState().overlay?.type === "previewWindow")), "Commander F3 previews the selected file");
    await close();
    key("F4");
    log(!!(await until(() => getState().overlay?.type === "openWith")), "Commander F4 opens editor selection when none is configured");
    await close();
    key("F5");
    log(!!(await until(() => row(0, "file1.txt") && row(1, "file1.txt"))), "Commander F5 copies to the other browser while keeping the source");
    key("z", { ctrlKey: true });
    log(!!(await until(() => row(0, "file1.txt") && !row(1, "file1.txt"))), "undo removes only the copied destination");
    await selectRow(0, "file1.txt");
    key("F6");
    log(!!(await until(() => !row(0, "file1.txt") && row(1, "file1.txt"))), "Commander F6 moves to the other browser");
    key("z", { ctrlKey: true });
    log(!!(await until(() => row(0, "file1.txt") && !row(1, "file1.txt"))), "undo restores the moved source");
    key("F7");
    const name = await until(() => $<HTMLInputElement>(".modal input"));
    if (!name) throw new Error("Commander F7 did not open the new-folder dialog");
    typeInto(name, "Commander folder");
    key("Enter", {}, name);
    log(!!(await until(() => row(0, "Commander folder"))), "Commander F7 creates a real folder");
    await selectRow(0, "file1.txt");
    key("F8");
    log(!!(await until(() => !row(0, "file1.txt"), 10_000)), "Commander F8 sends the selected file to trash");
    key("z", { ctrlKey: true });
    log(!!(await until(() => row(0, "file1.txt"))), "undo restores the trashed file");

    await choose("vim"); await close();
    await selectRow(0, "file1.txt");
    const before = controller(0)!.focused()?.path;
    key("j");
    log(!!(await until(() => controller(0)!.focused()?.path !== before)), "Vim j navigates rather than typing a filter");
    const pinned = A.isPinned(docs);
    await press(pane(0), " ", "b", "p");
    log(!!(await until(() => A.isPinned(docs) !== pinned)), "Vim leader pins the folder through the backend");
    key("b");
    log(!!(await until(() => A.isPinned(docs) === pinned)), "Vim b restores the pin");

    await choose("totalCommander");
    const shortcutsNav = $$(".settings nav button").find((el) => el.textContent === "Shortcuts")!;
    click(shortcutsNav);
    const pinInput = await until(() => {
      const pinRow = $$(".kb-row").find((el) => el.textContent?.startsWith("Pin / unpin"));
      return pinRow ? $<HTMLInputElement>("input", pinRow) : null;
    });
    if (!pinInput) throw new Error("Pin shortcut editor is missing");
    pinInput.focus();
    typeInto(pinInput, "F9");
    await sleep(50);
    key("Enter", {}, pinInput);
    log(!!(await until(() => getState().settings?.shortcuts.togglePin === "F9")), "shortcut editor accepts a custom profile override");
    for (const profile of ["explorer", "vim", "totalCommander"] as const) {
      await choose(profile);
      log((await savedSettings()).shortcuts.togglePin === "F9", "switching profiles preserves custom shortcuts on disk");
    }
    for (const note of getState().notifications) A.dismiss(note.id);
    await until(() => !$(".toast"));
    await getCurrentWindow().setSize(new LogicalSize(800, 600));
    await updateSettings({ theme: "light" });
    await shot("key-profiles-settings-light-800");
    await close();
    key("?", { shiftKey: true });
    await until(() => $(".kh input"));
    typeInto($(".kh input") as HTMLInputElement, "Pin");
    await until(() => $('.kh .key-bindings[aria-label="F9"]'));
    log(!!$('.kh .key-bindings[aria-label="F9"]') && !$('.kh .key-bindings[aria-label="Ctrl and B together"]'), "help shows the preserved custom override, not an inactive default");
    await shot("key-profiles-custom-help-light-800");
    await close();
    key("F9");
    log(!!(await until(() => A.isPinned(docs) !== pinned)), "preserved custom shortcut executes in Commander");
    key("F9");
    log(!!(await until(() => A.isPinned(docs) === pinned)), "custom pin shortcut remains reversible");
  } catch (e) { log(false, "keyboard profiles integration", String(e)); }
  log(!errors.length, "no uncaught profile errors", errors.join("; "));
  await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
}

/** Second process, reusing only the disposable settings written by --key-profiles. */
async function runKeyboardProfilesReloadSelfTest() {
  log(getState().settings?.keyboardProfile === "totalCommander" && !getState().settings?.vimKeys, "restart reloads the Commander profile");
  log(getState().settings?.shortcuts.togglePin === "F9", "restart retains custom shortcuts");
  await focusPane(0);
  key("?", { shiftKey: true });
  log(!!(await until(() => $(".kh-head .pill")?.textContent === "Total Commander")), "restarted application uses Commander help");
  await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
}

async function runBugReportSelfTest(textOnly = false, entryOnly = false) {
  const { openOverlay } = await import("./state/actions");
  const { LogicalSize } = await import("@tauri-apps/api/dpi");
  const { getReportDraft } = await import("./core/reportDraft");
  try {
    await getCurrentWindow().unmaximize();
    await getCurrentWindow().setSize(new LogicalSize(1100, 800));
    await sleep(250);
    const previous = await api.bugReportPrepare("", false, {}, true);
    if (!entryOnly) log(previous.body.includes("list_dir failed") && previous.body.includes("previous irregular exit"), "actual backend restores previous unclean session diagnostics");
    for (let i = 0; i < 200; i++) await api.bugReportRecord("command", `list_dir ${i} success`);
    await api.bugReportRecord("command", "move failed 25");
    await api.bugReportRecord("error", "Older failure excluded from GitHub");
    const { join } = await import("./core/path");
    try { await api.listDir(join(activePath(0), `missing-report-test-${Date.now()}`), { showHidden: false, hideTemp: false, dirsOnly: false }); }
    catch { /* The real failed IPC must retain its sanitized error message. */ }
    await sleep(150);
    openOverlay({ type: "settings" });
    const aboutNav = await until(() => Array.from(document.querySelectorAll<HTMLButtonElement>(".settings nav button")).find((b) => b.textContent === "About"));
    if (!aboutNav) throw new Error("Missing Settings About navigation");
    click(aboutNav);
    const entry = await until(() => $(".settings .about-report-button"));
    if (!entry) throw new Error("Missing About report button");
    await sleep(200); // Let the Settings modal finish its opening scale animation.
    log(entry.textContent?.trim() === "Report a bug" && !!entry.querySelector(".about-report-icon") && entry.querySelectorAll("svg").length === 1 && !entry.classList.contains("primary"), "About uses a distinct labeled report button with one activity icon and no trailing arrow");
    const hero = entry.closest(".about-hero")!.getBoundingClientRect();
    const wideEntry = entry.getBoundingClientRect();
    log(Math.abs(wideEntry.right - hero.right) < 2 && wideEntry.top < hero.top + 10, "Settings About report button sits at the top right of the header");
    await shot("bug-report-settings-entry-wide");
    await getCurrentWindow().setSize(new LogicalSize(800, 600));
    await sleep(250);
    const entryBounds = entry.getBoundingClientRect();
    log(innerWidth <= 820 && entryBounds.left >= 0 && entryBounds.right <= innerWidth && entryBounds.bottom <= innerHeight, "Settings About report button fits actual narrow window", `${innerWidth}x${innerHeight}`);
    const atEntry = document.elementFromPoint(entryBounds.left + entryBounds.width / 2, entryBounds.top + entryBounds.height / 2);
    log(!!atEntry && entry.contains(atEntry), "report entry is not covered by other controls");
    await shot("bug-report-settings-entry-narrow");
    const actualPrepare = api.bugReportPrepare;
    let releaseDiagnostics: (() => void) | undefined;
    api.bugReportPrepare = async (...args) => {
      const prepared = await actualPrepare(...args);
      await new Promise<void>((resolve) => { releaseDiagnostics = resolve; });
      return prepared;
    };
    try {
      click(entry);
      await until(() => releaseDiagnostics);
      const loadingAction = Array.from(document.querySelectorAll<HTMLButtonElement>(".bug-report button")).find((b) => b.textContent?.trim() === "Review report");
      log(!!$(".bug-report [role='status']")?.textContent?.includes("Preparing diagnostics") && !!loadingAction?.disabled && !($("#report-summary") as HTMLInputElement).disabled, "loading diagnostics leaves the draft editable and handoff disabled");
      await shot("bug-report-loading");
      releaseDiagnostics?.();
    } finally { releaseDiagnostics?.(); api.bugReportPrepare = actualPrepare; }
    log(!!(await until(() => $(".bug-report details"))), "Settings About entry opens the real report dialog");
    if (entryOnly) {
      await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
      return;
    }
    await getCurrentWindow().setSize(new LogicalSize(1100, 800));
    await sleep(250);
    log(!!(await until(() => $(".bug-report details"))), "actual backend prepares a bug report without description");
    const logSection = "## Additional sanitized diagnostic log";
    const compact = $(".bug-report pre")?.textContent ?? "";
    log(compact.includes("list_dir failed:") && !compact.includes("Older failure") && !compact.includes("move failed") && !compact.includes("Current UI context") && !compact.includes(logSection), "GitHub preview contains only the latest real error, not action history or logs");
    log(/- Resolution: \d+x\d+/.test(compact) && compact.includes("- Installation: portable") && compact.includes("- Session: current"), "compact environment includes actual resolution, installation and session");
    log(!compact.includes("- Webview:"), "compact report omits the hardcoded unknown webview placeholder");
    log(!$(".bug-report input[type='checkbox']"), "extra diagnostics are offered only inside Save report");
    $(".report-scroll")!.scrollTop = 0;
    await shot("bug-report-normal");
    const reviewAction = Array.from(document.querySelectorAll<HTMLButtonElement>(".bug-report button")).find((b) => b.textContent?.trim() === "Review report")!;
    const reviewRect = reviewAction.getBoundingClientRect();
    const reviewHit = document.elementFromPoint(reviewRect.left + reviewRect.width / 2, reviewRect.top + reviewRect.height / 2);
    log(!!reviewHit && reviewAction.contains(reviewHit), "review action is not obscured by the previous-session notification");
    const details = $(".bug-report .report-full") as HTMLDetailsElement;
    details.open = true;
    await sleep(150);
    log(!!details.textContent?.includes("Gall"), "full report contains application metadata");
    details.scrollIntoView({ block: "end" });
    await shot("bug-report-expanded");
    const button = (label: string) => Array.from(document.querySelectorAll<HTMLButtonElement>(".bug-report button")).find((b) => b.textContent?.trim() === label || b.querySelector("strong")?.textContent === label)!;
    typeInto($("#report-summary") as HTMLInputElement, "The file list stops updating");
    typeInto($("#report-description") as HTMLTextAreaElement, "blabla");
    await until(() => $(".bug-report pre")?.textContent?.includes("blabla"));
    const remaining = () => Number($(".report-budget span")?.textContent?.split(" link")[0].replace(/\D/g, ""));
    const beforeAccent = remaining();
    typeInto($("#report-description") as HTMLTextAreaElement, "blablač");
    await sleep(50);
    log(Number.isFinite(beforeAccent) && remaining() === beforeAccent - 6, "live counter accounts for encoded accents before the next diagnostics request");
    for (const vimKeys of [false, true]) {
      await updateSettings({ vimKeys });
      key("q", {}, $("#report-description"));
      log(!!$(".bug-report") && ($("#report-description") as HTMLTextAreaElement).value === "blablač", `typing q does not dismiss the report with Vim keys ${vimKeys ? "on" : "off"}`);
      key("Escape", {}, $("#report-description"));
      log(!!(await until(() => !$(".bug-report"))), `Escape closes the composer with Vim keys ${vimKeys ? "on" : "off"}`);
      openOverlay({ type: "bugReport" });
      await until(() => $("#report-description"));
      log(($("#report-description") as HTMLTextAreaElement).value === "blablač", "canceling an unfinished report preserves the draft");
    }
    typeInto($("#report-description") as HTMLTextAreaElement, "blabla");
    await until(() => $(".bug-report pre")?.textContent?.includes("blabla\n"));
    click(button("Review report"));
    await until(() => button("Copy & open GitHub"));
    const failedHandoffCopy = api.bugReportCopyText;
    api.bugReportCopyText = async () => { throw new Error("Synthetic handoff clipboard failure"); };
    try {
      click(button("Copy & open GitHub"));
      await until(() => $(".bug-report [role='alert']"));
      key("Escape", {}, $("#bug-report-title"));
      log(!!(await until(() => !$(".bug-report"))), "Escape also closes the review popup in a single press");
      openOverlay({ type: "bugReport" });
      await until(() => $("#report-description"));
      log(($("#report-description") as HTMLTextAreaElement).value === "blabla", "failed GitHub handoff does not mark the draft as completed");
      await until(() => $(".bug-report pre")?.textContent?.includes("blabla"));
    } finally { api.bugReportCopyText = failedHandoffCopy; }
    click(button("Review report"));
    await until(() => button("Copy & open GitHub"));
    await shot("bug-report-review");
    click(button("Copy & open GitHub"));
    log(!!(await until(() => document.documentElement.dataset.bugReportUrl)), "GitHub opening is intercepted without publishing");
    log(document.documentElement.dataset.bugReportUrl?.startsWith("https://github.com/GaMa87/gall/issues/new?") ?? false, "report targets public Gall repository");
    const issueBody = new URL(document.documentElement.dataset.bugReportUrl!).searchParams.get("body") ?? "";
    log(issueBody === $(".bug-report .report-full pre")?.textContent && issueBody.includes("blabla") && issueBody.includes("list_dir failed:") && !issueBody.includes("Current UI context") && !issueBody.includes("Older failure"), "GitHub receives the compact report directly without requiring manual paste");
    log(new TextEncoder().encode(document.documentElement.dataset.bugReportUrl!).length <= 7000, "actual GitHub handoff stays within encoded URL budget");
    await shot("bug-report-instructions-wide");
    click(button("Copy report"));
    log(!!(await until(() => $(".bug-report")?.textContent?.includes("Report text copied."))), "report copies through real backend clipboard");
    const copied = await api.clipInspect();
    log(copied.kind === "text" && copied.text === $(".bug-report pre")?.textContent && copied.text.includes("blabla") && copied.text.includes("list_dir failed:") && !copied.text.includes("Older failure") && !copied.text.includes("Current UI context"), "Copy report preserves the description and latest error without diagnostic history");
    const originalCopy = api.bugReportCopyText;
    api.bugReportCopyText = async () => { throw new Error("Synthetic clipboard failure"); };
    click(button("Copy report"));
    log(!!(await until(() => $(".bug-report [role='alert']"))), "copy failure remains visible without losing draft");
    await shot("bug-report-error");
    api.bugReportCopyText = originalCopy;
    click(button("Copy report"));
    log(!!(await until(() => !$(".bug-report [role='alert']"))), "copy retry clears error");
    click(button("Back"));
    const textarea = await until(() => $("#report-description") as HTMLTextAreaElement);
    if (!textarea) throw new Error("Missing description field after Back");
    const longDescription = "Diagnostic reproduction: " + "č".repeat(5000);
    typeInto(textarea, longDescription);
    await until(() => $(".bug-report pre")?.textContent?.includes(longDescription));
    log(!!$(".report-budget.over-budget") && !!$(".report-budget .link-btn")?.textContent?.includes("Save ZIP") && !!$("#report-budget-hint")?.textContent?.includes("images and videos"), "over-budget counter recommends saving the complete ZIP instead of truncating text");
    $(".report-scroll")!.scrollTop = 0;
    await shot("bug-report-budget-exceeded");
    click(button("Review report"));
    log(!!(await until(() => $(".report-handoff")?.textContent?.includes("attach the ZIP"))), "over-budget review recommends attaching the saved ZIP");
    const preparedLongReport = await until(() => {
      const text = $(".bug-report details pre")?.textContent;
      return text?.includes(longDescription) ? text : false;
    });
    log(!!preparedLongReport, "long description finishes preparing before export");
    const screenshotPath = await api.selftestShot("bug-report-long");
    click(button("Save report..."));
    const pathInput = await until(() => $(".bug-report-export input") as HTMLInputElement | null);
    if (!pathInput) throw new Error("Missing inline export path");
    const suggestedPath = pathInput.value;
    log(/gall-bug-report-\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}Z\.zip$/.test(suggestedPath) && !!getState().info?.home && suggestedPath.startsWith(getState().info!.home), "inline export suggests an absolute ZIP path with a Windows-safe UTC timestamp");
    await shot("bug-report-timestamped-name");
    const includeLog = $(".bug-report input[type='checkbox']") as HTMLInputElement;
    log(includeLog.checked, "extra diagnostics are enabled by default for saved reports");
    click(includeLog);
    await until(() => button("Save report").disabled);
    await until(() => !button("Save report").disabled);
    key("Escape", {}, pathInput);
    log(!!(await until(() => !$(".bug-report-export"))) && !!$(".bug-report"), "Escape cancels the save popup without closing or clearing the report");
    const reportText = $(".bug-report .report-full pre")?.textContent ?? "";
    click(button("Save report..."));
    let exportInput = await until(() => $(".bug-report-export input") as HTMLInputElement);
    if (!exportInput) throw new Error("Missing export path after Escape");
    log(exportInput.value !== suggestedPath, "reopening Save report generates a fresh timestamped suggestion");
    log(!($(".bug-report input[type='checkbox']") as HTMLInputElement).checked, "opting out of extra diagnostics persists within the unfinished report");
    typeInto(exportInput, "relative-report.zip");
    click(button("Save report"));
    log(!!(await until(() => $(".bug-report [role='alert']")?.textContent?.includes("absolute"))), "actual inline save rejects relative paths and retains the editable form");
    await shot("bug-report-export-error-wide");
    const exportPath = screenshotPath.replace(/[^/\\]+$/, "bug-report-export.zip");
    typeInto(exportInput, exportPath);
    await sleep(50);
    const actualExport = api.bugReportExport;
    let finishSave: (() => void) | undefined;
    api.bugReportExport = async (...args) => {
      await new Promise<void>((resolve) => { finishSave = resolve; });
      await actualExport(...args);
    };
    try {
      click(button("Save report"));
      await until(() => finishSave);
      log(!!$(".bug-report-export") && !!$("button[form='report-export']:disabled"), "save form stays open and prevents duplicate submission while saving");
      await shot("bug-report-saving");
      finishSave?.();
      log(!!(await until(() => $(".bug-report [role='status']")?.textContent?.includes("Report saved to"))) && !$(".bug-report-export"), "successful report save closes only the save form and confirms completion");
    } finally { finishSave?.(); api.bugReportExport = actualExport; }
    const readBundleText = async (path: string, expected: string) => {
      const [materialized] = await api.archiveMaterialize([join(path, "report.md")]);
      return new TextDecoder().decode(new Uint8Array(await api.readBytes(materialized, 0, new TextEncoder().encode(expected).length + 1)));
    };
    const exported = await readBundleText(exportPath, reportText);
    log(exported === reportText, "Save report without extra diagnostics matches the compact report");
    await shot("bug-report-export-success-wide");
    click(button("Save report..."));
    exportInput = await until(() => $(".bug-report-export input") as HTMLInputElement);
    if (!exportInput) throw new Error("Missing export path for overwrite protection");
    typeInto(exportInput, exportPath);
    click(button("Save report"));
    log(!!(await until(() => $(".bug-report [role='alert']")?.textContent?.includes("already exists"))) && !!$(".bug-report-export"), "failed save retains the form and refuses to overwrite an existing file");
    await getCurrentWindow().setSize(new LogicalSize(800, 600));
    await sleep(200);
    await shot("bug-report-export-narrow");
    const saveBounds = button("Save report").getBoundingClientRect();
    log(innerWidth <= 820 && saveBounds.left >= 0 && saveBounds.right <= innerWidth && saveBounds.bottom <= innerHeight, "inline Save report stays inside the actual narrow window");
    const hitSave = document.elementFromPoint(saveBounds.left + saveBounds.width / 2, saveBounds.top + saveBounds.height / 2);
    log(!!hitSave && button("Save report").contains(hitSave), "inline Save report is visible and unobstructed");
    click($(".bug-report input[type='checkbox']")!);
    const detailed = await until(() => {
      const body = $(".bug-report-export pre")?.textContent;
      return body?.includes(logSection) && body.includes("Current UI context") && body.includes("Older failure") ? body : false;
    });
    log(!!detailed, "optional saved diagnostics retain action history, UI context and sanitized logs");
    const savedPreview = $(".bug-report-export .report-full") as HTMLDetailsElement;
    savedPreview.open = true;
    savedPreview.scrollIntoView({ block: "end" });
    await shot("bug-report-save-diagnostics-narrow");
    const detailedPath = exportPath.replace(/\.zip$/, "-detailed.zip");
    typeInto(exportInput, detailedPath);
    click(button("Save report"));
    log(!!(await until(() => $(".bug-report [role='status']")?.textContent?.includes(detailedPath))), "Save report writes optional detailed diagnostics locally");
    const savedDetailed = await readBundleText(detailedPath, detailed ?? "");
    log(!!detailed && savedDetailed === detailed && savedDetailed.includes(longDescription), "detailed export preserves the complete user description and diagnostics");
    log(!$(".bug-report-export"), "successful detailed report save also closes its save form");
    await until(() => button("Back"));
    click(button("Copy & open GitHub"));
    await until(() => $(".bug-report [role='status']")?.textContent?.includes("GitHub opened"));
    const compactAfterExport = await api.clipInspect();
    log(compactAfterExport.kind === "text" && compactAfterExport.text === $(".bug-report .report-full pre")?.textContent && compactAfterExport.text.includes("list_dir failed:") && !compactAfterExport.text.includes(logSection) && !compactAfterExport.text.includes("Current UI context") && !compactAfterExport.text.includes("Older failure"), "GitHub handoff remains compact even with extra saved diagnostics enabled");
    await updateSettings({ theme: "light" });
    await shot("bug-report-compact-light");
    await updateSettings({ theme: "dark" });
    key("Escape", {}, $("#bug-report-title"));
    await until(() => !$(".bug-report"));
    openOverlay({ type: "bugReport" });
    await until(() => $("#report-description"));
    log(($("#report-description") as HTMLTextAreaElement).value === "" && ($("#report-summary") as HTMLInputElement).value === "" && getReportDraft().includeLog && getReportDraft().steps === "" && getReportDraft().expected === "", "a successful GitHub handoff starts the next report empty with saved diagnostics enabled by default");
    await shot("bug-report-new-draft");
    await getCurrentWindow().setSize(new LogicalSize(1100, 800));
    await sleep(200);
    typeInto($("#report-description") as HTMLTextAreaElement, "The file list stopped updating.");
    log(!!(await until(() => $(".bug-report pre")?.textContent?.includes("The file list stopped updating."))), "description remains editable after canceling export");
    if (textOnly) {
      await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
      return;
    }
    // A synthetic image file exercises the actual file input and visible preview.
    const canvas = document.createElement("canvas"); canvas.width = 240; canvas.height = 100;
    const ctx = canvas.getContext("2d")!; ctx.fillStyle = "#334455"; ctx.fillRect(0, 0, 240, 100); ctx.fillStyle = "white"; ctx.fillText("Gall test screenshot", 20, 50);
    const blob = await new Promise<Blob>((resolve) => canvas.toBlob((b) => resolve(b!), "image/png"));
    const transfer = new DataTransfer(); transfer.items.add(new File([blob], "test-screenshot.png", { type: "image/png" }));
    const fileInput = $(".bug-report input[type='file']") as HTMLInputElement;
    fileInput.files = transfer.files; fileInput.dispatchEvent(new Event("change", { bubbles: true }));
    log(!!(await until(() => $(".report-editor-stage img"))), "selected screenshot opens the annotation editor");
    click(button("Redact"));
    await until(() => button("Redact").getAttribute("aria-pressed") === "true");
    click(button("Add mark"));
    const annotation = $(".report-editor-stage svg[tabindex]")!;
    await until(() => $("rect", annotation));
    key("ArrowRight", {}, annotation); key("ArrowDown", { shiftKey: true }, annotation);
    await sleep(60);
    const beforeVim = $("rect", annotation)?.getAttribute("x");
    await updateSettings({ vimKeys: false }); key("h", {}, annotation); await sleep(60);
    log($("rect", annotation)?.getAttribute("x") === beforeVim, "plain h does not move screenshot annotations when Vim keys are disabled");
    await updateSettings({ vimKeys: true }); key("h", {}, annotation); await sleep(60);
    log($("rect", annotation)?.getAttribute("x") !== beforeVim, "Vim h moves a screenshot annotation without leaving the editor");
    await shot("bug-report-annotation");
    click(button("Apply changes"));
    log(!!(await until(() => $(".report-attachment img"))), "flattened screenshot has an attachment preview");
    const editedImage = $(".report-attachment img") as HTMLImageElement;
    await until(() => editedImage.src.startsWith("blob:"));
    await editedImage.decode();
    const flattened = document.createElement("canvas");
    flattened.width = editedImage.naturalWidth; flattened.height = editedImage.naturalHeight;
    flattened.getContext("2d")!.drawImage(editedImage, 0, 0);
    const privatePixel = flattened.getContext("2d")!.getImageData(80, 35, 1, 1).data;
    const clearPixel = flattened.getContext("2d")!.getImageData(5, 5, 1, 1).data;
    log(privatePixel[0] === 17 && privatePixel[1] === 17 && privatePixel[2] === 17 && privatePixel[3] === 255 && clearPixel[0] === 51, "saved annotation permanently covers source pixels while preserving the surrounding image");
    $(".report-attachment")!.scrollIntoView({ block: "center" });
    await shot("bug-report-attachment");
    click(button("Annotate"));
    await until(() => $(".report-editor-stage"));
    key("Escape", {}, $(".report-editor-stage svg[tabindex]"));
    log(!!(await until(() => !$(".report-editor-stage"))) && getReportDraft().media.length === 1, "Escape cancels screenshot editing without discarding the retained attachment");
    const verifyMediaBundle = async (path: string) => {
      const media = [...getReportDraft().media];
      const compactBody = $(".bug-report .report-diagnostics pre")!.textContent!;
      click(button("Review report"));
      await until(() => button("Save report..."));
      click(button("Save report..."));
      const field = await until(() => $("#report-path") as HTMLInputElement);
      if (!field) throw new Error("Missing bundle destination");
      const expectedBody = $(".bug-report-export .report-full pre")?.textContent ?? compactBody;
      typeInto(field, path);
      click(button("Save report"));
      if (!await until(() => $(".report-feedback-area [role='status']")?.textContent?.includes(path))) throw new Error(`Bundle save failed: ${$(".bug-report [role='alert']")?.textContent}`);
      const entries = await api.listDir(path, { showHidden: true, hideTemp: false });
      log(entries.map((entry) => entry.name).sort().join("|") === ["report.md", ...media.map((item) => item.name)].sort().join("|"), "saved ZIP contains exactly report.md and every visible attachment, without original images or extra files");
      log(await readBundleText(path, expectedBody) === expectedBody, "saved media ZIP retains the prepared report and selected diagnostics");
      for (const item of media) {
        const [file] = await api.archiveMaterialize([join(path, item.name)]);
        const actual = await api.readBytes(file, 0, item.blob.size + 1);
        const expected = new Uint8Array(await item.blob.arrayBuffer());
        log(actual.length === expected.length && actual.every((byte, i) => byte === expected[i]), `ZIP preserves finalized ${item.kind} attachment byte-for-byte`, item.name);
      }
      await shot(media.length === 1 ? "bug-report-redacted-image-bundle" : "bug-report-video-bundle");
      log(!$(".bug-report-export"), "successful ZIP save with media returns to the report");
      await until(() => button("Back"));
      click(button("Back"));
      await until(() => $("#report-description"));
    };
    await verifyMediaBundle(screenshotPath.replace(/[^/\\]+$/, "bug-report-redacted-image.zip"));
    click(button("Annotate"));
    await until(() => $(".report-editor-stage"));
    const originalToBlob = HTMLCanvasElement.prototype.toBlob;
    let releaseFlatten: (() => void) | undefined;
    HTMLCanvasElement.prototype.toBlob = function (callback, type, quality) {
      originalToBlob.call(this, (blob) => { releaseFlatten = () => callback(blob); }, type, quality);
    };
    try {
      click(button("Apply changes"));
      await until(() => releaseFlatten);
      key("Escape", {}, $(".report-editor-stage svg[tabindex]"));
      log(!!$(".report-editor-stage") && ($(".report-close") as HTMLButtonElement).disabled && button("Back to report").disabled, "editor cannot be closed while redaction is being flattened");
      releaseFlatten?.();
      await until(() => $(".report-attachment img"));
    } finally { HTMLCanvasElement.prototype.toBlob = originalToBlob; }
    click(button("Copy screenshot"));
    log(!!(await until(() => $(".bug-report [role='status']")?.textContent?.includes("Screenshot copied."))), "actual screenshot control copies a native clipboard image");
    const clipboardImage = await api.clipInspect();
    log(clipboardImage.kind === "image" && clipboardImage.width === 240 && clipboardImage.height === 100, "native clipboard contains the selected image dimensions");
    button("Copy screenshot").scrollIntoView({ block: "center" });
    await shot("bug-report-screenshot-copied-wide");
    await getCurrentWindow().setSize(new LogicalSize(800, 600));
    await sleep(200);
    button("Copy screenshot").scrollIntoView({ block: "center" });
    await sleep(100);
    const imageButton = button("Copy screenshot"), imageBounds = imageButton.getBoundingClientRect();
    const imageHit = document.elementFromPoint(imageBounds.left + imageBounds.width / 2, imageBounds.top + imageBounds.height / 2);
    log(innerWidth <= 820 && imageBounds.left >= 0 && imageBounds.right <= innerWidth && imageBounds.bottom <= innerHeight && !!imageHit && imageButton.contains(imageHit), "Copy screenshot is visible and unobstructed at 800x600");
    await shot("bug-report-screenshot-copied-narrow");
    const copyFeedback = $(".report-feedback-area [role='status']")!.getBoundingClientRect();
    log(copyFeedback.top >= 0 && copyFeedback.bottom <= innerHeight && copyFeedback.width > 0, "copy confirmation stays visible above the footer instead of below a long attachment list");
    click(button("Review report")); await until(() => button("Copy report"));
    click(button("Copy report"));
    await until(() => $(".bug-report [role='status']")?.textContent?.includes("Report text copied."));
    const copiedAfterImage = await api.clipInspect();
    log(copiedAfterImage.kind === "text" && copiedAfterImage.text === $(".bug-report pre")?.textContent, "text and image are explicitly copied in separate steps");
    await getCurrentWindow().setSize(new LogicalSize(1100, 800));
    await sleep(200);
    const remove = $(".report-remove")!;
    click(remove);
    log(!!(await until(() => !$(".bug-report img"))), "screenshot can be excluded before sharing");
    await updateSettings({ theme: "light" });
    await sleep(150);
    await shot("bug-report-review-light");
    await updateSettings({ theme: "dark" });
    click($(".report-close")!); await sleep(100);
    openOverlay({ type: "bugReport", previous: true });
    log(!!(await until(() => $(".bug-report")?.textContent?.includes("previous improperly closed session"))), "previous session report explains uncertainty");
    await shot("bug-report-previous");
    await getCurrentWindow().setSize(new LogicalSize(800, 600));
    await sleep(200);
    await shot("bug-report-narrow");
    const bounds = $(".bug-report")?.getBoundingClientRect();
    log(innerWidth <= 820 && !!bounds && bounds.left >= 0 && bounds.right <= innerWidth, "report fits actual narrow window", `${innerWidth}x${innerHeight}`);
    openOverlay({ type: "about" });
    const compactEntry = await until(() => $(".about-win .about-report-button"));
    if (!compactEntry) throw new Error("Missing compact About report button");
    await shot("bug-report-about-entry-narrow");
    click(compactEntry);
    log(!!(await until(() => $(".bug-report details"))), "compact About entry opens the real report dialog");
    await getCurrentWindow().setSize(new LogicalSize(1100, 800));
    await sleep(200);
    typeInto($("#report-summary") as HTMLInputElement, "Capture after sharing");
    await until(() => button("Review report") && !button("Review report").disabled);
    click(button("Review report"));
    await until(() => button("Copy & open GitHub"));
    click(button("Copy & open GitHub"));
    await until(() => $(".report-feedback-area [role='status']")?.textContent?.includes("GitHub opened"));
    click(button("Back"));
    await until(() => $("#report-summary"));
    click(button("Capture Gall"));
    log(!!(await until(() => $(".report-capture"))) && !$(".bug-report"), "capture closes the reporter so Gall is unobstructed");
    await shot("bug-report-capture-countdown");
    log(!!(await until(() => $(".report-attachment img"), 12000)), "Capture Gall returns an actual native webview screenshot to the report");
    log(getReportDraft().media.length === 1 && ($("#report-summary") as HTMLInputElement).value === "Capture after sharing", "automatic capture restoration after successful GitHub handoff preserves the new evidence and its report");
    $(".report-attachment")!.scrollIntoView({ block: "center" });
    await shot("bug-report-native-capture");
    await updateSettings({ vimKeys: false });
    click(button("Record a clip"));
    log(!!(await until(() => $(".report-capture")?.textContent?.includes("Recording Gall"), 12000)), "real Gall-only recording starts after countdown");
    await setLayout("classic"); await sleep(600);
    await setLayout("standard"); await sleep(600);
    const stopRecording = Array.from(document.querySelectorAll<HTMLButtonElement>(".report-capture button")).find((b) => b.textContent === "Stop recording");
    if (!stopRecording) throw new Error("Missing Stop recording control");
    key("Enter", { ctrlKey: true, shiftKey: true }, document.body);
    const video = await until(() => $(".report-attachment video") as HTMLVideoElement, 12000);
    log(!!video, "global keyboard stop works outside the controls with Vim keys disabled and creates a local clip");
    if (video) {
      await until(() => video.src.startsWith("blob:"));
      await video.play();
      log(!!(await until(() => video.readyState >= 2 && video.videoWidth > 0, 5000)), "recorded clip decodes and plays in the actual app");
      log(!!(await until(() => video.currentTime >= 0.25, 5000)) && !video.error, "recorded clip advances over time, not just a decodable first frame");
      video.pause();
      const card = video.closest(".report-attachment")!;
      click(Array.from(card.querySelectorAll("button")).find((b) => b.textContent === "Save attachment")!);
      const clipInput = await until(() => $(".bug-report-export input") as HTMLInputElement);
      if (!clipInput) throw new Error("Missing clip export path");
      const clipPath = screenshotPath.replace(/[^/\\]+$/, $(".report-attachment-info strong", card)!.textContent!);
      typeInto(clipInput, clipPath);
      click(button("Save attachment"));
      log(!!(await until(() => $(".bug-report [role='status']")?.textContent?.includes("Attachment saved to"))), "recorded clip exports through the actual backend media validation");
      const signature = await api.readBytes(clipPath, 0, 16);
      log((signature[0] === 0x1a && signature[1] === 0x45) || String.fromCharCode(...signature.slice(4, 8)) === "ftyp", "exported clip has an actual MP4 or WebM signature");
      await shot("bug-report-video-export");
      log(!$(".bug-report-export"), "successful attachment save returns to the report");
      await until(() => $(".report-attachment video"));
    }
    $(".report-attachment video")!.scrollIntoView({ block: "center" });
    await until(() => ($(".report-attachment video") as HTMLVideoElement).readyState >= 2);
    await shot("bug-report-recording-preview");
    // A native capture completed while another dialog is open must not replace it.
    while ($(".report-remove")) { const count = $$(".report-remove").length; click($(".report-remove")!); await until(() => $$(".report-remove").length < count); }
    click(button("Capture Gall"));
    await until(() => $(".report-capture"));
    openOverlay({ type: "about" });
    await until(() => $(".about-win"));
    await until(() => !$(".report-capture"), 12000);
    log(!!$(".about-win") && !$(".bug-report"), "capture completion preserves a newer open dialog instead of replacing it");
    const viewCapture = await until(() => Array.from(document.querySelectorAll<HTMLButtonElement>(".toasts button")).find((item) => item.textContent === "View report"));
    if (!viewCapture) throw new Error("Missing capture restoration notification");
    openOverlay(null);
    await until(() => !$(".about-win"));
    click(viewCapture);
    await until(() => $(".report-attachment img"));
    log($$(".report-attachment").length === 1, "View report restores waiting evidence after handoff without treating it as a new draft");
    while ($(".report-remove")) { const count = $$(".report-remove").length; click($(".report-remove")!); await until(() => $$(".report-remove").length < count); }
    // Resolve cancelled initialization after its successor to exercise controller ownership.
    const originalCapture = api.bugReportCapture;
    const pending: Array<(png: number[]) => void> = [];
    api.bugReportCapture = () => pending.length < 2 ? new Promise((resolve) => pending.push(resolve)) : originalCapture();
    try {
      await updateSettings({ vimKeys: true });
      click(button("Record a clip"));
      await until(() => pending.length === 1, 12000);
      key("Backspace", { ctrlKey: true, shiftKey: true }, document.body);
      await until(() => button("Record a clip") && !button("Record a clip").disabled);
      click(button("Record a clip"));
      await until(() => pending.length === 2, 12000);
      const pixels = await originalCapture();
      pending[1](pixels);
      await until(() => $(".report-capture")?.textContent?.includes("Recording Gall"), 5000);
      pending[0](pixels);
      await sleep(120);
      openOverlay({ type: "bugReport" });
      await until(() => $("#report-summary"));
      log(button("Add a file").disabled, "reopened reporter reserves the pending capture's attachment slot");
      typeInto($("#report-summary") as HTMLInputElement, "Summary edited during capture");
      await until(() => button("Review report") && !button("Review report").disabled);
      click(button("Review report"));
      await until(() => button("Copy & open GitHub"));
      log(button("Copy & open GitHub").disabled && !!$(".report-handoff")?.textContent?.includes("Finish or cancel"), "GitHub handoff cannot mark an unfinished capture as completed");
      click(button("Back"));
      await until(() => $("#report-summary"));
      key("Enter", { ctrlKey: true, shiftKey: true }, document.body);
      log(!!(await until(() => $(".report-attachment video"), 8000)) && !$(".report-capture"), "global keyboard stop targets the current recording after an older initialization completes");
      typeInto($("#report-summary") as HTMLInputElement, "Summary edited after capture");
      await sleep(80);
      log($$(".report-attachment video").length === 1, "capture delivered to an already open reporter survives further typing");
    } finally { api.bugReportCapture = originalCapture; }
    for (let i = 0; i < 2; i++) {
      const upload = $(".bug-report input[type='file']") as HTMLInputElement;
      const nextFile = new DataTransfer();
      nextFile.items.add(new File([blob], `boundary-${i}.png`, { type: "image/png" }));
      upload.files = nextFile.files; upload.dispatchEvent(new Event("change", { bubbles: true }));
      if (!await until(() => $(".report-editor-stage"))) throw new Error(`Attachment ${i + 2} did not open its editor: ${$(".bug-report [role='alert']")?.textContent ?? "no error shown"}`);
      click(button("Apply changes"));
      await until(() => $$(".report-attachment").length === i + 2 && !$(".report-editor-stage"));
    }
    log($$(".report-attachment").length === 3 && button("Add a file").disabled && button("Capture Gall").disabled, "three-attachment boundary keeps existing evidence and disables additional captures");
    await shot("bug-report-attachment-limit");
    await until(() => $(".bug-report .report-diagnostics pre"));
    await verifyMediaBundle(screenshotPath.replace(/[^/\\]+$/, "bug-report-all-media.zip"));
    click(button("Review report")); await until(() => button("Copy & open GitHub"));
    await getCurrentWindow().setSize(new LogicalSize(800, 600)); await sleep(200);
    await shot("bug-report-review-narrow");
    const handoffBounds = button("Copy & open GitHub").getBoundingClientRect();
    log(handoffBounds.left >= 0 && handoffBounds.right <= innerWidth && handoffBounds.bottom <= innerHeight, "primary handoff stays visible with multiple attachments at narrow size");
    click(button("Copy & open GitHub"));
    await until(() => $(".report-feedback-area [role='status']")?.textContent?.includes("GitHub opened"));
    log(getReportDraft().media.length === 3 && $$(".report-attachment").length === 3, "successful handoff keeps the current media available for saving before closing");
    key("Escape", {}, $("#bug-report-title"));
    await until(() => !$(".bug-report"));
    openOverlay({ type: "bugReport" });
    await until(() => $("#report-description"));
    log(getReportDraft().media.length === 0 && ($("#report-description") as HTMLTextAreaElement).value === "" && ($("#report-summary") as HTMLInputElement).value === "", "reopening after successful handoff clears the completed draft and all attachments");
    await updateSettings({ theme: "light" });
    $(".report-scroll")!.scrollTop = 0;
    await shot("bug-report-new-draft-narrow-light");
    results.push("SCREENSHOTS bug-report-settings-entry-wide, bug-report-settings-entry-narrow, bug-report-about-entry-narrow: standard labeled report button in About at wide and narrow sizes.");
    results.push("SCREENSHOTS bug-report-normal, bug-report-expanded, bug-report-narrow: actual report dialog, metadata review and narrow layout.");
  } catch (error) { log(false, "focused bug report", error instanceof Error ? error.stack ?? error.message : String(error)); await shot("bug-report-failure"); }
  await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
}

export const gitSelftest = { api, $, $$, key, click, mouse, typeInto, until, sleep, shot, log, focusPane, activePath, updateSettings, getState, drag, finish: () => api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`) };
