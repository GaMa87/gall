// E2E selftest modes for CLI launch state, the FileManager1 reveal loop guard and the file clipboard.
import type { FileEntry } from "./api/types";
import type { ModeGroup, SelftestTools } from "./selftestE2e";

const names = (es: FileEntry[]) => es.map((e) => e.name).join(", ");

/** Waits until the browser's folder is listed (the launch request may still be applying). */
async function settle(t: SelftestTools, b = 0) {
  const { controller } = await import("./state/controllers");
  await t.until(() => controller(b)?.path() && controller(b)!.entries().length >= 0 && controller(b)!.path() === t.activePath(b));
  await t.sleep(1200);
  return controller(b);
}

async function cliState(t: SelftestTools) {
  const { controller } = await import("./state/controllers");
  const c0 = await settle(t, 0);
  const sel = c0?.selection() ?? [];
  const s = t.getState();
  t.log(true, `cli-state: browser0=${t.activePath(0)} browser1=${t.activePath(1)} layout=${s.session?.layout}`);
  t.log(true, `cli-state: selected=[${names(sel)}] focused=${c0?.focused()?.path ?? "-"}`);
  t.log(true, `cli-state: entries0=[${names(c0?.entries() ?? [])}] entries1=[${names(controller(1)?.entries() ?? [])}]`);
  const o = s.overlay;
  if (o?.type === "diff") {
    const src = (x: typeof o.a) => (x.kind === "file" || x.kind === "gitHead" ? x.path : x.kind);
    t.log(true, `cli-state: compare overlay a=${src(o.a)} b=${src(o.b)}`);
  } else if (o?.type === "dircmp") t.log(true, `cli-state: compare folders left=${o.left} right=${o.right}`);
  else if (o?.type === "gitGraph") t.log(true, `cli-state: git workspace path=${o.path}`);
  else t.log(true, `cli-state: overlay=${o?.type ?? "none"}`);
}

async function dbusReveal(t: SelftestTools) {
  const { controller } = await import("./state/controllers");
  const { parent } = await import("./core/path");
  const root = t.activePath(0);
  const list = (dir: string) => t.api.listDir(dir, { showHidden: false, hideTemp: false }).catch(() => []);
  // The deepest item to show: nested/found.txt when the scenario made it, else any file here.
  const nested = (await list(root)).find((e) => e.isDir && e.name === "nested");
  const inNested = nested ? (await list(nested.path)).find((e) => !e.isDir) : undefined;
  const target = inNested ?? (await list(root)).find((e) => !e.isDir) ?? nested;
  if (!target) return t.log(true, `reveal: nothing to reveal under ${root}; skipped`);
  const dir = parent(target.path) ?? root;
  await t.api.revealPath(target.path);
  const ok = await t.until(() => t.activePath(0) === dir && controller(0)?.selection().some((e) => e.path === target.path), 6000);
  t.log(!!ok, "reveal_path navigates internally and selects the item while Gall owns FileManager1", `folder=${t.activePath(0)} selected=[${names(controller(0)?.selection() ?? [])}] target=${target.path}`);
}

/** Selects every entry of browser `b` and returns them. */
async function selectAll(t: SelftestTools, b: number, only?: string[]) {
  const c = await settle(t, b);
  const all = (c?.entries() ?? []).filter((e) => !e.isDir && (!only || only.includes(e.name)));
  c?.select(all.map((e) => e.path), all[0]?.path);
  await t.sleep(200);
  return all;
}

async function clipDump(t: SelftestTools) {
  const dump = new Map(await t.api.selftestClipDump());
  for (const [target, payload] of dump) t.log(true, `clipboard target ${target}: ${JSON.stringify(payload)}`);
  return dump;
}

async function clipCopy(t: SelftestTools) {
  const A = await import("./state/actions");
  const files = await selectAll(t, 0);
  await A.copyToClipboard(files, false);
  await t.sleep(400);
  const d = await clipDump(t);
  const gnome = d.get("x-special/gnome-copied-files") ?? "";
  const uris = d.get("text/uri-list") ?? "";
  const plain = d.get("text/plain;charset=utf-8") ?? "";
  t.log(gnome.startsWith("copy\nfile://") && !gnome.includes("\r"), "x-special/gnome-copied-files is copy\\nfile://… with LF", JSON.stringify(gnome));
  t.log(uris.startsWith("file://") && uris.endsWith("\r\n") && uris.split("\r\n").length === files.length + 1, "text/uri-list uses CRLF line endings (crlf)", JSON.stringify(uris));
  t.log(files.length > 0 && files.every((f) => plain.split(/\r?\n/).includes(f.path) && !plain.includes("\r")), "text/plain;charset=utf-8 lists the plain paths", JSON.stringify(plain));
}

async function clipCutPaste(t: SelftestTools) {
  const A = await import("./state/actions");
  const { controller } = await import("./state/controllers");
  const files = await selectAll(t, 0, ["cutme.txt"]);
  await A.copyToClipboard(files, true);
  await t.sleep(400);
  const d = await clipDump(t);
  const gnome = d.get("x-special/gnome-copied-files") ?? "";
  t.log(gnome.startsWith("cut\nfile://"), "cut payload starts with cut\\n", JSON.stringify(gnome));
  await settle(t, 1);
  A.focusBrowser(1);
  await A.paste(1);
  let moved = false;
  for (let i = 0; i < 60 && !moved; i++) {
    await t.sleep(100);
    moved = (await t.api.listDir(t.activePath(1), { showHidden: false, hideTemp: false })).some((e) => e.name === "cutme.txt");
  }
  t.log(moved, "paste moved cutme.txt into the second browser's folder");
  void controller;
}

async function clipPaste(t: SelftestTools) {
  const A = await import("./state/actions");
  const dir = t.activePath(0);
  await settle(t, 0);
  const before = new Set((await t.api.listDir(dir, { showHidden: false, hideTemp: false })).map((e) => e.name));
  // On Wayland the clipboard is only handed to the focused window: wait for it.
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  let content = await t.api.clipInspect();
  for (let i = 0; i < 25 && content.kind === "empty"; i++) {
    await getCurrentWindow().setFocus().catch(() => {});
    await t.sleep(200);
    content = await t.api.clipInspect();
  }
  t.log(true, `paste: clipboard kind=${content.kind}${content.kind === "paths" ? ` count=${content.paths.length} cut=${content.cut ?? false}` : ""}`);
  // Only file clipboards are pasted here: prose / images would create new files (a different feature).
  if (content.kind === "paths") {
    await A.paste(0);
    for (let i = 0; i < 100; i++) {
      await t.sleep(100);
      if (i > 5 && !Object.keys(t.getState().tasks).length) break;
    }
    await t.sleep(300);
  } else t.log(true, `paste: clipboard holds no files (${content.kind}); nothing pasted`);
  const after = (await t.api.listDir(dir, { showHidden: false, hideTemp: false })).map((e) => e.name);
  const added = after.filter((n) => !before.has(n));
  t.log(true, `paste: added to ${dir}: [${added.join(", ")}]`);
}

export const runCoreMode: ModeGroup = async (mode, t) => {
  const run = { "e2e-cli-state": cliState, "e2e-dbus-reveal": dbusReveal, "e2e-clip-copy": clipCopy, "e2e-clip-cut-paste": clipCutPaste, "e2e-clip-paste": clipPaste }[mode];
  if (!run) return false;
  const errors: string[] = [];
  window.addEventListener("error", (e) => errors.push(e.message));
  window.addEventListener("unhandledrejection", (e) => errors.push(String(e.reason)));
  try {
    await run(t);
  } catch (e) {
    t.log(false, `${mode} threw`, String(e));
  }
  t.log(!errors.length, `${mode}: no uncaught errors`, errors.join("; "));
  await t.finish();
  return true;
};
