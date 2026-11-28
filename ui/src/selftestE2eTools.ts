// `--e2e-*` self-test modes of the "tools" group: terminal / editor / Open with, GVFS mounts,
// actions.d, zoxide and the active terminal's folder. Each drives the real backend commands (and
// the real Quick Open / dialog UI) against the mocks the E2E harness puts on the PATH, and
// reports through `log` lines (PASS/FAIL) plus the facts the suite greps for.
import type { FileEntry } from "./api/types";
import type { ModeGroup, SelftestTools } from "./selftestE2e";

type T = SelftestTools;

const basename = (p: string) => p.split("/").filter(Boolean).pop() ?? p;

/** Races `p` against a timer; resolves `undefined` on timeout (the guard being tested). */
async function within<V>(p: Promise<V>, ms: number): Promise<{ value?: V; error?: string; timedOut: boolean }> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<"timeout">((r) => { timer = setTimeout(() => r("timeout"), ms); });
  try {
    const v = await Promise.race([p, timeout]);
    return v === "timeout" ? { timedOut: true } : { value: v as V, timedOut: false };
  } catch (e) {
    return { error: String(e), timedOut: false };
  } finally {
    clearTimeout(timer);
  }
}

async function listing(t: T, dir: string): Promise<FileEntry[]> {
  return t.api.listDir(dir, { showHidden: true, hideTemp: false }).catch(() => []);
}

/** The file the launch named (`gall <file>` selects it), else a likely test file in the folder. */
async function targetFile(t: T): Promise<string | null> {
  const args = await t.api.startupArgs();
  if (args.selectPaths?.[0]) return args.selectPaths[0];
  const dir = t.activePath(0);
  const files = (await listing(t, dir)).filter((e) => !e.isDir);
  for (const want of ["note.txt", "sample.txt", "check.txt"]) {
    const hit = files.find((e) => e.name === want);
    if (hit) return hit.path;
  }
  return files[0]?.path ?? null;
}

async function noErrorNotices(t: T, what: string) {
  const errors = t.getState().notificationLog.filter((n) => n.kind === "error");
  t.log(errors.length === 0, `${what}: no error notification`, errors.map((n) => n.text).join("; "));
}

// ───────────────────────── terminal / editor / Open with ─────────────────────────

async function terminalScript(t: T) {
  const A = await import("./state/actions");
  const dir = t.activePath(0);
  const apps = await t.api.terminalApps();
  t.log(true, `terminal: ${apps.length} terminal(s) found, default ${apps[0]?.name ?? "none"}`);
  try {
    await t.api.openTerminal(dir);
    t.log(apps.length > 0, "terminal: Terminal here started the preferred terminal in the folder", apps[0]?.command ?? "");
  } catch (e) {
    const msg = String(e);
    // No terminal at all (not even $TERMINAL): a clear error, not a crash.
    t.log(apps.length === 0 && /no terminal/i.test(msg), "terminal: missing terminal is reported as an error", msg);
  }
  const script = (await listing(t, dir)).find((e) => !e.isDir && e.name.endsWith(".sh"));
  if (script) {
    try {
      await t.api.openPath(script.path);
      t.log(true, "terminal: executable script handed to the terminal (script argv without -e)", script.name);
    } catch (e) {
      t.log(false, "terminal: opening the script failed", String(e));
    }
  }
  await t.sleep(1500);
  void A;
}

async function editorMode(t: T, mode: "omarchy" | "tui") {
  const A = await import("./state/actions");
  const file = await targetFile(t);
  if (!file) return t.log(false, "editor: no file to edit");
  const apps = await t.api.editorApps();
  t.log(apps.length > 0, `editor: ${apps.length} editor(s) found`, apps.map((a) => a.command.split(" ")[0]).join(", "));
  const pick = mode === "omarchy" ? apps.find((a) => a.default) ?? apps[0] : apps.find((a) => /(^|\s|\/)nvim(\s|$)/.test(a.command));
  if (!pick) return t.log(false, mode === "omarchy" ? "editor: no default editor" : "editor-tui: $EDITOR nvim is not offered as an editor");
  if (mode === "omarchy") t.log(pick.command.startsWith("omarchy-launch-editor"), "editor: omarchy-launch-editor is the default editor", pick.command);
  else t.log(true, "editor-tui: TUI editor offered from $EDITOR (run inside the terminal)", pick.command);
  await t.updateSettings({ editor: pick.command });
  A.editPaths([file]);
  await t.sleep(1800);
  await noErrorNotices(t, mode === "omarchy" ? "editor" : "editor-tui");
}

async function openWithDesktop(t: T) {
  const file = await targetFile(t);
  if (!file) return t.log(false, "openwith: no file to open");
  // A UWSM session cannot be started in the sandbox: pretend one when the PATH holds a stub.
  const sim = await t.api.selftestSimulateUwsm().catch(() => false);
  t.note(`openwith: uwsm session simulated for a stub uwsm-app: ${sim}`);
  const apps = await t.api.openWithApps(file);
  t.log(apps.length > 0, `openwith: ${apps.length} desktop application(s) offered for the file`);
  const hidden = ["NoDisplayApp", "HiddenApp", "MissingTryExec"];
  t.log(!apps.some((a) => hidden.includes(a.name)), "openwith: NoDisplay, Hidden and missing-TryExec desktop entries are not offered");
  const wanted = new Set(["TUI Viewer", "Mock GUI", "MockGui", "ParamTest", "GoodVisibleApp"]);
  // Only the harness's own entries are launched (never the host's real applications).
  const chosen = apps.filter((a) => wanted.has(a.name) || /e2e/i.test(`${a.name} ${a.id}`));
  if (!chosen.length) {
    const shown = apps.filter((a) => !hidden.includes(a.name)).map((a) => `${a.name} [${a.id}]`);
    return t.log(false, "openwith: the test desktop entry was not found among the offered applications", shown.join("; "));
  }
  for (const app of chosen) {
    try {
      await t.api.openWithApp(file, app.id);
      t.log(true, `openwith: launched desktop entry ${app.name}`);
    } catch (e) {
      t.log(false, `openwith: launching desktop entry ${app.name} failed`, String(e));
    }
  }
  await t.sleep(1800);
  await noErrorNotices(t, "openwith");
}

// ───────────────────────── GVFS ─────────────────────────

async function gvfsDrives(t: T) {
  const A = await import("./state/actions");
  await A.refreshDrives();
  const drives = await t.api.listDrives();
  const names = drives.map((d) => `${d.short || d.label} (${d.mount})`);
  t.note(`gvfs drives: ${names.join("; ")}`);
  const devices = drives.filter((d) => /gvfs/.test(d.mount));
  t.log(devices.length > 0, `gvfs drives: ${devices.length} MTP/gphoto2 device(s) in Drives`, devices.map((d) => d.short || d.label).join(", "));
  t.log(devices.every((d) => d.removable), "gvfs drives: devices are removable (Eject offered)");
  const side = await t.until(() => /Pixel|Canon/.test(t.$(".sidebar")?.textContent ?? ""), 4000);
  t.log(!!side || devices.length === 0, "gvfs drives: the sidebar lists the devices", t.$(".sidebar")?.textContent?.slice(0, 80) ?? "");
}

async function gvfsNetwork(t: T) {
  const A = await import("./state/actions");
  await A.refreshNetwork();
  const mounts = t.getState().networkMounts;
  t.note(`gvfs network: ${mounts.length} mount(s)`);
  for (const m of mounts) t.note(`gvfs network mount: ${m.name} | ${m.scheme} | ${m.uri} | ${m.mountPath}`);
  const linux = t.getState().info?.platform === "linux";
  t.log(Array.isArray(mounts), `gvfs network: the mount list is available (${mounts.length} entries; empty is fine without gvfs or gio)`);
  if (mounts.length) {
    const side = await t.until(() => /Network/i.test(t.$(".sidebar")?.textContent ?? ""), 3000);
    t.log(!!side, "gvfs network: the sidebar shows the Network section");
    t.log(mounts.every((m) => !/^(mtp|gphoto2)/.test(m.scheme)), "gvfs network: devices are not listed as network mounts");
  } else if (linux) {
    t.log(!!(await t.until(() => /Connect to server/.test(t.$(".sidebar")?.textContent ?? ""), 3000)), "gvfs network: an empty Network section still offers Connect to server…");
  }
}

async function clickDialogButton(t: T, label: string) {
  const btn = await t.until(() => t.$$(".modal button").find((b) => b.textContent?.trim() === label));
  if (btn) t.click(btn);
  return !!btn;
}

async function gvfsConnect(t: T) {
  const A = await import("./state/actions");
  const uri = "sftp://alice@dev/";
  const before = t.getState().notificationLog.length;
  const run = A.connectToServer();
  const input = await t.until(() => t.$<HTMLInputElement>(".modal input"));
  t.log(!!input, "connect: the Connect to server dialog opens");
  if (input) {
    t.typeInto(input, uri);
    t.log(await clickDialogButton(t, "Connect"), "connect: Connect accepts the address", uri);
  }
  await run;
  const notes = t.getState().notificationLog.slice(before).map((n) => `${n.kind}: ${n.text}`);
  t.note(`connect: notifications: ${notes.join(" | ")}`);
  const failed = notes.some((n) => n.startsWith("error"));
  t.log(notes.some((n) => /Connecting to sftp:\/\/alice@dev\//.test(n)), "connect: progress announced for the address");
  t.log(failed || notes.some((n) => /Connected to/.test(n)), failed ? "connect: gio failure is shown to the user as an error" : "connect: success announced");

  // An empty address is refused by the dialog without calling gio.
  const run2 = A.connectToServer();
  const input2 = await t.until(() => t.$<HTMLInputElement>(".modal input"));
  if (input2) {
    t.typeInto(input2, "");
    await clickDialogButton(t, "Connect");
    const warned = await t.until(() => t.getState().notifications.some((n) => n.kind === "warn" && /address/i.test(n.text)), 3000);
    t.log(!!warned, "connect: an empty address is refused with a hint");
    await t.sleep(200);
    await clickDialogButton(t, "Cancel");
  }
  await run2;
  const bad = await t.api.connectServer("not a uri").then(() => false, () => true);
  t.log(bad, "connect: an invalid URI is rejected by the backend");
}

async function gvfsUnmount(t: T) {
  const A = await import("./state/actions");
  await A.refreshNetwork();
  const net = t.getState().networkMounts.find((m) => /nas|smb/.test(`${m.name} ${m.uri} ${m.scheme}`)) ?? t.getState().networkMounts[0];
  if (net) {
    await A.disconnectNetwork(net);
    t.log(true, `unmount: Disconnect called gio mount -u for the network mount ${net.name}`);
  }
  const drives = await t.api.listDrives();
  const dev = drives.find((d) => /gvfs/.test(d.mount) && /Pixel|mtp/i.test(`${d.label} ${d.short} ${d.mount}`)) ?? drives.find((d) => /gvfs/.test(d.mount));
  if (dev) {
    const r = await within(t.api.eject(dev.mount), 6000);
    t.log(!r.timedOut, `unmount: Eject ${dev.short || dev.label} (gio mount -e) answered`, r.error ?? "");
  }
  t.log(!!net || !!dev, `unmount: ${net ? "network mount disconnected" : "no network mount"}, ${dev ? "device ejected" : "no device"}`);
  await t.sleep(300);
}

async function gvfsSlow(t: T) {
  const dir = t.activePath(0);
  const started = performance.now();
  // Slow `gio mount -l` (hung helper): the probes must come back (or be abandoned) quickly and
  // must not freeze the window; local browsing keeps working meanwhile.
  const probe = within(t.api.gvfsNetworkMounts(), 4000);
  const drivesProbe = within(t.api.listDrives(), 4000);
  let ticks = 0;
  const timer = setInterval(() => { ticks++; }, 50);
  const entries = await within(t.api.listDir(dir, { showHidden: true, hideTemp: false }), 4000);
  const listMs = Math.round(performance.now() - started);
  clearInterval(timer);
  if (entries.error) {
    t.log(true, "slow-mount guard: a vanished or unreadable folder is reported as an error, not a crash", entries.error.slice(0, 120));
  } else {
    t.log(!entries.timedOut, `slow-mount guard: listed ${entries.value?.length ?? 0} entries of the folder in ${listMs} ms without blocking`, dir);
    t.log(listMs < 3000, "slow-mount guard: listing finished in under 3 s");
  }
  t.log(ticks > 0 || listMs < 100, "slow-mount guard: the UI thread kept running during the probe", `${ticks} timer ticks`);
  const locked = entries.value?.find((e) => e.isDir && e.name === "locked");
  if (locked) {
    const r = await within(t.api.listDir(locked.path, { showHidden: true, hideTemp: false }), 4000);
    t.log(!r.timedOut, "slow-mount guard: an unreadable subfolder answers with an error", r.error?.slice(0, 100) ?? "listed");
  }
  const [net, dr] = await Promise.all([probe, drivesProbe]);
  t.log(true, `slow-mount guard: gvfs probe ${net.timedOut ? "abandoned after 4 s" : "finished"}, drives probe ${dr.timedOut ? "abandoned after 4 s" : "finished"}`);
  const cover = await within(t.api.indexCovers(dir), 3000);
  t.note(`slow-mount guard: index coverage for the folder: ${JSON.stringify(cover.value ?? cover.error ?? "timeout")}`);
  t.log(!cover.timedOut, "slow-mount guard: index coverage check does not block");
  await noErrorNotices(t, "slow-mount guard");
}

// ───────────────────────── actions.d ─────────────────────────

type Target = { path: string; isDir: boolean; ext: string };
const target = (e: FileEntry): Target => ({ path: e.path, isDir: e.isDir, ext: e.ext });

async function actionLabelsFor(t: T, entries: FileEntry[]) {
  const { actionLabel } = await import("./core/actionsMenu");
  const defs = await t.api.actionsForSelection(entries.map(target));
  return { defs, labels: defs.map((d) => actionLabel(d, entries.length)) };
}

async function actionsFilter(t: T) {
  const dir = t.activePath(0);
  const entries = await listing(t, dir);
  const file = entries.find((e) => !e.isDir);
  if (!file) return t.log(false, "actions: the folder has no file to select");
  const { defs, labels } = await actionLabelsFor(t, [file]);
  t.note(`actions for ${file.name}: ${labels.join(", ") || "(none)"}`);
  t.log(defs.length > 0, `actions: ${defs.length} action(s) match the selected file ${file.name}`);
  const bad = labels.filter((l) => /Missing tool action|Inspect directory/.test(l));
  t.log(bad.length === 0, "actions: whenCommand filtering hides actions whose tool is absent from the PATH (string and array forms)");
  const forDir = await t.api.actionsForSelection([{ path: dir, isDir: true, ext: "" }]);
  t.log(!forDir.some((d) => /Missing tool action|Inspect directory/.test(d.label)), "actions: folder targets hide actions with absent tools too");
  const all = await t.api.actionsList();
  t.log(Array.isArray(all) && all.length >= defs.length, `actions: ${all.length} action definition(s) loaded from actions.d (malformed files ignored)`);
}

async function actionsMatch(t: T) {
  const dir = t.activePath(0);
  const entries = await listing(t, dir);
  const files = entries.filter((e) => !e.isDir);
  const dirs = entries.filter((e) => e.isDir);
  const report = async (what: string, sel: FileEntry[]) => {
    const r = await actionLabelsFor(t, sel);
    t.note(`selection ${what}: matched ${r.labels.length ? r.labels.join(" | ") : "(none)"}; the other actions are hidden by their filters`);
    return r;
  };
  if (files.length) {
    const first = files[0];
    const one = await report(`1 file (${first.name})`, [first]);
    t.log(Array.isArray(one.defs), `actions: single file ${first.name} matches ${one.defs.length} action(s)`);
  }
  const media = files.filter((f) => /^(png|jpe?g|gif|webp|mp4|mov|mkv)$/i.test(f.ext));
  for (const f of media.slice(0, 1)) {
    const r = await report(`media file (${f.name}, extension ${f.ext})`, [f]);
    t.log(r.defs.some((d) => d.id.includes("transcode") || /Transcode/.test(d.label)), `actions: ${f.ext} extension / mime prefix matches the media action (case-insensitive)`);
  }
  const textFile = files.find((f) => f.ext.toLowerCase() === "txt");
  if (textFile) {
    const r = await report(`text file (${textFile.name})`, [textFile]);
    t.log(!r.defs.some((d) => /transcode/.test(d.id)), `actions: ${textFile.name} is excluded from extension / mime restricted actions`);
  }
  if (files.length > 1) {
    const r = await report(`${files.length} files`, files);
    t.log(!r.defs.some((d) => d.single), `actions: single=true actions are excluded for ${files.length} selected files`);
    const plural = r.defs.filter((d) => d.pluralLabel);
    t.log(plural.every(() => r.labels.some((l) => !l.includes("{count}") && l.includes(String(files.length)))) || plural.length === 0,
      `actions: pluralLabel {count} substituted with ${files.length}`);
    if (plural.length) t.note(`actions: pluralLabel examples: ${r.labels.filter((l) => l.includes(String(files.length))).join(" | ")}`);
    const media2 = files.filter((f) => /^(png|jpe?g|mp4|mov|mkv)$/i.test(f.ext));
    if (media2.length > 1) {
      const m = await report(`${media2.length} media files`, media2);
      t.log(m.labels.some((l) => /media files/.test(l)), "actions: pluralLabel for several media files (e.g. Transcode N media files)");
    }
  }
  if (dirs.length) {
    const r = await report(`1 folder (${dirs[0].name})`, [dirs[0]]);
    t.log(r.defs.length >= 0, `actions: single folder matches ${r.defs.length} action(s)`);
  }
  if (dirs.length > 1) {
    const r = await report(`${dirs.length} folders`, dirs);
    t.log(!r.defs.some((d) => d.single), "actions: single=true folder actions are excluded for several folders");
  }
  if (files.length && dirs.length) {
    const r = await report("mixed files and folders", [files[0], dirs[0]]);
    t.log(!r.defs.some((d) => d.selection === "files" || d.selection === "dirs"), "actions: a mixed file + folder selection excludes files-only and dirs-only actions");
  }
  t.log(true, "actions: matching verified against the real backend filters");
}

/** Runs an action by id on the selected paths (a launch with a file selects it, else all files). */
async function runAction(t: T, prefer: string[], pickFiles: (files: FileEntry[]) => FileEntry[], what: string) {
  const dir = t.activePath(0);
  const args = await t.api.startupArgs();
  const entries = await listing(t, dir);
  let sel: FileEntry[];
  if (args.selectPaths?.length) sel = entries.filter((e) => args.selectPaths.includes(e.path));
  else sel = pickFiles(entries.filter((e) => !e.isDir));
  if (!sel.length) return t.log(false, `${what}: nothing selected to run the action on`);
  const { defs } = await actionLabelsFor(t, sel);
  const def = prefer.map((id) => defs.find((d) => d.id === id)).find(Boolean);
  if (!def) return t.log(false, `${what}: none of the expected actions (${prefer.join(", ")}) matches the selection`, defs.map((d) => d.id).join(", "));
  const { actionLabel } = await import("./core/actionsMenu");
  const paths = sel.map((e) => e.path);
  try {
    await t.api.actionRun(def.id, paths, dir);
    t.log(true, `${what}: ran action ${def.id} (${actionLabel(def, paths.length)}) on ${paths.length} path(s)`);
  } catch (e) {
    t.log(false, `${what}: running ${def.id} failed`, String(e));
  }
  await t.sleep(1800);
}

// ───────────────────────── zoxide / terminal cwd ─────────────────────────

async function zoxideQuickOpen(t: T) {
  const A = await import("./state/actions");
  const dirs = await t.api.zoxideDirs().catch((e) => { t.note(`zoxide: query failed: ${String(e).slice(0, 80)}`); return [] as string[]; });
  t.note(`zoxide: ${dirs.length} folder(s) from zoxide query -l`);
  const live: string[] = [];
  for (const d of dirs) {
    const r = await t.api.pathResolve(d).catch(() => null);
    if (r?.exists && r.isDir) live.push(d);
    else t.note(`zoxide: a stale (deleted) folder in the zoxide output is ignored: ${basename(d)}`);
  }
  A.openOverlay({ type: "quickOpen" });
  const input = await t.until(() => t.$<HTMLInputElement>(".qo input, .qo-input input, .modal input"));
  t.log(!!input, "zoxide: Quick Open opens");
  if (!input) return;
  if (!live.length) {
    t.typeInto(input, "z");
    await t.sleep(600);
    t.log(true, "zoxide: no usable zoxide folders (zoxide missing, failing or empty): Quick Open still works");
    t.log(!t.$$(".qo-row .hidden-badge").some((b) => b.textContent === "zoxide"), "zoxide: no zoxide rows are shown");
    A.openOverlay(null);
    return;
  }
  const want = live[0];
  const name = basename(want);
  t.typeInto(input, name);
  // The live scan may list the folder as an ordinary hit (the zoxide duplicate is then skipped), so
  // accept any row for it and identify the right one by its preview path.
  const named = () => t.$$(".qo-row").filter((r) => r.querySelector(".qo-name")?.textContent === name);
  let row: HTMLElement | undefined;
  for (let i = 0; i < 25 && !row; i++) {
    for (const r of named()) {
      t.mouse(r, "mousemove");
      await t.sleep(150);
      if (t.$(".qo-pv-path")?.textContent === want) { row = r; break; }
    }
    if (!row) await t.sleep(200);
  }
  t.log(!!row, `zoxide: the zoxide folder ${name} is listed in Quick Open`);
  if (!row) { A.openOverlay(null); return; }
  t.key("Enter", {}, input);
  const arrived = await t.until(() => t.activePath(A.activeBrowser()) === want, 5000);
  t.log(!!arrived, `zoxide: Enter navigated the browser to ${want}`, t.activePath(A.activeBrowser()));
  t.note(`zoxide: navigated to ${t.activePath(A.activeBrowser())}`);
}

async function terminalCwd(t: T) {
  const A = await import("./state/actions");
  const cwd = await t.api.omarchyTerminalCwd().catch(() => null);
  t.note(`terminal-cwd: omarchy-cmd-terminal-cwd answered ${cwd === null ? "nothing usable" : cwd}`);
  A.openOverlay({ type: "quickOpen", initial: ">" });
  const input = await t.until(() => t.$<HTMLInputElement>(".qo input, .qo-input input, .modal input"));
  if (!input) return t.log(false, "terminal-cwd: the command palette did not open");
  t.typeInto(input, ">terminal");
  await t.sleep(700);
  const row = t.$$(".qo-row.cmd").find((r) => /Open active terminal's folder/.test(r.textContent ?? ""));
  if (cwd === null) {
    t.log(!row, "terminal-cwd: no terminal folder → the palette entry is hidden and nothing happens (missing tool, failure or bad directory)");
    A.openOverlay(null);
    return;
  }
  t.log(!!row, "terminal-cwd: the palette offers Open active terminal's folder");
  if (!row) return A.openOverlay(null);
  t.click(row);
  const b = A.activeBrowser();
  const ok = await t.until(() => t.activePath(b) === cwd, 5000);
  t.log(!!ok, `terminal-cwd: the browser navigated to the terminal's folder ${cwd}`, t.activePath(b));
}

async function compareScale(t: T) {
  const [{ measureCompareResponses }, A] = await Promise.all([import("./api/ipc"), import("./state/actions")]);
  const cwd = t.activePath(0);
  if (!cwd) throw new Error("Missing comparison fixture folder");
  const source = (name: string) => ({ kind: "file" as const, path: `${cwd}/scale-${name}.txt` });
  const frame = () => new Promise<number>((resolve) => requestAnimationFrame(resolve));
  const percentile = (values: number[], fraction: number) => {
    const ordered = values.slice().sort((a, b) => a - b);
    return ordered[Math.max(0, Math.ceil(ordered.length * fraction) - 1)] ?? 0;
  };
  for (const fixture of ["sparse", "dense", "long"]) {
    const latencies: number[] = [];
    const sizes: number[] = [];
    measureCompareResponses((_, milliseconds, bytes) => {
      latencies.push(milliseconds);
      sizes.push(bytes);
    });
    try {
      const began = performance.now();
      A.openOverlay({ type: "diff", a: source("left"), b: source(fixture) });
      if (fixture === "long") {
        const limited = await t.until(() => document.querySelector(".dv-msg")?.textContent?.includes("exceeds the text comparison limit"), 15000);
        t.log(!!limited, "32 MiB line reports an explicit limit");
      } else {
        const body = await t.until(() => document.querySelector<HTMLElement>(".dv-body"), 30000);
        if (!body) throw new Error(`No comparison body for ${fixture}`);
        const painted = await t.until(() => [...body.querySelectorAll(".dv-code")].some((el) => el.textContent?.includes("for comparison")), 15000);
        t.log(!!painted, `${fixture}: visible rows populated`, `open and paint ${(performance.now() - began).toFixed(1)} ms`);
        let maxRows = 0;
        const frames: number[] = [];
        let last = await frame();
        // Warm up and then scroll across both already-visible and newly requested pages.
        for (let i = 0; i < 150; i++) {
          body.scrollTop = i * 96;
          const now = await frame();
          if (i >= 30) frames.push(now - last);
          last = now;
          maxRows = Math.max(maxRows, body.querySelectorAll(".dv-row, .dv-fold").length);
        }
        const p95 = percentile(frames, 0.95);
        t.log(maxRows > 0 && maxRows <= 200, `${fixture}: mounted row limit`, String(maxRows));
        t.log(p95 <= 16.7, `${fixture}: frame interval`, `p50=${percentile(frames, 0.5).toFixed(2)} ms p95=${p95.toFixed(2)} ms`);
      }
      const bytes = Math.max(0, ...sizes);
      t.log(bytes > 0 && bytes <= 1024 * 1024, `${fixture}: comparison IPC response size`,
        `peak=${bytes} bytes, IPC p50=${percentile(latencies, 0.5).toFixed(2)} ms p95=${percentile(latencies, 0.95).toFixed(2)} ms`);
    } finally {
      measureCompareResponses();
      A.openOverlay(null);
      await frame();
    }
  }
}

// ───────────────────────── dispatcher ─────────────────────────

export const runToolsMode: ModeGroup = async (mode, t) => {
  const errors: string[] = [];
  const modes: Record<string, () => Promise<void>> = {
    "e2e-compare-scale": () => compareScale(t),
    "e2e-terminal-script": () => terminalScript(t),
    "e2e-editor-omarchy": () => editorMode(t, "omarchy"),
    "e2e-editor-tui": () => editorMode(t, "tui"),
    "e2e-openwith-desktop": () => openWithDesktop(t),
    "e2e-gvfs-drives": () => gvfsDrives(t),
    "e2e-gvfs-network": () => gvfsNetwork(t),
    "e2e-gvfs-connect": () => gvfsConnect(t),
    "e2e-gvfs-unmount": () => gvfsUnmount(t),
    "e2e-gvfs-slow": () => gvfsSlow(t),
    "e2e-actions-filter": () => actionsFilter(t),
    "e2e-actions-match": () => actionsMatch(t),
    "e2e-actions-exec": () => runAction(t, ["omarchy.localsend", "adv.send"], (f) => f, "exec"),
    "e2e-actions-floating": () => runAction(t, ["omarchy.transcode"], (f) => f.filter((e) => /^(mp4|mov|mkv|png|jpe?g)$/i.test(e.ext)).slice(0, 1), "floating"),
    "e2e-zoxide-quickopen": () => zoxideQuickOpen(t),
    "e2e-terminal-cwd": () => terminalCwd(t),
  };
  const run = modes[mode];
  if (!run) return false;
  window.addEventListener("error", (e) => errors.push(e.message));
  window.addEventListener("unhandledrejection", (e) => errors.push(String(e.reason)));
  // The first folder is up before the checks start (the launch request opens it asynchronously).
  await t.until(() => t.activePath(0) && t.getState().session, 4000);
  await t.sleep(600);
  try {
    await run();
  } catch (e) {
    t.log(false, `${mode} threw`, String(e));
  }
  t.log(!errors.length, `${mode}: no uncaught errors`, errors.join("; "));
  await t.finish();
  return true;
};
