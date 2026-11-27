// UI self-test (debug builds, GALL_SELFTEST=1). Drives the real UI with
// synthetic DOM events against a disposable test folder, takes screenshots and
// writes a report. Start: browser 1 = <testdata>, browser 2 = <testdata>/Projects.
import { api } from "./api/ipc";
import { getState } from "./state/store";
import { activePath, setLayout, updateSettings } from "./state/actions";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getCurrentWebview } from "@tauri-apps/api/webview";
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

function unobstructed(el: Element | null) {
  if (!el) return false;
  const { left, top, right, bottom, width, height } = el.getBoundingClientRect();
  if (width < 10 || height < 10 || left < 0 || top < 0 || right > innerWidth || bottom > innerHeight) return false;
  const hit = (x: number, y: number) => el.contains(document.elementFromPoint(x, y));
  return hit(left + 5, top + 5) && hit(right - 5, top + 5) &&
    hit(left + 5, bottom - 5) && hit(right - 5, bottom - 5) && hit(left + width / 2, top + height / 2);
}

function mouse(el: Element, type: string, opts: MouseEventInit = {}) {
  el.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, view: window, ...center(el), ...opts }));
}

function click(el: Element, opts: MouseEventInit = {}) {
  mouse(el, "mousedown", opts);
  mouse(el, "mouseup", opts);
  // A mouse click has detail 1; 0 is a click made with the keyboard.
  mouse(el, "click", { detail: 1, ...opts });
}

function key(k: string, opts: KeyboardEventInit = {}, target: Element | null = document.activeElement) {
  const code = k.length === 1 && /[a-z]/i.test(k) ? "Key" + k.toUpperCase() : k;
  (target ?? document.body).dispatchEvent(new KeyboardEvent("keydown", { key: k, code, bubbles: true, cancelable: true, ...opts }));
}

async function withoutTypeGroups(check: () => Promise<void>) {
  const group = getState().settings!.group;
  await updateSettings({ group: "folders" });
  try {
    await check();
  } finally {
    await updateSettings({ group });
  }
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
  if (getState().info?.platform !== "macos") {
    await getCurrentWindow().setFocus().catch(() => {});
  }
  await sleep(250);
  await api.selftestShot(name).catch((e) => log(false, `screenshot ${name}`, String(e)));
}

async function setWindowSize(width: number, height: number) {
  await api.selftestResize(width, height).catch(() => getCurrentWindow().setSize(new LogicalSize(width, height)).catch(() => {}));
  await until(() => Math.abs(window.innerWidth - width) <= 16 && Math.abs(window.innerHeight - height) <= 16, 400);
  await sleep(80);
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
    await setWindowSize(1100, 800);
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
    await setWindowSize(800, 600);
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
    await setWindowSize(1100, 800);
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

/** Uses an existing Windows mapping; the runner never creates or deletes shares. */
async function runMappedDrivesSelfTest() {
  const A = await import("./state/actions");
  const { controller } = await import("./state/controllers");
  try {
    // The local fixture's last component is the expected mapped letter. Startup
    // paths may canonicalize mapped drives to UNC, losing the letter under test.
    const local = activePath(0);
    const letter = local.split(/[/\\]/).pop() ?? "";
    if (!/^[a-z]$/i.test(letter)) throw new Error(`Expected a letter-named local fixture, got ${local}`);
    const root = `${letter.toUpperCase()}:\\`;
    const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();
    await A.refreshDrives();
    const matches = getState().drives.filter((d) => same(d.mount, root));
    log(matches.length === 1, "mapped drive returned exactly once through IPC", root);
    log(matches[0]?.label === "Network" && !matches[0]?.removable, "network row is identified and has no eject action");
    const driveRow = () => $$(".sidebar .drive").find((e) => same($("b", e)?.textContent ?? "", root.slice(0, 2)));
    log(!!await until(driveRow), "mapped drive appears in sidebar");
    for (const profile of ["explorer", "vim"] as const) {
      await updateSettings({ keyboardProfile: profile, showHidden: true, hideTemp: false });
      A.navigate(0, local);
      if (!await until(() => same(activePath(0), local) && same(controller(0)?.path() ?? "", local))) throw new Error("Cannot return to local fixture");
      await focusPane(0);
      const expected = (await api.listDir(root, { showHidden: true, hideTemp: false })).map((e) => e.name).sort();
      const item = driveRow();
      if (!item) throw new Error("Mapped drive row is missing");
      click(item);
      const loaded = await until(() => {
        const ctl = controller(0);
        if (!same(activePath(0), root) || !same(ctl?.path() ?? "", root)) return false;
        const actual = ctl?.entries().map((e) => e.name).sort() ?? [];
        const listingVisible = actual.length > 0 || !!$(".fp-list .empty-msg:not(.error)", browserEl(0)!);
        return listingVisible && JSON.stringify(actual) === JSON.stringify(expected);
      }, 15000);
      log(!!loaded, `sidebar opens mapped drive and renders its listing (${profile})`);
      log(!$(".fp-list .error", browserEl(0)!), `mapped folder has no listing error (${profile})`);
      log(driveRow()?.classList.contains("current") ?? false, `mapped drive is highlighted (${profile})`);
      await shot(`mapped-drive-${profile}`);
      await A.refreshDrives();
      log(getState().drives.filter((d) => same(d.mount, root)).length === 1, `refresh preserves one mapped row (${profile})`);
    }
  } catch (error) {
    log(false, "mapped drive integration", String(error));
    await shot("mapped-drive-error");
  }
  await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
}

async function runLinuxLiveSelfTest() {
  const root = activePath(0);
  const info = await api.appInfo();
  const trashRoot = `${root.slice(0, -3)}/data/Trash`;
  if (info.platform !== "linux" || !/^(?:\/[^/]+)*\/gall-selftest-[^/]+\/td$/.test(root) ||
      info.trashDir !== `${trashRoot}/files`) {
    log(false, "Linux live checks have an isolated Trash fixture");
    return api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
  }

  await step("isolated Linux Trash", async () => {
    const name = "gall-trash-probe.txt";
    const file = `${root}/${name}`;
    const id = `${trashRoot}/info/${name}.trashinfo`;
    const list = async () => {
      const entries = await api.trashList();
      if (entries.some((entry) => entry.id !== id)) throw new Error("Trash contains entries outside the test fixture");
      return entries;
    };
    if ((await list()).length !== 0 || (await api.readTextFile(file)).text !== "hello") {
      throw new Error("Expected an empty private Trash and the original five-byte fixture");
    }
    await api.trash([file]);
    const [entry] = await list();
    if (!entry || entry.name !== name || entry.originalParent !== root || entry.isDir || entry.size !== 5) {
      throw new Error("Trashed fixture metadata did not match the original file");
    }
    log(!(await api.pathResolve(file)).exists, "Trash lists the deleted file with its original path and five-byte size", JSON.stringify(entry));
    await api.trashRestore([id]);
    log((await api.readTextFile(file)).text === "hello" && (await list()).length === 0,
      "Restore returns the original bytes and removes the Trash entry");
    await api.trash([file]);
    if ((await list())[0]?.id !== id) throw new Error("The fixture is missing after the second trash");
    await api.trashPurge([id]);
    log((await list()).length === 0 && !(await api.pathResolve(file)).exists,
      "Purging the selected fixture removes its Trash entry without restoring it");
  });

  await step("live Linux volumes", async () => {
    type BlockDevice = { path: string; kname: string; pkname: string | null; fstype: string | null;
      mountpoint: string | null; mountpoints: (string | null)[] | null };
    const { blockdevices: host } = JSON.parse((await api.readTextFile(`${root}/live-block-devices.json`)).text) as { blockdevices: BlockDevice[] };
    const [volumes, drives] = await Promise.all([api.otherVolumes(), api.listDrives()]);
    const byPath = new Map(host.map((d) => [d.path, d]));
    const byName = new Map(host.map((d) => [d.kname, d]));
    const mounted = (d: BlockDevice) => !!d.mountpoint || !!d.mountpoints?.some(Boolean);
    const unmountedFilesystems = host.filter((d) => d.fstype && !mounted(d) && !host.some((child) => child.pkname === d.kname));
    const lockable = drives.filter((d) => d.lockable).map((d) => d.mount).sort();
    const hostLockable = host.flatMap((d) => {
      const encrypted = d.pkname && byName.get(d.pkname)?.fstype;
      return encrypted && ["crypto_LUKS", "BitLocker"].includes(encrypted) &&
        d.mountpoint && /^\/(run\/media|media|mnt)\//.test(d.mountpoint) ? [d.mountpoint] : [];
    }).sort();
    results.push(`Linux block devices: ${JSON.stringify(host)}`);
    results.push(`Mounted drives: ${JSON.stringify(drives.map(({ mount, fs, lockable }) => ({ mount, fs, lockable })))}`);
    log(volumes.every((v) => {
      const d = byPath.get(v.id);
      return !!d && !mounted(d) && d.fstype === v.fs;
    }) && (unmountedFilesystems.length !== 0 || volumes.length === 0),
    "Other Volumes entries match unmounted Linux block devices", JSON.stringify(volumes));
    log(JSON.stringify(lockable) === JSON.stringify(hostLockable), "Lockable mounts match encrypted removable mount locations", JSON.stringify(lockable));
  });

  await step("live Linux GPU", async () => {
    const cards = await api.listDir("/sys/class/drm", { showHidden: false, hideTemp: false });
    const paths = cards.filter((entry) => /^card\d+$/.test(entry.name)).map((entry) => entry.path);
    const read = async (path: string) => {
      const text = (await api.readTextFile(path).catch(() => null))?.text.trim();
      return text && /^\d+(?:\.\d+)?$/.test(text) ? Number(text) : null;
    };
    const counters = () => Promise.all(paths.map(async (path) => {
      const [busy, used, total] = await Promise.all(["gpu_busy_percent", "mem_info_vram_used", "mem_info_vram_total"]
        .map((name) => read(`${path}/device/${name}`)));
      return { path, busy, used, total };
    }));
    for (let i = 0; i < 3; i++) {
      if (i > 0) await sleep(1000);
      const before = await counters();
      const { gpu, gpuMemUsed, gpuMemTotal, gpuTemp } = await api.systemStats();
      const after = await counters();
      const readable = [...before, ...after];
      const hasBusy = readable.some(({ busy }) => busy !== null);
      const largestVram = Math.max(0, ...readable.map(({ used, total }) => used === null ? 0 : total ?? 0));
      // Busy and used memory can change between reads. Compare their availability
      // and range, and require the stable total VRAM counter to match exactly.
      const busyValid = gpu === null ? !hasBusy : hasBusy && Number.isFinite(gpu) && gpu >= 0 && gpu <= 100;
      const memoryValid = largestVram === 0
        ? gpuMemTotal === null && gpuMemUsed === null
        : gpuMemTotal === largestVram && gpuMemUsed !== null && gpuMemUsed >= 0 && gpuMemUsed <= largestVram;
      log(busyValid && memoryValid && (gpuTemp === null || Number.isFinite(gpuTemp)),
        `GPU sample ${i + 1} matches available Linux counters`,
        JSON.stringify({ before, app: { gpu, gpuMemUsed, gpuMemTotal, gpuTemp }, after }));
    }
  });
  await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
}

export async function runSelfTest() {
  // "<mode>@<scale>" (git-workspace@1.5) runs a mode at a display scale derived from the display's own scale.
  const [mode, scaleText] = (await api.startupArgs()).selftestMode.split("@");
  // WKWebView on macOS shares localStorage per origin across portable runs, so clear
  // webview-persisted UI preferences (diff mode/options, pane widths, etc.) before each suite.
  if (mode !== "key-profiles-reload" && mode !== "updates") {
    try { localStorage.clear(); } catch { /* ignore */ }
  }
  const scale = Number(scaleText);
  if (scaleText && scale > 0) {
    const zoom = scale / window.devicePixelRatio;
    (window as unknown as { __selftestZoom?: number }).__selftestZoom = zoom;
    await getCurrentWebview().setZoom(zoom).catch(() => {});
    await sleep(300);
  }
  if (mode.startsWith("e2e-")) return (await import("./selftestE2e")).runE2eSelfTest(mode, gitSelftest);
  if (mode === "mapped-drives") return runMappedDrivesSelfTest();
  if (mode === "diff-permission") {
    const { runDiffPermissionSelfTest } = await import("./selftestDiffPermission");
    await runDiffPermissionSelfTest(gitSelftest, activePath(0));
    await gitSelftest.finish();
    return;
  }
  if (mode === "themed-select") return (await import("./selftestThemedSelect")).runThemedSelectSelfTest(gitSelftest);
  if (mode === "git-rebase-submodules") return (await import("./selftestGitWorkspace")).runGitRebaseSubmodulesSelfTest(gitSelftest);
  if (mode === "git-compare") return runGitCompareSelfTest();
  if (mode === "structural-diff") return runStructuralDiffSelfTest();
  if (mode === "git-baseline") return (await import("./selftestGitBaseline")).runGitBaselineSelfTest(gitSelftest);
  if (mode === "file-groups") return (await import("./selftestFileGroups")).runFileGroupsSelfTest(gitSelftest);
  if (mode === "standalone-merge") {
    const { runMergeWorkspaceSelfTest } = await import("./selftestStandaloneMerge");
    await runMergeWorkspaceSelfTest(gitSelftest, activePath(0));
    await gitSelftest.finish();
    return;
  }
  if (mode === "merge-ai") {
    const { runMergeAiSelfTest } = await import("./selftestMergeAi");
    await runMergeAiSelfTest(gitSelftest, activePath(0));
    await gitSelftest.finish();
    return;
  }
  if (mode === "patch-import") {
    const { runPatchImportSelfTest } = await import("./selftestPatchImport");
    await runPatchImportSelfTest(gitSelftest, activePath(0));
    await gitSelftest.finish();
    return;
  }
  if (mode === "rich-formats") {
    const { runRichFormatsSelfTest } = await import("./selftestRichFormats");
    await runRichFormatsSelfTest(gitSelftest, activePath(0));
    await gitSelftest.finish();
    return;
  }
  if (mode === "doc-compare") {
    const { runDocCompareSelfTest } = await import("./selftestDocCompare");
    await runDocCompareSelfTest(gitSelftest, activePath(0));
    await gitSelftest.finish();
    return;
  }
  if (mode === "git-scrubber") {
    const { runGitScrubberSelfTest } = await import("./selftestGitScrubber");
    await runGitScrubberSelfTest(gitSelftest, activePath(0));
    await gitSelftest.finish();
    return;
  }
  if (mode === "compare-tabs") {
    const { runCompareTabsSelfTest } = await import("./selftestCompareTabs");
    await runCompareTabsSelfTest(gitSelftest, activePath(0));
    await gitSelftest.finish();
    return;
  }
  if (mode === "compare-ai") {
    const { runCompareAiSelfTest } = await import("./selftestCompareAi");
    await runCompareAiSelfTest(gitSelftest, activePath(0));
    await gitSelftest.finish();
    return;
  }
  if (mode === "outline-generated") {
    const { runOutlineGeneratedSelfTest } = await import("./selftestOutlineGenerated");
    await runOutlineGeneratedSelfTest(gitSelftest, activePath(0));
    await gitSelftest.finish();
    return;
  }
  if (mode === "git-workspace") return (await import("./selftestGitWorkspace")).runGitWorkspaceSelfTest(gitSelftest);
  if (mode === "popup-placement") return (await import("./selftestPopups")).runPopupPlacementSelfTest(gitSelftest);
  if (mode === "preview-shortcuts") return runPreviewShortcutsSelfTest();
  if (mode === "quick-open") return runQuickOpenSelfTest();
  if (mode === "search-hidden") return runSearchHiddenSelfTest();
  if (mode === "key-profiles") return runKeyboardProfilesSelfTest();
  if (mode === "key-profiles-reload") return runKeyboardProfilesReloadSelfTest();
  if (mode === "key-help") return runKeyHelpSelfTest();
  if (mode === "notifications") return runNotificationSelfTest();
  if (mode === "linux-live") return runLinuxLiveSelfTest();
  if (mode === "titlebar") return runTitleBarSelfTest();
  if (mode === "settings-sections") return runSettingsSectionsSelfTest();
  if (mode === "window-overlays") return runWindowOverlaysSelfTest();
  if (mode === "keyboard") return runKeyboardSelfTest();
  if (mode === "bug-report-entry") return runBugReportSelfTest(false, true);
  if (mode === "bug-report") return runBugReportSelfTest();
  if (mode === "bug-report-text") return runBugReportSelfTest(true);
  if (mode === "updates") return runUpdateSelfTest();
  if (mode === "i18n") return (await import("./selftestI18n")).runI18nSelfTest(gitSelftest);
  if (mode === "rendering") return (await import("./selftestRendering")).runRenderingSelfTest(gitSelftest);
  if (mode === "omarchy") return (await import("./selftestOmarchy")).runOmarchySelfTest(gitSelftest);
  if (mode === "adaptive-layout") return (await import("./selftestOmarchy")).runAdaptiveLayoutSelfTest(gitSelftest);
  if (mode === "workspaces-pairs") return (await import("./selftestWorkspacesPairs")).runWorkspacesPairsSelfTest(gitSelftest);
  const errors: string[] = [];
  window.addEventListener("error", (e) => errors.push(e.message));
  window.addEventListener("unhandledrejection", (e) => errors.push(String(e.reason)));
  const root = activePath(0);
  results.push(`root: ${root}`);
  // Make sure we are on screen and at a normal dual-pane width (tiling compositors
  // such as Hyprland can hand out a half-screen tile before auto-floating takes over).
  await getCurrentWindow().setFocus().catch(() => {});
  await getCurrentWindow().unmaximize().catch(() => {});
  if (window.innerWidth < 1200 || window.innerHeight < 760) {
    await setWindowSize(1366, 800);
  }
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

  // These scenarios exercise name ordering and Miller columns explicitly; the
  // first-start defaults are date ordering and breadcrumbs.
  await updateSettings({ defaultSort: "name", defaultDescending: false, showNavColumns: true });

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
    const { batchRename, dismiss, notify, undo } = await import("./state/actions");
    const dir = root + "/Rename";
    const list = async () => (await api.listDir(dir, { showHidden: false, hideTemp: false })).map((e) => e.name).sort().join(",");
    const entries = (await api.listDir(dir, { showHidden: false, hideTemp: false })).sort((a, b) => a.name.localeCompare(b.name));
    batchRename(0, entries);
    const win = await until(() => $(".brn-win"), 3000);
    log(!!win, "F2 on several items opens the batch rename window");
    const previewNames = () => $$(".brn-row .new").map((e) => e.textContent).join(",");
    const fillRename = async () => {
      typeInto($<HTMLInputElement>(".brn-field input")!, "Trip {n:2}");
      $<HTMLElement>('.brn-opts .seg button:nth-child(2)')?.click(); // lower case
      return until(() => previewNames() === "trip 01.HEIC,trip 02.HEIC,trip 03.HEIC");
    };
    log(!!(await fillRename()), "preview shows the new names", previewNames());
    const startSize = { width: innerWidth, height: innerHeight };
    let actionRan = false;
    const noteIds = [
      notify("Restored 3 files to a folder with a long name", "success"),
      notify("Moved 3 files to the trash", "success"),
      notify("Copied 3 files to a folder with a long name", "success", { label: "Undo", run: () => { actionRan = true; } }),
    ];
    const footerButtons = () => $$(".brn-foot button");
    const footerReachable = () => footerButtons().length === 2 && footerButtons().every(unobstructed);
    try {
      const stacked = await until(() => $$(".toasts .toast").length >= 3);
      log(!!stacked && footerReachable(), "batch rename actions are unobstructed with stacked notifications");
      const backdrop = $(".brn-win")?.parentElement;
      log(!!backdrop && document.elementFromPoint(2, innerHeight - 2) === backdrop,
        "the modal backdrop still blocks the workspace beside notifications");
      await shot("45-batch-rename");
      const cancel = $(".brn-foot .btn:not(.primary)");
      if (cancel && unobstructed(cancel)) click(cancel);
      log(!!(await until(() => !$(".brn-win"))), "Cancel closes batch rename with stacked notifications");
      const action = await until(() => {
        const toast = $$(".toasts .toast").find((el) => el.textContent?.includes("Copied 3 files to a folder with a long name"));
        const button = toast && $("button", toast);
        return button && unobstructed(button) && button;
      });
      if (action) click(action);
      log(actionRan, "a retained notification action works after closing batch rename");
      await setWindowSize(640, 600);
      batchRename(0, entries);
      await until(() => $(".brn-win"));
      await fillRename();
      log(innerWidth <= 656 && $$(".toasts .toast").length >= 2 && footerReachable(),
        "both batch rename actions are unobstructed in a narrow window with stacked notifications");
      await shot("45-batch-rename-narrow");
      const apply = $<HTMLButtonElement>(".brn-foot .btn.primary");
      if (apply && !apply.disabled && unobstructed(apply)) click(apply);
    } finally {
      for (const id of noteIds) dismiss(id);
      await setWindowSize(startSize.width, startSize.height);
    }
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
    const size = (name: string) => row(0, name)?.querySelector(".fp-size-col")?.textContent?.trim() ?? "";
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
      // From the parent: the changed subfolder is only listed, so no watcher reports it.
      const { trashPaths, undo } = await import("./state/actions");
      await trashPaths([root + "/Sizes/small/a.bin"]);
      const listedTrashed = await until(() => size("small") === "0 B", 5000);
      log(!!listedTrashed, "automatic size of a listed subfolder refreshes after moving its contents to the trash", `small: ${size("small")}`);
      await undo();
      const listedUndone = await until(() => size("small") === "300 B", 5000);
      log(!!listedUndone, "automatic size of a listed subfolder refreshes after undo", `small: ${size("small")}`);
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
    // Extension groups can put the JPG before the PNG even with name sort.
    const images = rowNames(0);
    const [first, last] = images;
    const widths: Record<string, number> = { "plasma.png": 300, "sunset.jpg": 640 };
    if (images.length !== 2 || !widths[first] || !widths[last] || first === last) {
      return log(false, "two previewable images are listed", images.join(", "));
    }
    await selectRow(0, last);
    await focusPane(0);
    key(" ", {}, pane(0));
    const pw = await until(() => $<HTMLElement>(".pw"));
    log(!!pw, "Space opens preview window");
    if (!pw) return;
    pw.focus();
    const showing = (name: string) => getState().focused[0]?.name === name && row(0, name)?.classList.contains("selected") &&
      $(".pw .ph .t")?.textContent === name && $<HTMLImageElement>(".pw .pv-image img")?.naturalWidth === widths[name];
    log(!!(await until(() => showing(last))), "preview renders the last image", last);
    key("ArrowUp");
    log(!!(await until(() => showing(first))), "Up moves to previous file in preview", getState().focused[0]?.name ?? "");
    await shot("09-preview-window");
    key("ArrowDown");
    log(!!(await until(() => showing(last))), "Down returns to the last file in preview", getState().focused[0]?.name ?? "");
    key("Escape");
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
    // These motions count file rows; extension headers are separate cursor stops.
    await withoutTypeGroups(async () => {
      await until(() => !$(".fp-group", browserEl(0)!));
      const names = rowNames(0);
      const focused = () => getState().focused[0]?.name;
      await press(pane(0), "g", "g");
      log(!!(await until(() => !!names[0] && focused() === names[0])), "gg goes to the first item", focused() ?? "none");
      await press(pane(0), "j", "j");
      log(!!(await until(() => !!names[2] && focused() === names[2])), "j moves down", focused() ?? "none");
      await press(pane(0), "k");
      log(!!(await until(() => !!names[1] && focused() === names[1])), "k moves up", focused() ?? "none");
      const last = names.at(-1);
      await press(pane(0), ["G", { shiftKey: true }]);
      log(!!(await until(() => !!last && focused() === last)), "G goes to the last item", focused() ?? "none");
    });
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
    // With type groups, the size order applies separately inside each group.
    await withoutTypeGroups(async () => {
      await until(() => !$(".fp-group", browserEl(0)!));
      await press(pane(0), ",", ["S", { shiftKey: true }]);
      const sortedBySize = await until(() => {
        const names = rowNames(0);
        const direction = $(".fp-hrow .fp-size-col .col-head.on .arrow", browserEl(0)!)?.textContent;
        return direction === "↓"
          && names.includes("archive.zip") && names.includes("file20.txt") && names.includes("file1.txt")
          && names.indexOf("archive.zip") < names.indexOf("file20.txt")
          && names.indexOf("file20.txt") < names.indexOf("file1.txt");
      });
      log(!!sortedBySize, ", S sorts by size descending", rowNames(0).join(", "));
    });
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
    // `text-align: end` (logical, so right-to-left mirrors it) computes to "end"; in this left-to-right check it is the right edge.
    const ltr = getComputedStyle(document.documentElement).direction === "ltr";
    const align = getComputedStyle(ext).textAlign;
    log(align === "right" || (ltr && align === "end"), "type values right-aligned", align);
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
      // The submenu may be shifted up to stay inside the window, so only require it to sit beside the item vertically.
      const beside = !!sr && sr.top <= ir.bottom && sr.bottom >= ir.top && sr.bottom <= window.innerHeight + 1;
      log(!!sr && Math.abs(sr.left - ir.right) < 12 && beside, "submenu opens next to its item", sr ? `item ${Math.round(ir.right)},${Math.round(ir.top)} sub ${Math.round(sr.left)},${Math.round(sr.top)}` : "no submenu");
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
    const { showHistory, showSystemMonitor } = getState().settings!;
    const startSize = { width: innerWidth, height: innerHeight };
    const visibleMonitor = () => $(".sidebar-monitor:not(.is-space-hidden) .sysmon");
    const button = (pressed: boolean) => {
      const toggle = $<HTMLButtonElement>(".sidebar .main-toolbar .sidebar-monitor-toggle");
      return toggle && toggle.getAttribute("aria-pressed") === String(pressed) &&
        toggle.classList.contains("active") === pressed && unobstructed(toggle) ? toggle : null;
    };
    try {
      // The full monitor needs enough height, even with the I/O details open.
      if (innerHeight < 760 || innerWidth < 960) await setWindowSize(1100, 760);
      await updateSettings({ showHistory: false });
      const card = await until(visibleMonitor);
      log(!!card && !!$(".group-head", card) && getComputedStyle(card).boxShadow === "none", "monitor is a sidebar section (group head, no floating card)");
      if (!card) return log(false, "system monitor shown by default");
      const vals = await until(() => { const v = $$(".sysmon .sm-val").map((e) => e.textContent!); return v.length && v.every((t) => /^\d+%$/.test(t)) ? v : null; }, 5000);
      log(!!vals, "meters show live percentages", (vals ?? []).join(" "));
      const labels = $$(".sysmon .sm-lbl").map((e) => e.textContent);
      log(labels.includes("CPU") && labels.includes("Memory"), "CPU and Memory meters", labels.join(","));
      const detailsButton = $(".sysmon .sm-toggle");
      if (detailsButton && unobstructed(detailsButton)) click(detailsButton);
      const det = await until(() => $(".sysmon .sm-details"));
      log(!!det && det.textContent!.includes("Network") && det.textContent!.includes("Disk I/O"), "details show network and disk I/O", det?.textContent?.slice(0, 80));
      await shot("25-system-monitor");
      if (det && detailsButton) click(detailsButton);

      const hide = await until(() => button(true));
      log(!!hide, "toolbar has an active monitor button");
      if (hide) click(hide);
      log(!!(await until(() => !$(".sysmon") && !getState().settings!.showSystemMonitor)), "toolbar button hides the monitor");
      const show = await until(() => button(false));
      log(!!show, "button stays visible to bring it back");
      if (show) click(show);
      log(!!(await until(() => !!visibleMonitor() && button(true))), "toolbar button shows the monitor again");
      log(!$(".sysmon .sm-chart"), "no usage chart (removed)");
      log(getState().settings!.historyCount === 10, "history keeps the last 10 folders by default", String(getState().settings!.historyCount));

      await updateSettings({ showHistory: true });
      await setWindowSize(960, 420);
      const crowded = await until(() => getState().monitorCrowded && getState().settings!.showSystemMonitor && !visibleMonitor() && button(false));
      log(!!crowded, "the crowded sidebar makes room for History and keeps the monitor button reachable", `${innerWidth}×${innerHeight}`);
      await shot("25-system-monitor-crowded");
      if (crowded) click(crowded);
      const shownAnyway = await until(() => !!visibleMonitor() && button(true));
      log(!!shownAnyway && getState().monitorCrowded, "the monitor can be shown while the sidebar is still crowded");
      if (shownAnyway) click(shownAnyway);
      log(!!(await until(() => !$(".sysmon") && button(false))), "the monitor can be hidden again in the short window");
    } finally {
      await setWindowSize(startSize.width, startSize.height);
      await updateSettings({ showHistory, showSystemMonitor });
    }
  });

  await step("per-panel undo", async () => {
    const { navigate, newFolder } = await import("./state/actions");
    navigate(0, root + "/Documents");
    navigate(1, root + "/Projects");
    await until(() => rowNames(0).includes("file1.txt") && rowNames(1).includes("Alpha"));
    const leftBefore = rowNames(0).slice().sort().join(",");
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
    log(!!(await until(() => activePath(0) === root + "/Documents" && rowNames(0).slice().sort().join(",") === leftBefore)), "left panel unchanged by right panel undo", rowNames(0).join(","));
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
    const { navigate, compareSelection, compareWithClipboard, compare, compareGitFile } = await import("./state/actions");
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
      compareGitFile(root + "/Projects/Alpha/notes.txt");
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
      await setWindowSize(w, h);
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
    const text = await api.diffCompare({ kind: "file", path: R + "/newer-right.txt" }, { kind: "text", label: "t", text: "right newer version\n" }, { ignoreWhitespace: false, ignoreCase: false, ignoreEol: true, minimal: false, structural: false });
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
    await focusPane(0);
    await selectRow(0, "L");
    click(row0("R")!, { ctrlKey: true });
    const { controller } = await import("./state/controllers");
    const selected = () => controller(0)?.selection().map((entry) => entry.path).sort().join(",");
    const expected = [root + "/Sync/L", root + "/Sync/R"].sort().join(",");
    log(!!(await until(() => selected() === expected)), "two folders are selected before comparison", selected());
    key("D", { ctrlKey: true, shiftKey: true }, pane(0));
    log(!!(await until(() => $(".dircmp .dc-row"), 5000)), "Ctrl+Shift+D on two selected folders compares them", `selected=${selected()} active=${getState().session?.activeBrowser} overlay=${getState().overlay?.type}`);
    const win = getCurrentWindow();
    await win.unmaximize();
    await setWindowSize(800, 600);
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
    const originalRight = (await api.readTextFile(R)).text;
    const permission = () => $<HTMLInputElement>(".dv-allow-changes input")!;
    const copyButtons = () => $$(".dv-merge button[title^='Copy this change']");
    log(!!permission() && !permission().checked && copyButtons().length === 0, "diff starts read-only with no copy buttons");
    await shot("33-merge-read-only");
    const removed = () => Number(($(".dv-stat .del")?.textContent ?? "−0").replace("−", ""));
    const before = removed();
    key("n", {}, dv);
    await sleep(100);
    key("ArrowRight", { altKey: true }, dv);
    await sleep(300);
    log(!$(".dv-file.r .dv-dirty") && removed() === before, "Alt+→ cannot change a read-only diff");
    click(permission());
    await sleep(100);
    log(permission().checked && copyButtons().length >= 2, "allowing changes reveals copy buttons");
    key("ArrowRight", { altKey: true }, dv);
    log(!!(await until(() => $(".dv-file.r .dv-dirty"), 3000)), "Alt+→ copies the change to the right (marked modified)");
    await until(() => removed() < before, 3000);
    log(removed() < before, "the copied change is gone from the diff", `−${before} → −${removed()}`);
    await shot("33-merge");
    click(permission());
    await sleep(100);
    key("s", { ctrlKey: true }, dv);
    await sleep(300);
    log(!!$(".dv-file.r .dv-dirty") && !permission().checked && copyButtons().length === 0 &&
      (await api.readTextFile(R)).text === originalRight,
    "revoking permission blocks saving and preserves the unsaved draft");
    click(permission());
    await sleep(100);
    key("z", { ctrlKey: true }, dv);
    log(!!(await until(() => !$(".dv-file.r .dv-dirty"), 3000)), "Ctrl+Z undoes the copy inside the view");
    key("ArrowRight", { altKey: true }, dv);
    await until(() => $(".dv-file.r .dv-dirty"), 3000);
    key("s", { ctrlKey: true }, dv);
    log(!!(await until(() => !$(".dv-file.r .dv-dirty"), 4000)), "Ctrl+S saves the right file");
    const saved = await api.diffCompare({ kind: "file", path: R }, { kind: "text", label: "x", text: "alpha\nbeta\ngamma\ndelta\nepsilon\n" }, { ignoreWhitespace: false, ignoreCase: false, ignoreEol: false, minimal: false, structural: false });
    log(!!saved.text && saved.text.added + saved.text.removed === 0, "saved file has the merged content (final newline kept)");
    key("Escape", {}, dv);
    await until(() => !$(".diffv"));
    const { undo } = await import("./state/actions");
    await undo();
    let restored = false;
    for (let k = 0; k < 30 && !restored; k++) {
      const r = await api.diffCompare({ kind: "file", path: R }, { kind: "text", label: "x", text: "alpha\nBETA\ngamma\ndelta\nepsilon\n" }, { ignoreWhitespace: false, ignoreCase: false, ignoreEol: false, minimal: false, structural: false });
      restored = !!r.text && r.text.added + r.text.removed === 0;
      if (!restored) await sleep(100);
    }
    log(restored, "Undo restores the file as it was before saving");
    // read-only side: clipboard text cannot be changed
    const { compareWithClipboard } = await import("./state/actions");
    await api.clipSetText("alpha\nzzz\n");
    await compareWithClipboard(L);
    await until(() => $(".diffv .dv-row"), 4000);
    log(!permission().checked, "a newly opened diff starts read-only");
    click(permission());
    await sleep(100);
    key("n", {}, $(".diffv")!);
    key("ArrowRight", { altKey: true }, $(".diffv")!);
    await sleep(300);
    log(!$(".dv-file.r .dv-dirty") && getState().notifications.some((n) => n.text.includes("read-only")), "clipboard side is read-only");
    key("Escape", {}, $(".diffv")!);
    await sleep(200);
  });

  await step("compare: images", async () => {
    const { compare, dismiss, notify } = await import("./state/actions");
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
    const noteIds = [
      notify("Copied a large image to the comparison folder", "success"),
      notify("Restored an older image to the comparison folder", "success"),
      notify("Moved 3 image files to the trash", "success"),
    ];
    try {
      await until(() => $$(".toasts .toast").length >= 3);
      const stack = $(".toasts");
      const view = $(".diffv");
      log($$(".toasts .toast").length >= 3 && !!stack && !!view &&
        stack.getBoundingClientRect().top >= view.getBoundingClientRect().bottom,
        "stacked notifications stay below the image comparison");
      log(unobstructed($(".ic-bar .seg button.on")), "image difference controls remain reachable");
      await shot("34-image-diff");
    } finally {
      for (const id of noteIds) dismiss(id);
    }
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
      await setWindowSize(w, h);
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
    log([...sel.options].some((o) => o.value === "nightowl") && [...sel.options].some((o) => o.value === "gruvbox"), "Night Owl and Gruvbox are included");
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

/** Runs against the mounted title bar and checks that a plain click has no window effect. */
async function runWindowOverlaysSelfTest() {
  const A = await import("./state/actions");
  const visibleImmediately = (element: Element | null): boolean => {
    if (!element) return false;
    const style = getComputedStyle(element);
    return style.animationName === "none" && style.opacity === "1";
  };
  const verifyOverlay = async (name: string, selector: string) => {
    const element = await until(() => $(selector));
    log(visibleImmediately(element) && visibleImmediately(element!.closest(".backdrop")),
      `${name} opens fully visible without a backdrop fade`);
  };
  try {
    await updateSettings({ keyboardProfile: "vim", animations: true, adaptiveLayout: false });
    await setWindowSize(1100, 760);
    await A.setLayout("classic");
    await focusPane(0);
    await selectRow(0, "sample.svg");
    key("i", {}, pane(0));
    await verifyOverlay("Image preview", ".pw");
    log(!!(await until(() => {
      const image = $<HTMLImageElement>(".pw .pb img");
      return image?.naturalWidth === 240 && getComputedStyle(image).opacity === "1";
    })), "the image paints at full opacity after decoding");
    await shot("window-overlay-image-preview");
    key("Escape", {}, $(".pw"));
    await until(() => !$(".pw"));
    await focusPane(0);
    key("N", { ctrlKey: true, shiftKey: true }, pane(0));
    await verifyOverlay("New Folder dialog", ".modal");
    await shot("window-overlay-new-folder");
    key("Escape", {}, $(".modal input"));
    await until(() => !$(".modal"));
    await A.setLayout("standard");
    const second = await until(() => $$(".browser").filter((el) => el.getBoundingClientRect().width > 0)[1]);
    log(visibleImmediately(second), "the second file pane opens fully visible");
    await shot("window-overlay-second-pane");
  } catch (error) {
    log(false, "window overlays", String(error));
  }
  await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
}

async function runTitleBarSelfTest() {
  const titlebar = document.querySelector<HTMLElement>(".titlebar");
  if (!titlebar) {
    log(false, "title bar is mounted");
    await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
    return;
  }
  const win = getCurrentWindow();
  const state = async () => JSON.stringify({
    position: await win.outerPosition(),
    size: await win.outerSize(),
    maximized: await win.isMaximized(),
  });
  const before = await state();
  const rect = titlebar.getBoundingClientRect();
  const click = (type: string) => titlebar.dispatchEvent(new MouseEvent(type, {
    bubbles: true, cancelable: true, button: 0, detail: 1,
    clientX: rect.left + 12, clientY: rect.top + rect.height / 2,
  }));
  click("mousedown");
  click("mouseup");
  click("click");
  await sleep(250);
  const after = await state();
  log(titlebar.getAttribute("data-tauri-drag-region") === "false", "native mousedown drag region is explicitly disabled");
  log(before === after, "a single title-bar click leaves position, size, and maximize state unchanged", `${before} → ${after}`);
  await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
}

/** Guards the Settings shell and its scroll position while every section is switched. */
async function runSettingsSectionsSelfTest() {
  await updateSettings({ language: "en", theme: "dark", windowButtons: "auto", animations: true });
  const runLayout = async (layout: string, width: number, height: number) => {
    await setWindowSize(width, height);
    const vimKeys = layout === "wide";
    await updateSettings({ vimKeys });
    const opener = vimKeys ? ".main-toolbar > button:first-child" : ".tb-logo";
    const settingsButton = await until(() => $(opener));
    if (!settingsButton) {
      log(false, "Settings button is available");
      return;
    }
    click(settingsButton);
    const modal = await until(() => $(".settings"));
    const panel = await until(() => modal && $(".panel", modal));
    const nav = modal && $(".sbody > nav", modal);
    if (!modal || !panel || !nav) {
      log(false, `Settings shell mounts at ${layout} size`);
      return;
    }

    const backdrop = modal.closest(".backdrop");
    log(!!backdrop && [backdrop, modal, panel].every((element) => {
      const style = getComputedStyle(element);
      return style.animationName === "none" && style.opacity === "1";
    }), `Settings is fully visible on open (${layout}, ${opener})`);
    const search = $<HTMLInputElement>(".ssearch input", modal);
    log(!!search && document.activeElement === search, `Settings search takes focus on open (${layout})`);
    if (search) {
      const keyAllowed = search.dispatchEvent(new KeyboardEvent("keydown", { key: "q", code: "KeyQ", bubbles: true, cancelable: true }));
      typeInto(search, "q");
      log(keyAllowed && !!(await until(() => $(".section-head h2", modal)?.textContent === "Search")),
        `Settings search accepts typing (Vim keys ${vimKeys ? "on" : "off"})`);
      typeInto(search, "");
      await until(() => $(".section-head h2", modal)?.textContent === "Appearance");
    }

    await sleep(250);
    const shellRect = modal.getBoundingClientRect();
    const navRect = nav.getBoundingClientRect();
    const panelRect = panel.getBoundingClientRect();
    let panelAnimations = 0;
    const countPanelAnimation = (event: AnimationEvent) => {
      if (event.target instanceof Element && event.target.matches(".settings .panel")) panelAnimations++;
    };
    document.addEventListener("animationstart", countPanelAnimation, true);
    await shot(`settings-sections-${layout}-appearance`);

    const allNavButtons = $$<HTMLButtonElement>("button", nav);
    const keyboardButton = allNavButtons.find((button) => button.textContent?.trim() === "Keyboard");
    const navButtons = keyboardButton
      ? [keyboardButton, ...allNavButtons.filter((button) => button !== keyboardButton)]
      : allNavButtons;
    for (const [index, button] of navButtons.entries()) {
      if (button.classList.contains("on")) continue;
      const name = button.textContent?.trim() || `section ${index + 1}`;
      const slug = name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
      const animationCount = panelAnimations;
      const scrollProbe = Math.min(24, Math.max(0, panel.scrollHeight - panel.clientHeight));
      if (scrollProbe) panel.scrollTop = scrollProbe;
      click(button);
      await new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
      await sleep(50);

      const currentPanel = $(".panel", modal);
      const currentNav = $(".sbody > nav", modal);
      const activeLabel = $("button.on span", nav)?.textContent?.trim();
      if (name === "Window") {
        const select = $<HTMLSelectElement>("#opt-windowButtons", panel);
        const text = select?.selectedOptions[0]?.label;
        const context = document.createElement("canvas").getContext("2d");
        if (select && text && context) {
          const style = getComputedStyle(select);
          context.font = `${style.fontStyle} ${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
          const available = select.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
          const width = context.measureText(text).width;
          log(select.value === "auto" && width <= available, `Automatic window buttons label fits (${layout})`,
            `"${text}": ${width.toFixed(1)}px of ${available}px`);
        } else {
          log(false, `Automatic window buttons label can be measured (${layout})`);
        }
      }
      const currentModalRect = modal.getBoundingClientRect();
      const currentNavRect = nav.getBoundingClientRect();
      const currentPanelRect = currentPanel?.getBoundingClientRect();
      const currentScrollLimit = currentPanel ? Math.max(0, currentPanel.scrollHeight - currentPanel.clientHeight) : 0;
      const expectedScroll = Math.min(scrollProbe, currentScrollLimit);
      const frame = `settings-sections-${layout}-${index + 1}-${slug}-transition`;
      await api.selftestShot(frame).catch((error) => log(false, `screenshot ${frame}`, String(error)));
      log(currentPanel === panel && currentNav === nav && activeLabel === name,
        `Settings shell and sidebar stay mounted while opening ${name} (${layout})`);
      log(panelAnimations === animationCount, `Settings content does not replay its fade animation for ${name} (${layout})`);
      log(!!currentPanelRect
        && Math.abs(currentModalRect.width - shellRect.width) < 1
        && Math.abs(currentModalRect.height - shellRect.height) < 1
        && Math.abs(currentNavRect.left - navRect.left) < 1
        && Math.abs(currentNavRect.width - navRect.width) < 1
        && Math.abs(currentPanelRect.left - panelRect.left) < 1
        && Math.abs(currentPanelRect.top - panelRect.top) < 1
        && Math.abs(currentPanelRect.width - panelRect.width) < 1
        && Math.abs(currentPanelRect.height - panelRect.height) < 1,
      `Settings shell and sidebar geometry stay stable for ${name} (${layout})`);
      if (scrollProbe) {
        log(!!currentPanel && Math.abs(currentPanel.scrollTop - expectedScroll) < 1,
          `Settings content scroll position stays stable for ${name} (${layout})`,
          `${scrollProbe} → ${currentPanel?.scrollTop ?? "missing"}`);
      }
      await shot(`settings-sections-${layout}-${index + 1}-${slug}`);
    }

    document.removeEventListener("animationstart", countPanelAnimation, true);
    key("Escape", {}, search);
    log(!!(await until(() => !$(".settings"))), `Escape closes Settings (${layout})`);
  };

  await runLayout("wide", 1200, 900);
  await runLayout("compact", 850, 650);
  await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
}

/** Structural diff: reformatting is told apart from changes in the compare view and in both Git patch views. */
async function runStructuralDiffSelfTest() {
  const marked = (selector: string) => $$(selector).map((m) => m.textContent ?? "");
  try {
    // Exercise the real hook and IPC while only the language changes. Render synchronously
    // to inspect the frame before the new request returns, when stale marks used to leak.
    const { createElement } = await import("react");
    const { createRoot } = await import("react-dom/client");
    const { flushSync } = await import("react-dom");
    const { usePatchMarks } = await import("./components/PatchMarks");
    const host = document.createElement("div");
    const root = createRoot(host);
    const patch = "@@ -1 +1 @@\n-a();\n+a( );\n";
    function Probe({ name }: { name: string }) {
      return createElement("span", null, JSON.stringify(usePatchMarks(name, patch)));
    }
    try {
      flushSync(() => root.render(createElement(Probe, { name: "a.rs" })));
      log(!!await until(() => host.textContent === "[null,[],[]]"), "patch hook loads Rust formatting marks through IPC", host.textContent ?? "");
      flushSync(() => root.render(createElement(Probe, { name: "a.txt" })));
      log(host.textContent === "null", "changing language clears marks before the next response");
      log(!!await until(() => host.textContent !== "null" && host.textContent !== "[null,[],[]]"), "plain text keeps the same patch's whitespace as content", host.textContent ?? "");
    } finally {
      flushSync(() => root.unmount());
    }
    await updateSettings({ keyboardProfile: "vim", leaderKey: "Space", shortcuts: {} });
    const A = await import("./state/actions");
    const file = activePath(0) + "/calc.rs";
    A.openOverlay({ type: "diff", a: { kind: "gitHead", path: file }, b: { kind: "file", path: file } });
    log(!!await until(() => $(".diffv .dv-cell.fmt")), "compare view dims the line that was only reformatted");
    log($$(".diffv .dv-cell.fmt").length === 2 && $$(".diffv .dv-cell.fmt .dv-sign").every((s) => s.textContent === "≈"), "the reformatted line is dimmed on both sides", `${$$(".diffv .dv-cell.fmt").length} cells`);
    log(marked(".diffv mark.dv-word").join(",") === "2,3", "only the changed number is marked", marked(".diffv mark.dv-word").join(","));
    log($(".diffv .dv-count")?.textContent?.trim() === "– / 1", "the reformatted line is not counted as a change", $(".diffv .dv-count")?.textContent ?? "");
    log(!!$(".diffv .dv-fmt")?.textContent?.includes("1 formatting only") && !!$(".diffv .dv-stat")?.textContent?.includes("+1"), "the header counts one change and one formatting-only place", $(".diffv .dv-stat")?.textContent ?? "");
    await shot("structural-compare-split");
    key("n", {}, $(".diffv"));
    log(!!await until(() => $(".diffv .dv-row.cur mark.dv-word")), "n goes to the real change, past the reformatted line");
    key("u", {}, $(".diffv"));
    log(!!await until(() => $$(".diffv .dv-row.unified.fmt").length === 2), "unified view dims the reformatted lines");
    await shot("structural-compare-unified");
    key("u", {}, $(".diffv"));
    key("t", {}, $(".diffv"));
    log(!!await until(() => $(".diffv .dv-row") && !$(".diffv .dv-cell.fmt") && $(".diffv .dv-count")?.textContent?.trim() === "– / 2"), "t turns the structural diff off: both lines are changes again", $(".diffv .dv-count")?.textContent ?? "");
    await shot("structural-compare-off");
    key("t", {}, $(".diffv"));
    log(!!await until(() => $(".diffv .dv-cell.fmt")), "t turns it on again");
    key("Escape", {}, $(".diffv"));
    await until(() => !$(".diffv"));

    for (const [name, before, after] of [
      ["escaped.sh", "printf '<%s>\\n' a\\ b", "printf '<%s>\\n' a\\  b"],
      ["long-string.lua", "return [=[a b]=]", "return [=[a  b]=]"],
      ["value.properties", "message=hello world", "message=hello  world"],
    ]) {
      A.openOverlay({ type: "diff", a: { kind: "text", label: name, text: before + "\n" }, b: { kind: "text", label: name, text: after + "\n" } });
      log(!!await until(() => $(".diffv .dv-row") && $(".diffv .dv-count")?.textContent?.trim() === "– / 1"), `${name}: significant spaces count as a change`, $(".diffv .dv-count")?.textContent ?? "");
      log(!$(".diffv .dv-cell.fmt"), `${name}: significant spaces are not dimmed as formatting`);
      key("n", {}, $(".diffv"));
      log(!!await until(() => $(".diffv .dv-row.cur")), `${name}: next change reaches the whitespace edit`);
      await shot(`structural-whitespace-${name}`);
      key("Escape", {}, $(".diffv"));
      await until(() => !$(".diffv"));
    }

    A.openOverlay({ type: "gitGraph", path: activePath(0) });
    await until(() => $(".git-graph-lines .gg-row"));
    $(".git-graph")?.focus();
    key("Enter");
    log(!!await until(() => $(".gc-file")), "Working Changes lists the changed file");
    log(!!await until(() => $$(".gc-diff-line.fmt").length === 2), "Working Changes dims the reformatted lines", `${$$(".gc-diff-line.fmt").length} lines`);
    log(marked(".gc-patch mark.patch-token").join(",") === "2,3", "Working Changes marks only the changed number", marked(".gc-patch mark.patch-token").join(","));
    log($$(".gc-diff-line.fmt input[type=checkbox]").length === 2, "reformatted lines can still be selected for staging");
    await shot("structural-git-changes");
    const commit = $$(".git-graph-lines .gg-row").find((row) => row.textContent?.includes("reflow the call"));
    if (commit) click(commit);
    log(!!await until(() => $(".gcd-patch .gcd-fmt")), "commit details dim the lines of the reflowed call");
    // The reflowed call also gained a trailing comma, two characters after the changed index.
    log(marked(".gcd-patch mark.patch-token").join(" ") === "1 2],", "commit details mark only the changed argument and the new trailing comma", marked(".gcd-patch mark.patch-token").join(" "));
    $(".gcd-patch .gcd-fmt")?.scrollIntoView({ block: "center" });
    await sleep(150);
    await shot("structural-git-commit");
  } catch (e) {
    log(false, "structural diff self-test", String(e));
    await shot("structural-diff-error");
  }
  await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
}

async function runGitCompareSelfTest() {
  try {
    await updateSettings({ keyboardProfile: "vim", leaderKey: "Space", shortcuts: {} });
    await focusPane(0);
    await selectRow(0, "file.txt");
    key("=", {}, pane(0));
    const menu = await until(() => $(".wk"));
    const menuStyle = menu && getComputedStyle(menu);
    log(menuStyle?.animationName === "none" && menuStyle.opacity === "1",
      "compare key menu opens fully visible without shifting");
    await shot("git-compare-menu");
    const scope = 'input[aria-label="Only this file"]';
    const reflog = ".gg-reflog:last-of-type input";
    const graphRows = () => $$(".git-graph-lines .gg-row");
    key("g", {}, pane(0));
    log(!!await until(() => graphRows().length === 4 && $(scope) && !($(scope) as HTMLInputElement).checked), "commit picker starts with repository history");
    const head = graphRows()[0]?.getAttribute("data-commit");
    log(!$(".git-graph-lines")?.textContent?.includes("reflog-only commit"), "reflog commits are hidden by default");
    click($(reflog)!);
    log(!!await until(() => $(".git-graph-lines")?.textContent?.includes("reflog-only commit")), "Reflog checkbox reveals detached commit");
    await shot("git-reflog-enabled");
    click($(reflog)!);
    log(!!await until(() => !($(reflog) as HTMLInputElement)?.checked && graphRows().length === 4), "unchecking Reflog restores branch history");
    click($(scope)!);
    log(!!await until(() => graphRows().length === 3), "checking file scope hides unrelated commits");
    const lastChange = graphRows()[0]?.getAttribute("data-commit");
    const beforeChange = graphRows()[1]?.getAttribute("data-commit");
    await shot("git-compare-commits");

    key("j", {}, $(".git-graph"));
    await until(() => graphRows()[1]?.classList.contains("selected"));
    key("j", {}, $(".git-graph"));
    log(!!await until(() => graphRows()[2]?.classList.contains("selected")), "j selects the oldest commit");
    key("Enter", {}, $(".git-graph"));
    log(!!await until(() => !$(".git-graph") && $(".diffv .dv-row")), "=g replaces the commit picker with the comparison");
    log($(".dv-file.l b")?.textContent === "before.txt" && !!$(".diffv")?.textContent?.includes("first") && !!$(".diffv")?.textContent?.includes("working"), "compare follows the historical name and shows the working copy");
    await shot("git-compare-result");
    key("Escape", {}, $(".diffv"));
    await until(() => !$(".diffv"));
    const fileChord = async (chord: string) => {
      await focusPane(0);
      await selectRow(0, "file.txt");
      key("=", {}, pane(0)); key(chord, {}, pane(0));
    };
    await fileChord("r");
    log(!!await until(() => graphRows().length === 3 && ($(scope) as HTMLInputElement)?.checked), "file history starts scoped to the selected file");
    const oldest = graphRows()[2]?.getAttribute("data-commit");
    click(graphRows()[2]!);
    log(!!oldest && !!await until(() => !$(".git-graph") && $(".dv-file.l .dv-tag")?.getAttribute("title") === oldest && $(".diffv .dv-row")), "=r replaces file history with the selected commit comparison");
    log($(".dv-file.l b")?.textContent === "before.txt" && !!$(".diffv")?.textContent?.includes("first") && !!$(".diffv")?.textContent?.includes("working"), "=r compares the historical path with the current working copy");
    key("Escape", {}, $(".diffv"));
    await until(() => !$(".diffv"));
    await fileChord("h");
    log(!!head && !!await until(() => $(".dv-file.l .dv-tag")?.getAttribute("title") === head), "=h opens HEAD against the working copy");
    key("Escape", {}, $(".diffv"));
    await until(() => !$(".diffv"));
    await fileChord("l");
    log(!!beforeChange && !!lastChange && !!await until(() => $(".dv-file.l .dv-tag")?.getAttribute("title") === beforeChange && $(".dv-file.r .dv-tag")?.getAttribute("title") === lastChange), "=l opens the parent and last file-changing commit");
    log(!!await until(() => $(".diffv [role=status]")?.textContent?.includes("Only line endings differ")), "=l explains a line-ending-only commit");
    key("Escape", {}, $(".diffv"));
    await until(() => !$(".diffv"));

    await updateSettings({ keyboardProfile: "explorer" });
    await getCurrentWindow().unmaximize();
    await setWindowSize(700, 600);
    const A = await import("./state/actions");
    A.openOverlay({ type: "gitCommits", path: activePath(0) + "/new.txt" });
    await until(() => graphRows().length === 4);
    key("r", {}, $(".git-graph"));
    log(!($(reflog) as HTMLInputElement)?.checked, "r does not toggle reflog with Vim disabled");
    key("j", {}, $(".git-graph"));
    await sleep(100);
    log(graphRows()[0]?.classList.contains("selected"), "j does not navigate with Vim disabled");
    const search = $(".git-graph .gg-search");
    search?.focus();
    key("ArrowDown", {}, search);
    log(document.activeElement === graphRows()[0] && !$(".diffv"), "ArrowDown leaves search for the first commit without comparing");
    key("ArrowDown", {}, graphRows()[0]);
    log(!!await until(() => graphRows()[1]?.classList.contains("selected")), "arrow keys select with Vim disabled");
    await shot("git-compare-narrow");
    key("Enter", {}, $(".git-graph"));
    log(!!await until(() => $(".diffv")?.textContent?.includes("file is not in the selected revision")), "missing historical file reports an error");
    await shot("git-compare-missing");
    await updateSettings({ theme: "dark" });
    await setWindowSize(1100, 720);
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
    click($(".modal [data-id=stash]")!); // the dialog button, not the "Stashes" tab behind it
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
  await setWindowSize(1440, 900);
  await win.center().catch(() => {});
  await sleep(600);
  await updateSettings({ theme: "dark", showNavColumns: false, showDetailsPane: false, showSystemMonitor: true });
  setLayout("standard");
  A.toggleColumns(0, false);
  A.toggleColumns(1, false);
  await goHome();
  await until(() => rowNames(0).length > 0 && rowNames(1).length > 0, 5000);
  await selectRow(0, "Glacier lagoon.jpg");
  await sleep(600);
  await shot("main");

  await updateSettings({ showNavColumns: false, showDetailsPane: false });
  setLayout("columns");
  A.toggleColumns(0, false);
  A.toggleColumns(1, false);
  await goHome();
  await sleep(500);
  await selectRow(1, "index.html");
  await shot("stacked");

  setLayout("standard");
  A.toggleColumns(0, false);
  A.toggleColumns(1, false);
  await updateSettings({ theme: "light", showNavColumns: false });
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
  await updateSettings({ theme: "dark", showNavColumns: false, showDetailsPane: false });
  A.toggleColumns(0, false);
  A.toggleColumns(1, false);
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

  // Find in files: a word somewhere in system headers (Linux /usr/include or macOS SDK headers).
  let grepRoot = home + "/Projects";
  for (const cand of ["/usr/include", "/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk/usr/include"]) {
    if ((await api.pathResolve(cand).catch(() => null))?.exists) { grepRoot = cand; break; }
  }
  A.openOverlay({ type: "grep", root: grepRoot, scope: "folder" });
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

  // Git workspace: commit graph, branches, tags and working changes diff on ~/Projects/website.
  const W = args.path2!;
  await A.openGitWorkspace(W).catch(() => {});
  if (await until(() => $$(".git-graph-lines [data-commit]").length > 0, 8000)) {
    await sleep(600);
    const firstCommit = $$(".git-graph-lines [data-commit]").find((r) => !r.textContent?.includes("Working Changes"))
      ?? $$(".git-graph-lines [data-commit]")[0];
    if (firstCommit) click(firstCommit);
    await sleep(700);
    await shot("git-workspace");
    // Close Git workspace cleanly (confirm if exit prompt appears).
    key("Escape", {}, document.body);
    await sleep(250);
    const leaveBtn = $$("button").find((b) => /leave|close|yes/i.test(b.textContent ?? "") && b.closest(".modal"));
    if (leaveBtn) click(leaveBtn);
    await until(() => !$(".gg-root, .git-graph-lines"), 3000);
    await sleep(300);
  }

  // Compare: text, Excel and images.
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
    await setWindowSize(w, h);
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
  await setWindowSize(1280, 820);
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
    for (let i = 0; i < 5; i++) {
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
  const info = getState().info!;
  const panel = () => $("section[aria-label='Application updates']");
  const checkButton = () => $$("button", panel() ?? document).find((button) => button.textContent === "Check for updates") as HTMLButtonElement | undefined;
  const restartButton = () => $$("button", panel() ?? document).find((button) => button.textContent === "Restart to update") as HTMLButtonElement | undefined;
  const terminal = () => {
    const text = panel()?.textContent ?? "";
    const stateAttr = panel()?.getAttribute("data-state") ?? "";
    return !checkButton()?.disabled && ["error", "current", "available", "ready"].includes(stateAttr) ? text : null;
  };
  const phaseFile = `${info.dataDir.replace(/[\\/]+$/, "")}/selftest-updates-phase1.json`;
  try {
    await win.unmaximize();
    await setWindowSize(1280, 820);
    await sleep(500);

    // Installed by a distribution package (package-managed marker): the updater must never
    // download or stage anything; the UI says the system package manager owns updates.
    if (info.installKind === "distro-package" || info.updateStrategy === "storeManaged") {
      log(info.updateStrategy === "storeManaged", "package-managed install reports the storeManaged strategy", info.updateStrategy);
      log(info.installKind === "distro-package", "package-managed install reports the distro-package kind", info.installKind);
      A.openOverlay({ type: "settings", section: "updates" });
      const tab = await until(() => $$(".settings nav button").find((b) => b.textContent?.trim() === "Updates"));
      if (tab) click(tab);
      log(!!(await until(panel)), "Settings Updates opens for a package-managed install");
      await until(() => checkButton() && !checkButton()!.disabled, 25000);
      if (checkButton() && !checkButton()!.disabled) {
        click(checkButton()!);
        await sleep(300);
        await until(() => checkButton() && !checkButton()!.disabled, 25000);
      }
      const text = panel()?.textContent ?? "";
      log(/system|package manager|pacman|omarchy update/i.test(text), "Updates explains that the system package manager owns updates", text.slice(0, 120));
      const labels = $$("button", panel() ?? document).map((b) => b.textContent?.trim() ?? "");
      log(!labels.some((l) => /^(Install|Download|Restart to update)/i.test(l)), "no install / download / restart controls are offered", labels.join(" | "));
      log(Object.keys(getState().tasks).length === 0, "no download task is running");
      const st = panel()?.getAttribute("data-state") ?? "";
      log(st !== "downloading" && st !== "ready", "updater never reaches downloading / ready", st);
      await shot("updates-package-managed");
      results.push("SCREENSHOT updates-package-managed.png: Updates page explaining system-managed updates, with only 'Check for updates'.");
      await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
      return;
    }

    // Phase 2 after an actual signed overlay restart into the updated binary.
    if (info.runningOverlay) {
      let priorResults: string[] = [];
      let priorFailures = 0;
      try {
        const rawBytes = await api.readBytes(phaseFile, 0, 65536);
        const parsed = JSON.parse(new TextDecoder().decode(new Uint8Array(rawBytes))) as { results: string[]; failures: number };
        priorResults = parsed.results ?? [];
        priorFailures = parsed.failures ?? 0;
      } catch (e) {
        log(false, "read phase 1 self-test state after restart", String(e));
      }

      const notice = await until(() => getState().notificationLog.some((n) => n.text.includes(`Update successful: Gall ${info.version} is now installed.`)), 6000);
      log(!!notice, "restarted overlay process announces successful installation", info.version);
      log(info.baseVersion === "0.1.0", "overlay preserves base installed version", info.baseVersion);
      log(info.updatesDir.includes("updates"), "appInfo exposes updatesDir", info.updatesDir);

      A.openOverlay({ type: "settings", section: "about" });
      log(!!(await until(() => $(".about-facts"))), "Settings About renders install and update paths");
      const aboutText = $(".about-facts")?.textContent ?? "";
      log(aboutText.includes(`updated over ${info.baseVersion}`) && aboutText.includes(info.updatesDir), "About shows overlay version note and update directory");
      await shot("updates-about-overlay");
      results.push("SCREENSHOT updates-about-overlay.png: Settings About showing Install and Updates overlay paths.");

      // Search Settings for automatic update setting.
      const searchInput = $<HTMLInputElement>(".ssearch input");
      if (searchInput) {
        typeInto(searchInput, "automatic");
        await sleep(150);
        log($(".settings .panel")?.textContent?.includes("Download updates automatically") ?? false, "Settings search discovers Download updates automatically");
        typeInto(searchInput, "");
        await sleep(100);
      }

      const updatesTab = $$(".settings nav button").find((b) => b.textContent?.trim() === "Updates");
      if (updatesTab) click(updatesTab);
      log(!!(await until(panel)), "Settings Updates opens after restart");
      await until(() => !checkButton()?.disabled, 15000);
      if (checkButton() && !checkButton()!.disabled) {
        click(checkButton()!);
        await until(() => panel()?.getAttribute("data-state") === "checking", 2000);
      }
      await until(() => panel()?.getAttribute("data-state") === "current" && !checkButton()?.disabled, 15000);
      log(panel()?.getAttribute("data-state") === "current", "updated overlay reports Up to date against release server", panel()?.getAttribute("data-state") ?? "");
      const revertBtn = $$(".updates-location button").find((b) => b.textContent?.includes(`Revert to ${info.baseVersion}`));
      log(!!revertBtn, "overlay installation offers Revert button in Settings Updates");

      for (const theme of ["light", "dark"] as const) {
        await updateSettings({ theme });
        for (const [width, height, label] of [[1280, 820, "wide"], [800, 600, "narrow"]] as const) {
          await setWindowSize(width, height);
          await sleep(450);
          panel()?.scrollIntoView({ block: "nearest" });
          await sleep(150);
          const bounds = panel()?.getBoundingClientRect();
          log(!!bounds && bounds.left >= 0 && bounds.right <= window.innerWidth, `update card stays inside ${theme} ${label} window`);
          const name = `updates-result-${theme}-${label}`;
          await shot(name);
          results.push(`SCREENSHOT ${name}.png: ${width}x${height} ${theme}, expected up-to-date overlay card + auto-download switch + paths.`);
        }
      }

      // Keyboard toggle check with Vim keys ON and OFF.
      for (const vimKeys of [true, false]) {
        await updateSettings({ vimKeys });
        const sw = $<HTMLInputElement>("#opt-autoDownloadUpdates");
        log(!!sw, `auto-download switch rendered (vimKeys=${vimKeys})`);
        if (sw) {
          const before = sw.checked;
          sw.focus();
          click(sw);
          await sleep(150);
          log(getState().settings?.autoDownloadUpdates === !before, `auto-download switch toggles cleanly (vimKeys=${vimKeys})`);
          click(sw);
          await sleep(150);
        }
      }

      key("Escape", {}, $(".settings"));
      log(!!(await until(() => !getState().overlay)), "Settings closes normally after update verification");

      const combined = [...priorResults, ...results];
      const totalFailures = priorFailures + failures;
      await api.selftestDone(combined.join("\n") + `\n\n${totalFailures} failure(s)`);
      return;
    }

    // Phase 1 (or standalone run): check About paths, real error + retry, auto-download, staging & restart.
    A.openOverlay({ type: "settings", section: "about" });
    log(!!(await until(() => $(".about-facts"))), "Settings About exposes Install and Updates facts");
    await shot("updates-about-base");
    results.push("SCREENSHOT updates-about-base.png: Settings About showing base Install and Updates locations.");

    const updatesTab = $$(".settings nav button").find((b) => b.textContent?.trim() === "Updates");
    if (updatesTab) click(updatesTab);
    else A.openOverlay({ type: "settings", section: "updates" });

    log(!!(await until(panel)), "Settings Updates exposes the update section");
    log(!!checkButton(), "manual update check has a visible labelled button");
    const support = await invoke<string>("update_support");
    log(typeof support === "string", "real backend reports update support", support || "signed installation supported");

    // Ensure initial background startup check finishes before starting deterministic phase checks.
    await until(() => checkButton() && !checkButton()!.disabled, 25000);
    const initialState = panel()?.getAttribute("data-state") ?? "";

    // If startup already hit the transient 503 failure, inspect error details & screenshots first, then retry!
    if (initialState === "error") {
      log(!!$("[role='alert']", panel()!), "real endpoint failure is presented as an accessible alert");
      const detail = $(".updates-error details", panel()!) as HTMLDetailsElement | null;
      if (detail) {
        log(!detail.open, "technical details are collapsed by default");
        click($("summary", detail)!);
        log(!!(await until(() => detail.open)), "technical details can be expanded without losing the update action");
        log(!!$("pre", detail)?.textContent?.trim(), "technical error evidence is preserved");
        await updateSettings({ theme: "dark" });
        await setWindowSize(800, 600);
        await sleep(300);
        panel()?.scrollIntoView({ block: "end" });
        await sleep(200);
        await shot("updates-error-details-dark-narrow");
        results.push("SCREENSHOT updates-error-details-dark-narrow.png: expanded readable technical error and retry action.");
        click($("summary", detail)!);
      } else {
        await updateSettings({ theme: "dark" });
      }
      await updateSettings({ fontSize: 16 });
      await setWindowSize(640, 600);
      await sleep(450);
      panel()?.scrollIntoView({ block: "nearest" });
      const compactBounds = panel()?.getBoundingClientRect();
      log(!!compactBounds && compactBounds.left >= 0 && compactBounds.right <= window.innerWidth, "update card fits a 640px window with larger text");
      await shot("updates-error-large-type-compact");
      results.push("SCREENSHOT updates-error-large-type-compact.png: 640x600 dark, 16px base text, readable error & check action.");
      await updateSettings({ theme: "light", fontSize: 13 });
      await setWindowSize(1280, 820);
      await sleep(450);
    }

    // Trigger manual check (which succeeds and auto-downloads & stages when local server is active).
    if (panel()?.getAttribute("data-state") !== "ready") {
      click(checkButton()!);
      await until(() => ["checking", "downloading"].includes(panel()?.getAttribute("data-state") ?? ""), 4000);
      await shot("updates-request-wide");
      results.push("SCREENSHOT updates-request-wide.png: active Settings Updates check/download progress.");
      const outcome = await until(terminal, 60000);
      log(!!outcome, "manual update check finishes with a visible result", outcome ?? "timed out");
    }

    if (panel()?.getAttribute("data-state") === "ready" && restartButton() && info.version === "0.1.0") {
      log(true, "auto-download verified, probed, and staged update ready to restart");
      await shot("updates-ready-wide");
      results.push("SCREENSHOT updates-ready-wide.png: staged signed update ready with Restart to update action.");
      await api.saveText(phaseFile, JSON.stringify({ results, failures }));
      click(restartButton()!);
      // Process will exit and restart into the overlay binary (Phase 2 above).
      await sleep(10000);
      log(false, "overlay update restart did not replace process within 10s");
    } else if (info.version === "0.1.0") {
      log(false, "update reaches ready state for restart", panel()?.textContent ?? "");
    } else {
      results.push("SKIP restart phase: standalone run without GALL_UPDATER_TEST_VERSION=0.1.0.");
      key("Escape", {}, $(".settings"));
      log(!!(await until(() => !getState().overlay)), "Settings closes normally after checking for updates");
    }
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
    await setWindowSize(1100, 800);
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

/** Real-app verification for hidden-filter hints and Alt+I across Quick Open, Find in files and F3. */
async function runSearchHiddenSelfTest() {
  const root = activePath(0);
  try {
    await getCurrentWindow().setFocus();
    await setWindowSize(1180, 820);
    await updateSettings({ indexRoots: [root], vimKeys: false, shortcuts: {} });
    await api.indexRefresh(true);
    for (let i = 0; i < 200 && (await api.indexStatus()).building; i++) await sleep(60);
    if ((await api.indexStatus()).building) throw new Error("Index build did not finish");

    // 1. Quick Open: zero visible + N hidden -> Alt+I -> visible + N hidden -> nothing hidden
    await focusPane(0);
    key("p", { ctrlKey: true });
    const qoInput = await until(() => $<HTMLInputElement>(".qo-input input"));
    if (!qoInput) throw new Error("Quick Open did not open");
    typeInto(qoInput, "xyzzy_");
    log(!!await until(() => $(".qo-empty")?.textContent?.includes("2 hidden files match")), "Quick Open empty state shows hidden match count and Alt+I");
    await shot("search-hidden-qo-empty");
    key("i", { altKey: true }, qoInput);
    log(!!await until(() => $$(".qo-row.is-hidden .hidden-badge").length === 2), "Quick Open Alt+I shows hidden results with hidden badge");
    await shot("search-hidden-qo-alt-i");
    key("i", { altKey: true }, qoInput);
    typeInto(qoInput, "report_");
    log(!!await until(() => $$(".qo-row .qo-name").some((n) => n.textContent === "report_visible.ts") && $(".hidden-bar")?.textContent?.includes("+1 hidden by filters")), "Quick Open shows visible result + hidden footer");
    await shot("search-hidden-qo-footer");
    // Switch to everywhere: hidden count still appears and Alt+I works in everywhere too.
    key("Tab", {}, qoInput);
    log(!!await until(() => $(".hidden-bar")?.textContent?.includes("+1 hidden by filters")), "Quick Open everywhere shows hidden footer without Alt+I");
    key("i", { altKey: true }, qoInput);
    log(!!await until(() => $$(".qo-row.is-hidden .hidden-badge").length === 1 && $(".qo-status")?.textContent?.includes("Including hidden files everywhere")), "Quick Open everywhere Alt+I shows hidden results with badge");
    await shot("search-hidden-qo-everywhere-alt-i");
    key("i", { altKey: true }, qoInput);
    key("Tab", {}, qoInput);
    typeInto(qoInput, "definitely_no_such_file");
    await sleep(300);
    log(!!$(".qo-empty") && !$(".qo-empty")?.textContent?.includes("hidden file") && !$(".hidden-bar"), "Quick Open hides hint when nothing is hidden");
    key("Escape", {}, qoInput);
    await until(() => !$(".qo"));

    // 2. Find in files: zero visible + N hidden -> Alt+I -> visible + N hidden -> nothing hidden
    key("f", { ctrlKey: true });
    const fifInput = await until(() => $<HTMLInputElement>('.fif-query input[data-role="query"]'));
    if (!fifInput) throw new Error("Find in files did not open");
    typeInto(fifInput, "hidden_only_needle");
    log(!!await until(() => $(".fif-empty")?.textContent?.includes("3 hidden files match")), "Find in files empty state shows hidden match count and Alt+I");
    await shot("search-hidden-fif-empty");
    key("i", { altKey: true }, fifInput);
    log(!!await until(() => $$(".fif-file.is-hidden .hidden-badge").length === 3), "Find in files Alt+I shows hidden results with hidden badge");
    await shot("search-hidden-fif-alt-i");
    key("i", { altKey: true }, fifInput);
    typeInto(fifInput, "shared_needle");
    log(!!await until(() => $$(".fif-file").length === 1 && $(".hidden-bar")?.textContent?.includes("+1 hidden by filters")), "Find in files shows visible result + hidden footer");
    await shot("search-hidden-fif-footer");
    typeInto(fifInput, "absent_needle_xyz");
    await until(() => $(".fif-empty")?.textContent === "No results");
    await sleep(250);
    log($(".fif-empty")?.textContent === "No results" && !$(".hidden-bar"), "Find in files hides hint when nothing is hidden");
    key("Escape", {}, fifInput);
    await until(() => !$(".fif"));

    // 3. F3 Name search: zero visible + N hidden -> Alt+I -> visible + N hidden -> nothing hidden
    key("F3");
    const f3Input = await until(() => $<HTMLInputElement>('.search-win .shead input[type="text"]'));
    if (!f3Input) throw new Error("F3 search did not open");
    typeInto(f3Input, "xyzzy_*");
    key("Enter", {}, f3Input);
    log(!!await until(() => $(".search-empty")?.textContent?.includes("2 hidden files match")), "F3 empty state shows hidden match count and Alt+I");
    await shot("search-hidden-f3-empty");
    key("i", { altKey: true }, f3Input);
    log(!!await until(() => $$(".search-win .r.is-hidden .hidden-badge").length === 2), "F3 Alt+I immediately re-runs and marks hidden results");
    await shot("search-hidden-f3-alt-i");
    key("i", { altKey: true }, f3Input);
    typeInto(f3Input, "report_*");
    key("Enter", {}, f3Input);
    log(!!await until(() => $$(".search-win .r").length === 1 && $(".hidden-bar")?.textContent?.includes("+1 hidden by filters")), "F3 shows visible result + hidden footer");
    await shot("search-hidden-f3-footer");
    typeInto(f3Input, "no_such_f3_*");
    key("Enter", {}, f3Input);
    await until(() => $(".search-empty")?.textContent === "No results");
    await sleep(250);
    log($(".search-empty")?.textContent === "No results" && !$(".hidden-bar"), "F3 hides hint when nothing is hidden");
    key("Escape", {}, f3Input);
    await until(() => !$(".search-win"));
  } catch (e) {
    log(false, "search-hidden integration", String(e));
  }
  await api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`);
}

/** Notification feedback through the real keyboard handlers, IPC and bug reporter. */
async function runNotificationSelfTest() {
  const A = await import("./state/actions");
  const { join } = await import("./core/path");
  const root = activePath(0);
  const projects = join(root, "Projects");
  const clear = () => getState().notifications.forEach((n) => A.dismiss(n.id));
  const toast = (text: string) => $$(".toast").find((el) => $(".t", el)?.textContent?.includes(text));
  const reportButton = (el: Element) => $$("button", el).find((b) => b.textContent === "Report a bug");
  const verifyToast = async (text: string, kind: string, shotName?: string) => {
    const card = await until(() => toast(text));
    log(!!card?.classList.contains(kind) && !reportButton(card), `${text} shows as ${kind} without a report action`);
    const style = card && getComputedStyle(card);
    log(style?.animationName === "none" && style.opacity === "1", `${kind} notification opens fully visible`);
    if (card && shotName) await shot(shotName);
    return card;
  };
  const options = { showHidden: false, hideTemp: false };
  const listMissingFolder = () => api.listDir(join(root, "missing-notification-folder"), options).catch(() => {});
  const createFolder = async (name: string) => {
    await focusPane(0);
    key("N", { ctrlKey: true, shiftKey: true });
    const input = await until(() => $<HTMLInputElement>(".modal input[type=text]"));
    if (!input) throw new Error("New Folder input did not open");
    typeInto(input, name);
    key("Enter", {}, input);
    await until(() => !getState().overlay);
  };
  try {
    await getCurrentWindow().unmaximize();
    await setWindowSize(1100, 800);
    await updateSettings({ keyboardProfile: "vim", theme: "dark", animations: true, shortcuts: {} });
    await focusPane(0);
    await verifyToast("Restored “settings.json” from backup.", "warn", "notification-recovered-settings");
    log($$(".toast").length === 1, "settings recovery shows one notice");
    clear();
    log(!!(await until(() => !$(".toast"))), "dismissed notifications leave the view clear");

    const originalMount = api.volumeMount;
    const mountFailure = "wrong fs type, bad option, bad superblock on /dev/sdb1, missing codepage or helper program, or other error";
    let rejectMount!: (reason?: unknown) => void;
    api.volumeMount = () => new Promise<string>((_resolve, reject) => { rejectMount = reject; });
    try {
      const mounting = A.mountVolume({ id: "/dev/sdb1", label: "GaMa87", size: 932_000_000_000, fs: "exfat", locked: false, removable: true });
      const progress = await until(() => toast("Mounting “GaMa87”"));
      log(!!progress, "mounting a volume shows its progress toast");
      rejectMount(mountFailure);
      await mounting;
      const mapped = await until(() => getState().notifications.find((n) => n.details === mountFailure));
      const card = mapped && toast(mapped.text);
      const technical = card?.querySelector<HTMLDetailsElement>(".toast-details");
      log(!!mapped && !!card && mapped.kind === "warn" && !mapped.reportError && !!technical
        && !reportButton(card), "filesystem mount failure gives actionable feedback with expandable details and no bug report");
      if (technical) {
        click($("summary", technical)!);
        log(technical.open && $("pre", technical)?.textContent === mountFailure, "expanded mount details preserve the raw system error");
      }
      if (card) await shot("notification-volume-mount-failure");
      log(!getState().notifications.some((n) => n.text.startsWith("Mounting “GaMa87”")), "mounting toast is removed after failure");
      clear();
    } finally {
      api.volumeMount = originalMount;
    }

    key("u");
    await verifyToast("Nothing to undo", "info", "notification-empty-undo-vim");
    const firstId = getState().notifications.at(-1)?.id;
    key("u");
    await until(() => getState().notifications.at(-1)?.id !== firstId);
    log(getState().notificationLog.filter((note) => note.text === "Nothing to undo").length === 1
      && $$(".toast.info").length === 1, "repeated undo keeps one toast and one notification history entry");
    clear();

    await setWindowSize(640, 540);
    await updateSettings({ keyboardProfile: "explorer", theme: "light" });
    await focusPane(0);
    key("u");
    await sleep(90);
    log(!toast("Nothing to undo") && !!$(".quick-filter", browserEl(0)!)?.textContent?.includes("u"),
      "plain u still types in Explorer mode");
    key("Escape");
    await focusPane(0);
    key("z", { ctrlKey: true });
    await verifyToast("Nothing to undo", "info", "notification-empty-undo-explorer");
    clear();

    await createFolder("bad/name");
    await verifyToast("is not a valid name", "warn", "notification-invalid-name");
    clear();

    await createFolder("Undo fixture");
    await until(() => rowNames(0).includes("Undo fixture") && getState().undoLabel);
    log(rowNames(0).includes("Undo fixture"), "valid folder name creates the folder");
    await A.undoIn(projects);
    await verifyToast("Nothing to undo in this folder", "info");
    log(rowNames(0).includes("Undo fixture"), "folder undo leaves other folders' history available");
    clear();
    await focusPane(0);
    key("z", { ctrlKey: true });
    const undone = await until(() => !rowNames(0).includes("Undo fixture") && $(".toast.success"));
    log(!!undone?.textContent?.includes("Undo fixture"), "Ctrl+Z still undoes a real action");
    if (undone) await shot("notification-successful-undo");
    clear();

    const source = join(join(root, "Documents"), "file1.txt");
    await api.startTask({ kind: "copy", sources: [source], dest: projects, conflict: "skip" });
    await verifyToast("Copied “file1.txt”", "success");
    clear();

    // WN-174: two consecutive copies keep both cards visible; clicking Undo on the older card
    // reverses ONLY the older copy, leaves the newer copy intact, and reports unavailable on repeat.
    const docsDir = join(root, "Documents");
    const firstSrc = join(docsDir, "undo-first.txt");
    const secondSrc = join(docsDir, "undo-second.txt");
    await api.saveText(firstSrc, "first copy content\n");
    await api.saveText(secondSrc, "second copy content\n");
    await api.startTask({ kind: "copy", sources: [firstSrc], dest: projects, conflict: "keepBoth" });
    const firstCard = await verifyToast("Copied “undo-first.txt”", "success");
    await api.startTask({ kind: "copy", sources: [secondSrc], dest: projects, conflict: "keepBoth" });
    const secondCard = await verifyToast("Copied “undo-second.txt”", "success");
    await shot("notification-two-copy-undo-cards");
    const firstUndoNote = getState().notifications.find((n) => n.text.includes("undo-first.txt") && n.action?.label === "Undo");
    const firstUndoBtn = firstCard && $$("button", firstCard).find((b) => b.textContent === "Undo");
    log(!!firstCard && !!secondCard && !!firstUndoBtn && !!firstUndoNote, "both copy success cards stay visible with independent Undo actions");
    if (firstUndoBtn) click(firstUndoBtn);
    await verifyToast("Removed “undo-first.txt”", "success");
    const afterFirst = await api.listDir(projects, options);
    log(!afterFirst.some((e) => e.name === "undo-first.txt") && afterFirst.some((e) => e.name === "undo-second.txt"),
      "clicking Undo on the older card reverses only the older copy and leaves the newer copy intact",
      afterFirst.map((e) => e.name).join(","));
    clear();
    // Repeating the already-consumed notification action reports unavailable without touching undo-second.txt.
    if (firstUndoNote?.action) await firstUndoNote.action.run();
    await verifyToast("This action can no longer be undone", "info");
    const afterRepeat = await api.listDir(projects, options);
    log(afterRepeat.some((e) => e.name === "undo-second.txt"),
      "repeating an already-undone notification reports unavailable without changing other files");
    clear();
    // Global undo still reverses the remaining newer copy in LIFO order.
    await A.undo();
    await verifyToast("Removed “undo-second.txt”", "success");
    const afterGlobal = await api.listDir(projects, options);
    log(!afterGlobal.some((e) => e.name === "undo-second.txt"), "global undo still reverses the remaining newer copy");
    clear();

    await api.startTask({ kind: "copy", sources: [source], dest: projects, conflict: "skip" });
    const skipped = await verifyToast("Skipped “file1.txt”", "info", "notification-skipped-copy");
    log(!!skipped && !$$("button", skipped).some((b) => b.textContent === "Undo"), "a fully skipped copy does not offer undo");
    clear();

    const docs = join(root, "Documents");
    await api.startTask({ kind: "copy", sources: [docs], dest: projects, conflict: "skip", merge: true });
    await verifyToast("Copied “Documents”", "success");
    const previousUndo = await api.undoPeek();
    clear();
    await api.startTask({ kind: "copy", sources: [docs], dest: projects, conflict: "skip", merge: true });
    const merged = await verifyToast("Skipped “Documents”", "info");
    log(!!merged && !$$("button", merged).length && (await api.undoPeek()) === previousUndo,
      "a merge with every child skipped leaves undo history intact");
    clear();

    await api.startTask({ kind: "copy", sources: [join(root, "missing-notification-file")], dest: projects });
    const failed = await verifyToast("missing-notification-file", "error", "notification-expected-failure");
    if (failed) {
      const dismiss = $('[aria-label="Dismiss notification"]', failed);
      if (dismiss) {
        dismiss.focus();
        const nativeKeys = ["Enter", " ", "Tab"].every((key) => dismiss.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true })));
        log(document.activeElement === dismiss && nativeKeys, "focused dismiss button keeps native keyboard controls");
        click(dismiss);
      }
      log(!!dismiss && !!(await until(() => !toast("missing-notification-file"))) && pane(0).contains(document.activeElement),
        "dismissing a focused toast returns keyboard focus to the file list");
    }
    await listMissingFolder();
    const initial = await api.bugReportPrepare("", false, {}, false);
    log(!initial.body.includes("create_dir failed") && !initial.body.includes("list_dir failed")
      && !initial.body.includes("Copy failed") && !initial.body.includes("Nothing to undo")
      && !initial.body.includes("settings.json"),
      "routine command failures stay out of the compact bug report");

    // An invalid argument simulates a frontend/backend contract fault without touching user data.
    await api.listDir(null as unknown as string, options).catch(A.fail);
    const error = await until(() => toast("command `list_dir`"));
    const report = error && reportButton(error);
    log(!!error?.classList.contains("error") && !!report, "the real IPC fault offers Report a bug");
    await listMissingFolder();
    const latest = await api.bugReportPrepare("", false, {}, false);
    log(latest.body.includes("command `list_dir`"), "a later missing-folder rejection leaves the original fault in diagnostics");
    if (error) await shot("notification-reportable-failure");

    await api.createDir(root, null as unknown as string).catch(A.fail);
    const newer = await until(() => toast("command `create_dir`"));
    const current = await api.bugReportPrepare("", false, {}, false);
    log(!!newer && current.body.includes("command `create_dir`"), "a newer unexpected fault becomes the latest error");
    const dismissNewer = newer && $('[aria-label="Dismiss notification"]', newer);
    if (dismissNewer) click(dismissNewer);
    if (report) {
      await updateSettings({ keyboardProfile: "vim" });
      await focusPane(0);
      await press(pane(0), " ", "s", "n");
      log(document.activeElement === report && report.dispatchEvent(new KeyboardEvent("keydown", {
        key: " ", bubbles: true, cancelable: true,
      })), "the Vim Notifications command focuses a toast action with native Space activation");
      key("Escape");
      log(!getState().showNotificationLog && pane(0).contains(document.activeElement), "Escape from the notification returns focus to the file list");
      click(report);
    }
    const chosen = await until(() => $(".bug-report pre")?.textContent?.includes("command `list_dir`"));
    log(!!chosen && !$(".bug-report pre")?.textContent?.includes("command `create_dir`"),
      "the report uses the error from the selected toast");
    log(!toast("command `list_dir`"), "the report action dismisses its toast");
  } catch (e) {
    log(false, "notification integration", String(e));
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
    await setWindowSize(1100, 800);
    // Two browsers are needed for F6 and Alt+middle-click: adaptive layout would pick one at 1100 px (150 % display).
    await updateSettings({ vimKeys: false, leaderKeyBasic: "Ctrl Ctrl", shortcuts: {}, adaptiveLayout: false });
    await setLayout("standard");
    await focusPane(0);

    key("F1", { shiftKey: true });
    log(!!(await until(() => $(".kh"))), "Shift+F1 opens help without vim");
    log($(".kh")?.textContent?.includes("File list (standard)") === true && !$(".kh")?.textContent?.includes("File list (vim)"), "basic help shows standard navigation, not vim commands");
    log(!!$('.kh .key-bindings[aria-label="Space"]'), "basic preview hint uses Space");
    await shot("keyboard-basic-wide");
    await setWindowSize(800, 600);
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
    await setWindowSize(1100, 800);
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
    log(!!(await until(() => !row(0, "file1.txt") && row(1, "file1.txt"))), "F6 moves the selected file to the other browser", JSON.stringify({
      overlay: getState().overlay?.type,
      window: `${innerWidth}x${innerHeight}@${devicePixelRatio}`, adaptiveMode: getState().adaptiveMode,
      shown: getState().shownBrowsers, layout: getState().session?.layout, collapsed: getState().session?.browsers.map((x) => x.collapsed),
      pane0: activePath(0), pane1: activePath(1), rows0: rowNames(0), rows1: rowNames(1),
      selected: controller(0)!.selection().map((entry) => entry.name),
      notifications: getState().notificationLog.slice(0, 3).map((note) => note.text),
    }));
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

    // Folder marks: m{x} sets, '{x} / `{x} jump, '' returns, dm{x} deletes; c is the colour tag.
    const marks = () => getState().marks.marks;
    await A.navigate(0, docs);
    await until(() => rowNames(0).includes("file2.txt"));
    await selectRow(0, "file2.txt");
    key("m");
    const setList = await until(() => $(".wk-marks"), 1500);
    log(!!setList && !!setList.textContent?.includes("No marks yet"), "m shows the (empty) mark list", setList?.textContent?.slice(0, 80) ?? "");
    await shot("keyboard-vim-marks-set-empty");
    key("a");
    log(!!(await until(() => marks().a?.path === docs && !!marks().a?.item?.endsWith("file2.txt"))), "m a marks the folder and its focused item", JSON.stringify(marks().a ?? null));
    log(!$(".wk"), "the mark list closes after the letter");
    await A.navigate(0, root);
    await until(() => activePath(0) === root && rowNames(0).includes("Documents"));
    await focusPane(0);
    key("m");
    await sleep(60);
    key("B", { shiftKey: true });
    log(!!(await until(() => marks().B?.path === root)), "m B (upper case) is a separate mark");
    const stored = await api.marksGet();
    log(stored.marks.a?.path === docs && stored.marks.B?.path === root, "marks are saved in the backend store", JSON.stringify(stored));
    key("'");
    const jumpList = await until(() => $(".wk-marks .wk-grid"), 1500);
    log(!!jumpList && jumpList.textContent!.includes("Documents") && $$(".wk-marks .wk-item").length === 2, "' lists the marks with their folders", jumpList?.textContent ?? "");
    await shot("keyboard-vim-marks-jump-list");
    key("a");
    log(!!(await until(() => activePath(0) === docs)), "' a opens the marked folder", activePath(0));
    log(!!(await until(() => marks()["'"]?.path === root)), "the folder left behind becomes mark '");
    await until(() => rowNames(0).includes("file2.txt"));
    await focusPane(0);
    key("'"); await sleep(60); key("'");
    log(!!(await until(() => activePath(0) === root)), "'' returns to the folder before the jump", activePath(0));
    await until(() => rowNames(0).includes("Documents"));
    await selectRow(0, "Documents");
    key("`"); await sleep(60); key("a");
    log(!!(await until(() => activePath(0) === docs && controller(0)?.focused()?.name === "file2.txt", 3000)), "` a opens the folder and selects the marked item", JSON.stringify({
      focused: controller(0)?.focused()?.path ?? "none", mark: marks().a?.item, rows: rowNames(0),
      selected: controller(0)?.selection().map((entry) => entry.name),
    }));
    // US International: ' is a dead key and Shift+b after it arrives composed.
    await focusPane(0);
    key("Dead", { code: "Quote" }); await sleep(60); key("Á", { code: "KeyB", shiftKey: true });
    log(!!(await until(() => activePath(0) === root)), "dead ' then a composed letter still jumps (mark B)", activePath(0));
    await until(() => rowNames(0).includes("Documents"));
    await focusPane(0);
    key("'"); await sleep(60); key("'");
    await until(() => activePath(0) === docs && rowNames(0).includes("file2.txt"));
    await focusPane(0);
    key("'"); await sleep(60); key("x");
    log(!!(await until(() => getState().notifications.some((n) => n.text.includes("Mark x is not set")))), "' x warns about an unset mark");
    log(activePath(0) === docs, "an unset mark does not navigate");
    const clipBefore = JSON.stringify(getState().clipboard);
    for (const k of ["d", "m", "a"]) { key(k); await sleep(60); }
    log(!!(await until(() => !marks().a)), "d m a deletes the mark");
    log(JSON.stringify(getState().clipboard) === clipBefore, "d m does not cut the selection");
    await selectRow(0, "file1.txt");
    key("c");
    log(!!(await until(() => getState().overlay?.type === "tag")), "c opens the colour tag popup");
    key("Escape");
    await until(() => !getState().overlay);

    await updateSettings({ vimKeys: false });
    await focusPane(0);
    key("m");
    await sleep(300);
    log(!$(".wk-marks") && !getState().keySeq.length && !!$(".quick-filter", browserEl(0)!)?.textContent?.includes("m"), "without vim keys m types into the filter, not a mark", $(".quick-filter", browserEl(0)!)?.textContent ?? "no filter");
    key("Escape");
    await until(() => rowNames(0).length > 1);
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
    const legend = $(".kh-legend")!;
    const ok = help.left >= 0 && help.right <= innerWidth + 1 && help.bottom <= innerHeight + 1
      && scroll.scrollWidth <= scroll.clientWidth + 1
      && legend.scrollWidth <= legend.clientWidth + 1;
    const detail = ok
      ? ""
      : `win=${innerWidth}x${innerHeight} help=${Math.round(help.left)},${Math.round(help.right)},${Math.round(help.bottom)} scroll=${scroll.scrollWidth}/${scroll.clientWidth} legend=${legend.scrollWidth}/${legend.clientWidth} wide=${(() => {
          const edge = scroll.getBoundingClientRect().left + scroll.clientWidth + 1;
          return $$("*", scroll).filter((el) => el.getBoundingClientRect().right > edge).slice(0, 4).map((el) => `${el.tagName.toLowerCase()}.${String(el.className).split(" ")[0]}@${Math.round(el.getBoundingClientRect().right - edge)}`).join(",");
        })()}`;
    return { ok, detail };
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
      await setWindowSize(width, width === 1100 ? 800 : 600);
      await updateSettings({ keyboardProfile: profile, theme, fontSize: width === 640 ? 16 : 13, leaderKey: "Space", leaderKeyBasic: "Ctrl Ctrl", shortcuts: {} });
      await focusPane(0);
      key("?", { shiftKey: true });
      log(!!(await until(() => $(".kh"))), `? opens actual help for ${KEYBOARD_PROFILES[profile]}`);
      log($(".kh-head .pill")?.textContent === KEYBOARD_PROFILES[profile], "help names the active profile");
      log(!!$(".kh-legend .key-plus") && !!$(".kh-legend .key-then") && !!$(".kh-legend .key-or"), "legend separates together, in-order and alternatives");
      const fit = fits();
      log(fit.ok, `help fits ${width}px ${theme} window without horizontal scrolling`, fit.detail);
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
    // The first Escape may only close an open dropdown of the themed selector.
    for (let i = 0; i < 3; i++) {
      key("Escape");
      if (await until(() => !getState().overlay, 600)) break;
    }
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
    await setWindowSize(1100, 800);
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
    await setWindowSize(800, 600);
    await updateSettings({ theme: "light" });
    await shot("key-profiles-welcome-light-800");
    log($(".welcome")!.scrollWidth <= $(".welcome")!.clientWidth + 1, "three welcome cards fit the narrow window");
    await close();
    await setWindowSize(1100, 800);
    await updateSettings({ keyboardProfile: "vim", theme: "dark" });
    await press(pane(0), " ", "f", "c");
    log(!!(await until(() => $(".settings"))), "Vim Space f c opens settings for keyboard profile selection");
    for (const profile of ["explorer", "totalCommander", "vim"] as const) {
      const select = await choose(profile);
      log(select.options.length === 3 && select.value === profile, "native keyboard-accessible selector reflects the active profile");
      const typed = new KeyboardEvent("keydown", { key: "j", bubbles: true, cancelable: true });
      select.dispatchEvent(typed);
      // The themed selector takes the typed letter itself (type-ahead opens its list): it must not reach the Vim keys.
      log(!getState().keySeq.length, "profile selector keeps typing instead of starting Vim commands", `prevented=${typed.defaultPrevented} keySeq=${getState().keySeq.join("")}`);
      await shot(`key-profiles-settings-${profile}`);
      await close();
      key("?", { shiftKey: true });
      log(!!(await until(() => $(".kh-head .pill")?.textContent === KEYBOARD_PROFILES[profile])), "? opens correctly named profile help", `pill=${$(".kh-head .pill")?.textContent} overlay=${getState().overlay?.type} active=${document.activeElement?.tagName}.${document.activeElement?.className}`);
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
    // The first row: with the default date order file1.txt is the last one and j has nowhere to go.
    await selectRow(0, rowNames(0)[0]);
    const before = controller(0)!.focused()?.path;
    key("j");
    log(!!(await until(() => controller(0)!.focused()?.path !== before)), "Vim j navigates rather than typing a filter", `vimKeys=${getState().settings?.vimKeys} profile=${getState().settings?.keyboardProfile} before=${before} now=${controller(0)!.focused()?.path} rows=${rowNames(0).join(",")} active=${document.activeElement?.tagName}.${document.activeElement?.className} filter=${!!$(".quick-filter")}`);
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
    await setWindowSize(800, 600);
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
  const { getReportDraft } = await import("./core/reportDraft");
  const { join } = await import("./core/path");
  try {
    await getCurrentWindow().unmaximize();
    await setWindowSize(1100, 800);
    await sleep(250);
    const previous = await api.bugReportPrepare("", false, {}, true);
    if (!entryOnly) log(previous.body.includes("list_dir failed") && previous.body.includes("previous irregular exit"), "actual backend restores previous unclean session diagnostics");
    for (let i = 0; i < 200; i++) await api.bugReportRecord("command", `list_dir ${i} success`);
    await api.bugReportRecord("command", "move failed 25");
    await api.bugReportRecord("error", "Older failure excluded from GitHub");
    // Deliberately send an invalid argument to exercise an actual IPC contract failure.
    await api.listDir(null as unknown as string, { showHidden: false, hideTemp: false }).catch(() => {});
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
    await setWindowSize(800, 600);
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
    await setWindowSize(1100, 800);
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
    await setWindowSize(800, 600);
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
    await setWindowSize(1100, 800);
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
    await setWindowSize(800, 600);
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
    await setWindowSize(1100, 800);
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
    await setWindowSize(800, 600);
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
    await setWindowSize(1100, 800);
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
    await setWindowSize(800, 600); await sleep(200);
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

export const gitSelftest = { note: (line: string) => { results.push(line); }, api, $, $$, key, click, mouse, typeInto, until, sleep, shot, setWindowSize, log, focusPane, activePath, updateSettings, getState, drag, finish: () => api.selftestDone(results.join("\n") + `\n\n${failures} failure(s)`) };
