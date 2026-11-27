import { getCurrentWebview } from "@tauri-apps/api/webview";
import { api } from "./api/ipc";
import { setStandaloneMergeDialogsForSelfTest } from "./components/StandaloneMerge";
import { join, parent } from "./core/path";
import { invoke } from "@tauri-apps/api/core";
import * as A from "./state/actions";
import { controller } from "./state/controllers";

export async function runMergeWorkspaceSelfTest(h: typeof import("./selftest").gitSelftest, repo: string) {
  try {
    await runWindowedMergeSelfTest(h, join(parent(repo) ?? repo, "windowed-merge"));
  } catch (error) {
    h.log(false, "Windowed Git merge scenario", String(error));
    await h.shot("windowed-merge-failure");
  } finally {
    A.openOverlay(null);
  }
  try {
    await runStandaloneMergeSelfTest(h, repo);
  } catch (error) {
    h.log(false, "Standalone merge scenario", String(error));
    await h.shot("standalone-merge-failure");
  }
}

async function runWindowedMergeSelfTest(h: typeof import("./selftest").gitSelftest, path: string) {
  const { $, $$, click, key, until, shot, log, typeInto } = h;
  const press = async (label: string, scope: ParentNode) => {
    const target = await until(() => $$<HTMLButtonElement>("button", scope).find((button) => {
      if (button.disabled) return false;
      const clone = button.cloneNode(true) as HTMLElement;
      clone.querySelectorAll("kbd,svg").forEach((node) => node.remove());
      return clone.textContent?.trim() === label;
    }));
    if (!target) throw new Error(`Missing windowed merge button ${label}`);
    click(target);
  };
  await h.updateSettings({ keyboardProfile: "vim", theme: "dark" });
  await h.setWindowSize(1320, 900);
  // The combined scenario can leave a Git workspace or confirmation open.
  A.openOverlay(null);
  if (!await until(() => !$(".git-graph"))) throw new Error("The previous Git workspace did not close");
  await A.openGitWorkspace(path);
  const conflict = await until(() => $$<HTMLButtonElement>(".gg-side-row:not(:disabled)").find((row) => row.textContent?.includes("windowed.txt")));
  if (!conflict) throw new Error("The 5,000-line merge fixture did not produce a conflict");
  click(conflict);
  const cards = await until(() => {
    const first = $(".gm-card[data-card='0']");
    const second = $(".gm-card[data-card='1']");
    return first && second && $$(".gm-card").length === 2 ? { first, second } : null;
  });
  if (!cards) throw new Error("The 5,000-line fixture did not show two separated conflicts");
  const { first, second } = cards;
  log(true, "windowed merge has two separated conflicts");
  const context = () => $$(".gm-card .gm-col").every((el) => {
    const code = $("pre", el)?.textContent ?? "";
    return code.trim().split("\n").length === 7;
  });
  log(context(), "each side shows three lines before and after its conflict");
  const gap = $(".gm-card[data-card='1']")?.previousElementSibling;
  if (gap?.classList.contains("gm-gap")) click(gap);
  const gapWindow = await until(() => $<HTMLElement>(".gm-gap-open .gm-win"));
  log(!!gapWindow, "a long hidden gap opens as a windowed list");
  if (gapWindow) {
    gapWindow.scrollIntoView({ block: "center" });
    gapWindow.focus();
    const start = gapWindow.scrollTop;
    key("j");
    log(!!await until(() => gapWindow.scrollTop > start && document.activeElement === gapWindow), "j scrolls the focused gap without moving to another conflict");
    const down = gapWindow.scrollTop;
    key("k");
    const returned = await until(() => gapWindow.scrollTop < down && document.activeElement === gapWindow);
    const outline = getComputedStyle(gapWindow).outlineStyle;
    log(!!returned && outline === "solid", "k scrolls back and the window shows its focus ring", `${down} → ${gapWindow.scrollTop}, outline=${outline}`);
    const long = await until(() => $$(".gm-win-cell", gapWindow).find((cell) => cell.textContent?.includes("long tail")));
    const selection = window.getSelection();
    const range = document.createRange();
    if (long) {
      long.scrollIntoView({ block: "center", inline: "start" });
      range.selectNodeContents(long);
      selection?.removeAllRanges();
      selection?.addRange(range);
    }
    log(!!long && gapWindow.scrollWidth > gapWindow.clientWidth && !!selection?.toString().includes("long tail"),
      "a long gap row scrolls sideways and its text can be selected");
    if (long && selection) {
      const selected = selection.toString();
      const rect = long.getBoundingClientRect();
      long.dispatchEvent(new MouseEvent("contextmenu", {
        button: 2, clientX: rect.left + 8, clientY: rect.top + 4, bubbles: true, cancelable: true,
      }));
      const item = await until(() => $$(".menu .mi:not(.disabled)").find((entry) => $(".lbl", entry)?.textContent === "Copy"));
      if (!item) throw new Error("Missing Copy for the selected merge row");
      click(item);
      let copied = false;
      for (let attempt = 0; attempt < 10 && !copied; attempt++) {
        const clipboard = await api.clipInspect();
        copied = clipboard.kind === "text" && clipboard.text === selected;
        if (!copied) await h.sleep(60);
      }
      log(copied, "Copy puts the complete selected merge row on the clipboard");
    }
    selection?.removeAllRanges();
    if (long) {
      const text = document.createTreeWalker(long, NodeFilter.SHOW_TEXT);
      let last: Text | null = null;
      for (let node = text.nextNode() as Text | null; node; node = text.nextNode() as Text | null) {
        if (node.length) last = node;
      }
      if (last) {
        range.setStart(last, last.length - 1);
        range.setEnd(last, last.length);
      }
      const scrollEdge = gapWindow.getBoundingClientRect().right;
      gapWindow.scrollLeft += range.getBoundingClientRect().right - scrollEdge + 32;
      log(!!await until(() => {
        const tail = range.getBoundingClientRect();
        const view = gapWindow.getBoundingClientRect();
        const cards = $(".gm-cards")!.getBoundingClientRect();
        return tail.width > 0 && tail.left >= view.left && tail.right <= view.right &&
          tail.top >= Math.max(view.top, cards.top) && tail.bottom <= Math.min(view.bottom, cards.bottom);
      }), "horizontal scrolling reveals the end of the long row");
    }
    await shot("workspace-merge-horizontal");
  }
  const editor = $<HTMLElement>(".gm")!;
  editor.focus();
  key("]");
  log(!!await until(() => second.classList.contains("active")), "] steps to the second conflict");
  key("[");
  log(!!await until(() => first.classList.contains("active")), "[ steps back to the first conflict");
  await press("Both full blocks", first);
  click($(".gm-left", second)!);
  const confirm = await until(() => {
    const button = $<HTMLButtonElement>(".gm-confirm", editor);
    return button && !button.disabled ? button : null;
  });
  log(!!confirm, "Both on one card and ours on the other make the full result available");
  await press("Full file", editor);
  const result = await until(() => $<HTMLElement>(".gm-result .gm-win"));
  if (!result || !confirm) throw new Error("The full merge result did not become available");
  const frame = () => new Promise<number>((resolve) => requestAnimationFrame(resolve));
  const percentile95 = (samples: number[]) => samples.slice().sort((a, b) => a - b)[Math.ceil(samples.length * .95) - 1];
  let idle = await frame();
  const idleIntervals: number[] = [];
  for (let sample = 0; sample < 30; sample++) {
    const now = await frame();
    idleIntervals.push(now - idle);
    idle = now;
  }

  const warmupFrames = 30;
  const measuredFrames = 120;
  let maxRows = 0;
  const scroll = async (first: number, count: number) => {
    let last = await frame();
    const intervals: number[] = [];
    for (let sample = first; sample < first + count; sample++) {
      // Match the 96 px steps used by the T01 text comparison scenario.
      result.scrollTop = sample * 96;
      const now = await frame();
      maxRows = Math.max(maxRows, $$(".gm-win-row", result).length);
      intervals.push(now - last);
      last = now;
    }
    return intervals;
  };
  await scroll(0, warmupFrames);
  const recording = await api.selftestFrameTrace(true);
  let frames: Awaited<ReturnType<typeof api.selftestFrameTrace>> = null;
  let intervals: number[];
  try {
    intervals = await scroll(warmupFrames, measuredFrames);
  } finally {
    if (recording !== null) frames = await api.selftestFrameTrace(false);
  }
  const webP95 = percentile95(intervals);
  const idleP95 = percentile95(idleIntervals);
  log(maxRows > 0 && maxRows <= 64, "full 5,000-line result keeps rendered rows bounded", `${maxRows} rows`);
  if (frames === null) {
    log(webP95 <= 16.7, "windowed merge result meets the T01 60 Hz frame budget",
      `WebKit p95=${webP95.toFixed(2)} ms, idle p95=${idleP95.toFixed(2)} ms`);
  } else {
    // Several paints can be latched at the same physical refresh. Count it once.
    const presented = [...new Set(frames.map(([timestamp]) => timestamp))].sort((a, b) => a - b);
    const presentedIntervals = presented.slice(1).map((timestamp, i) => (timestamp - presented[i]) / 1e6);
    const p95 = presentedIntervals.length ? percentile95(presentedIntervals) : Infinity;
    const refresh = frames[0]?.[1] / 1e6;
    const onReference = Math.abs(refresh - 1000 / 60) <= 0.05 &&
      frames.every(([, period]) => period === frames[0][1]);
    log(onReference && presented.length >= Math.ceil(measuredFrames * 0.95) && p95 <= 16.7,
      "windowed merge result meets the T01 60 Hz frame budget",
      `presented p95=${p95.toFixed(3)} ms, ${presented.length}/${measuredFrames} frames at ${(1000 / refresh).toFixed(2)} Hz, WebKit p95=${webP95.toFixed(2)} ms (idle ${idleP95.toFixed(2)} ms)`);
  }
  result.scrollTop = result.scrollHeight;
  log(!!await until(() => $$(".gm-win-cell", result).some((cell) => cell.textContent?.includes("windowed line 5000"))),
    "scrolling the full result reaches the last line");
  await shot("workspace-merge-full-file");

  editor.focus();
  key("e");
  const text = await until(() => $<HTMLTextAreaElement>("textarea[aria-label='Resolved file']", editor));
  if (!text) throw new Error("Missing result editor");
  const expected = Array.from({ length: 5000 }, (_, i) => `windowed line ${String(i + 1).padStart(4, "0")}`);
  expected[17] = "wide " + "x".repeat(1024) + " long tail";
  expected[2499] = "remote clean edit";
  expected[3499] = "local conflict two";
  expected.splice(9, 1, "local conflict one", "remote conflict one");
  const composed = expected.join("\n") + "\n";
  log(text.value === composed, "the complete result keeps Both, ours and the distant clean edit");
  expected[expected.length - 1] = "manual last line";
  const wanted = expected.join("\n") + "\n";
  typeInto(text, text.value.replace(/windowed line 5000\n$/, "manual last line\n"));
  await press("Done editing", editor);
  await press("Confirm", editor);
  const operation = await until(() => $(".gg-operation"));
  if (!operation) throw new Error("Missing Git merge operation");
  await press("Continue", operation);
  await until(() => !$(".gg-operation"), 12000);
  const saved = await invoke<{ changes: { path: string }[] }>("git_workspace", { path });
  log(!saved.changes.some((change) => change.path === "windowed.txt") &&
    (await h.api.readTextFile(join(path, "windowed.txt"))).text === wanted,
    "the staged result completes the merge and matches every expected line");
}

export async function runStandaloneMergeSelfTest(h: typeof import("./selftest").gitSelftest, repo: string) {
  const { api, $, $$, click, key, until, sleep, shot, log, typeInto } = h;
  const before = h.activePath(0);
  const size = { width: innerWidth, height: innerHeight };
  const zoom = (window as unknown as { __selftestZoom?: number }).__selftestZoom ?? 1;
  const settings = h.getState().settings!;
  const fixture = await api.createDir(repo, "standalone merge fixture");
  const base = join(fixture, "00-base.txt"), local = join(fixture, "10-local.txt"), remote = join(fixture, "20-remote.txt");
  const result = join(fixture, "merged result.txt");
  const alias = fixture + "/./10-local.txt";
  let savePath = result, saves = 0, browses = 0;
  const resetDialogs = setStandaloneMergeDialogsForSelfTest({
    open: async () => { browses++; return base; },
    save: async () => { saves++; return savePath; },
  });
  const panel = () => $<HTMLElement>(".sm");
  const field = (label: string) => $$<HTMLInputElement>(".sm .sm-row input").find((input) =>
    Array.from(input.labels ?? []).some((item) => item.textContent?.trim() === label));
  const button = (label: string) => $$<HTMLButtonElement>(".sm button").find((item) => item.textContent?.trim() === label);
  const pick = async (label: string) => {
    const target = await until(() => { const candidate = button(label); return candidate && !candidate.disabled ? candidate : null; });
    if (!target) throw new Error(`Missing or disabled standalone merge button: ${label}`);
    click(target);
  };
  const confirm = () => $<HTMLButtonElement>(".sm .gm .gm-confirm");
  const waitConfirm = () => until(() => { const control = confirm(); return control && !control.disabled ? control : null; });
  const close = async () => {
    const view = panel();
    if (view) key("Escape", {}, view);
    if (!await until(() => !panel())) throw new Error("Standalone merge did not close");
  };
  try {
    A.openOverlay(null);
    await h.setWindowSize(1320, 900);
    await getCurrentWebview().setZoom(1);
    await h.updateSettings({ keyboardProfile: "vim", leaderKey: "Space" });
    await api.saveText(base, "local standalone\r\n");
    await api.saveText(local, "local standalone\r\n");
    await api.saveText(remote, "remote standalone\r\n");
    A.navigate(0, fixture);
    const browser = await until(() => {
      const current = controller(0);
      return current?.entries().some((entry) => entry.path === remote) ? current : null;
    });
    if (!browser) throw new Error("Standalone merge fixture was not listed");
    browser.select([local, remote], local);
    if (!await until(() => browser.selection().length === 2)) throw new Error("Both merge inputs could not be selected");
    const [shownLocal, shownRemote] = browser.selection().map((entry) => entry.path);
    const expectedLocal = (await api.readTextFile(shownLocal)).text;
    await h.focusPane(0);
    for (const input of [" ", "b", "m"]) { key(input); await sleep(80); }
    log(!!await until(() => panel() && field("Local")?.value === shownLocal && field("Remote")?.value === shownRemote),
      "Space b m opens the standalone merge with two selected files");
    log($<HTMLInputElement>(".sm .sm-nobase input")?.checked === true, "two selected files default to an empty base");
    await shot("standalone-merge-pick-100");

    await pick("Merge");
    if (!await until(() => $(".sm .gm-card"))) throw new Error("Expected a standalone add/add conflict");
    log(confirm()?.disabled === true, "saving an unresolved standalone result is disabled");
    const editor = $<HTMLElement>(".sm .gm")!;
    editor.focus();
    key("Enter", { ctrlKey: true });
    await sleep(160);
    log(saves === 0 && !(await api.pathResolve(result)).exists, "Ctrl+Enter on an unresolved merge writes nothing");
    await shot("standalone-merge-conflict-100");
    click($(".sm .gm-card .gm-left")!);
    await waitConfirm();
    await pick("Confirm");
    log(!!await until(() => $(".sm-out[role='status']")) &&
      (await api.readTextFile(result)).text === expectedLocal,
      "a resolved standalone merge saves a new file with its CRLF lines");
    await close();

    await getCurrentWebview().setZoom(1.5);
    A.openStandaloneMerge();
    await until(panel);
    const noBase = $(".sm .sm-nobase input");
    if (!noBase) throw new Error("Missing no-base option");
    click(noBase);
    const baseField = field("Base");
    const browse = baseField?.parentElement?.querySelector("button");
    if (!browse) throw new Error("Missing Browse for the base file");
    click(browse);
    log(!!await until(() => field("Base")?.value === base && browses === 1), "Browse fills the base path");
    typeInto(field("Local")!, local);
    typeInto(field("Remote")!, remote);
    await shot("standalone-merge-pick-150");
    await pick("Merge");
    await waitConfirm();
    log(!!$(".sm .gm-status")?.textContent?.includes("No conflicts"), "a base matching Local yields a clean merge");
    await shot("standalone-merge-clean-150");
    savePath = alias;
    await pick("Confirm");
    log(!!await until(() => $(".sm-out[role='status']")) && (await api.readTextFile(local)).text === "remote standalone\r\n",
      "an unchanged input can be saved through a canonical path alias");
    await close();

    await api.saveText(local, "local standalone\r\n");
    A.openStandaloneMerge([base, local, remote]);
    await until(panel);
    await pick("Merge");
    await waitConfirm();
    const newer = "External edit after loading the merge\r\n";
    await api.saveText(local, newer);
    await pick("Confirm");
    const refused = await until(() => $(".sm [role='alert']"));
    log(!!refused && (await api.readTextFile(local)).text === newer, "saving through an alias refuses an input changed after loading");
    await shot("standalone-merge-changed-target-150");
    await close();

    A.openStandaloneMerge();
    await until(panel);
    const focused = field("Local");
    focused?.focus();
    key("Escape", {}, focused);
    log(!!await until(() => !panel()), "Escape from a standalone file path closes the dialog");
  } finally {
    resetDialogs();
    A.openOverlay(null);
    await getCurrentWebview().setZoom(zoom);
    await h.setWindowSize(size.width, size.height);
    await h.updateSettings({ keyboardProfile: settings.keyboardProfile, leaderKey: settings.leaderKey });
    A.navigate(0, before);
  }
}
