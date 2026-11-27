// The yardstick of the Git overhaul (scripts/selftest.sh --git-baseline, selftest-git.ps1 -Scenario git-baseline): the steps
// seven everyday tasks need with the keyboard and with the mouse, executed for real on the repositories the lazygit yardstick
// uses, and the latencies a user sees in the window, which the Rust benchmark (git_speed) cannot measure.
import { invoke } from "@tauri-apps/api/core";
import * as A from "./state/actions";
import type { Repository } from "./core/gitRepository";
import { contextMenuPoint, overBaseline, stepsText, type Baseline, type Steps } from "./core/gitBaseline";
import stored from "./gitBaseline.json";

const baseline = stored as Baseline;
const NEW_MESSAGE = "add three things", FEATURE_TIP = "feature: add feat-3", STASH = "wip: invoice export tweaks";

export async function runGitBaselineSelfTest(h: typeof import("./selftest").gitSelftest) {
  const { $, $$, until, sleep, shot, setWindowSize, log, note } = h;
  const platform = h.getState().info?.platform ?? "unknown";
  const root = h.activePath(0), sep = root.includes("\\") ? "\\" : "/";
  const at = (name: string) => `${root}${sep}repos${sep}${name}`;
  let steps: Steps = { keys: 0, typed: 0, clicks: 0 }, path = "";

  // ── input, counted ──
  const focused = () => {
    const active = document.activeElement;
    if (!active || active === document.body) throw new Error(`nothing has the keyboard focus after ${steps.keys} keys and ${steps.clicks} clicks, a user would have to click first`);
    return active;
  };
  /** One key press where the focus really is; Shift and Ctrl belong to the key. */
  const press = async (name: string, opts: KeyboardEventInit = {}) => { steps.keys++; h.key(name, opts, focused()); await sleep(90); };
  const type = async (text: string) => {
    const field = focused();
    if (!(field instanceof HTMLInputElement || field instanceof HTMLTextAreaElement)) throw new Error("typing without a focused field");
    steps.keys += text.length; steps.typed += text.length; h.typeInto(field, text); await sleep(90);
  };
  const need = async <T,>(find: () => T | null | undefined | false, what: string, ms = 8000): Promise<T> => { const found = await until(find, ms); if (!found) throw new Error(`missing: ${what}`); return found; };
  const click = async (find: () => Element | null | undefined, what: string) => {
    const element = await need(() => { const e = find(); return e && !(e as HTMLButtonElement).disabled ? e : null; }, what);
    steps.clicks++;
    element.scrollIntoView({ block: "nearest" });
    // A real click moves the focus to the nearest focusable element; a synthetic one does not.
    (element.closest("button,input,textarea,[tabindex]") as HTMLElement | null)?.focus();
    h.click(element); await sleep(120);
  };
  const rightClick = async (find: () => Element | null | undefined, what: string) => {
    const element = await need(find, what);
    steps.clicks++;
    element.scrollIntoView({ block: "nearest" });
    element.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, view: window, ...contextMenuPoint(element.getBoundingClientRect()) }));
    await need(() => $(".menu"), `menu of ${what}`);
  };
  const menuItem = (text: string) => $$<HTMLElement>(".menu .mi").find((item) => $(".lbl", item)?.textContent?.trim() === text && !item.classList.contains("disabled"));
  /** A submenu opens under a moving pointer and costs no click. */
  const hover = async (text: string) => {
    const item = await need(() => menuItem(text), `submenu ${text}`), box = item.getBoundingClientRect();
    for (const dx of [4, 9, 15]) { const point = { bubbles: true, cancelable: true, view: window, clientX: box.left + dx, clientY: box.top + box.height / 2, screenX: box.left + dx, screenY: box.top }; item.dispatchEvent(new MouseEvent("mouseover", point)); item.dispatchEvent(new MouseEvent("mousemove", point)); await sleep(40); }
  };
  const choose = (text: string) => click(() => menuItem(text), `menu item ${text}`);
  const button = (text: string, scope: string) => () => $$<HTMLButtonElement>(`${scope} button`).find((b) => { const copy = b.cloneNode(true) as HTMLElement; copy.querySelectorAll("kbd,svg,.accel-key").forEach((n) => n.remove()); return copy.textContent?.trim() === text; });

  // ── what the application shows and what Git has ──
  /** Reading fails while an operation changes the repository; the next read sees the result. */
  const reading = async <T,>(read: () => Promise<T>): Promise<T> => { for (let tries = 0; ; tries++) { try { return await read(); } catch (error) { if (tries > 40) throw error; await sleep(150); } } };
  const repo = () => reading(() => invoke<Repository>("git_repository", { path }));
  const headRow = async () => { const { head } = await repo(); return () => $<HTMLElement>(`.git-graph-lines [data-commit="${head}"]`); };
  const row = (text: string) => $$<HTMLElement>(".git-graph-lines [data-commit]").find((r) => r.textContent?.includes(text));
  const sideRow = (text: string) => $$<HTMLElement>(".gg-side-row").find((r) => r.textContent?.includes(text));
  const ready = () => need(() => { const b = $(".gg-actions") as HTMLButtonElement | null; return b && !b.disabled; }, "idle workspace", 20000);
  const modal = (text: string) => need(() => $(".modal")?.textContent?.includes(text), `dialog ${text}`, 15000);
  const modalGone = () => need(() => !$(".modal"), "closed dialog");
  const subjects = async (count: number) => {
    const result: { id: string; message: string; parents: string[] }[] = [];
    for (let id: string | undefined = (await repo()).head; id && result.length < count;) { const d: { id: string; message: string; parents: string[] } = await invoke("git_commit_details", { path, commit: id, base: null }); result.push(d); id = d.parents[0]; }
    return result;
  };
  const first = async () => (await subjects(1))[0].message.trim().split("\n")[0];
  const open = async (name: string) => {
    A.openOverlay(null); await sleep(150);
    path = at(name); A.openOverlay({ type: "gitGraph", path });
    await need(() => $(".git-graph-lines [data-commit]"), `history of ${name}`, 15000);
    if (!$(".gg-sidebar")) { const toggle = $("[aria-label='Toggle repository panels']"); if (toggle) { h.click(toggle); await until(() => $(".gg-sidebar")); } }
    await ready(); $<HTMLElement>(".git-graph")?.focus(); await sleep(150);
    steps = { keys: 0, typed: 0, clicks: 0 };
  };

  /** `setup` brings the repository into the state the task starts from; it is not counted. */
  const scenario = async (id: string, title: string, route: "keyboard" | "mouse", drive: () => Promise<void>, verify: () => Promise<boolean>, setup?: () => Promise<void>) => {
    const name = `${id}${route[0]}`, label = `scenario ${id} (${route}): ${title}`;
    try {
      if (setup) { path = at(name); await setup(); }
      await open(name);
      const started = performance.now();
      await drive();
      const spent = steps, ms = Math.round(performance.now() - started);
      // Dialogs and reloads finish asynchronously; what counts is the state Git ends up with.
      let ok = false;
      for (let tries = 0; tries < 60 && !ok; tries++) { ok = await verify().catch(() => false); if (!ok) await sleep(250); }
      log(ok, `${label} reaches the expected Git state`, stepsText(spent));
      note(`STEPS ${name} ${JSON.stringify(spent)} ${ms} ms  — ${title}`);
      const limit = baseline.steps[name];
      log(!limit || !overBaseline(spent, limit), `${label} needs no more steps than the baseline`, limit ? `${stepsText(spent)}, baseline ${stepsText(limit)}` : "no baseline yet");
      if (!limit) note(`NOTE ${name} has no stored baseline`);
    } catch (error) {
      log(false, label, error instanceof Error ? error.message : String(error));
      await shot(`baseline-${name}-failure`);
    }
    if ($(".modal")) { h.key("Escape", {}, $(".modal")); await sleep(200); }
    if (h.getState().menu) { h.key("Escape", {}, document.body); await sleep(200); }
  };
  const committedLines = async () => {
    const details = await invoke<{ message: string; files: { path: string; added: number; deleted: number }[] }>("git_commit_details", { path, commit: (await repo()).head, base: null });
    const left = await invoke<{ changes: { path: string }[] }>("git_workspace", { path });
    return details.message.trim() === "stage two lines" && details.files.length === 1 && details.files[0].path === "edit.txt" && details.files[0].added === 2 && left.changes.length === 1;
  };
  const diffRows = () => $$<HTMLElement>(".gc-diff-line");
  const text = (element: Element | undefined) => element?.textContent?.trim() ?? "";

  // ── latencies ──
  const frame = () => new Promise<void>((done) => requestAnimationFrame(() => done()));
  /** Milliseconds until `shown` holds in a frame that was painted (the frame after the one that has it). */
  const painted = async (act: () => void, shown: () => unknown, ms = 30000): Promise<number> => {
    const start = performance.now(); act();
    while (performance.now() - start < ms) { await frame(); if (shown()) { await frame(); return performance.now() - start; } }
    throw new Error("not painted in time");
  };
  const median = (values: number[]) => [...values].sort((x, y) => x - y)[Math.floor(values.length / 2)];
  const metric = (name: string, values: number[]) => {
    const value = Math.round(median(values) * 10) / 10, known = baseline.latencyMs[platform]?.[name];
    note(`METRIC ${platform} ${name} median ${value} ms (${values.map((v) => Math.round(v)).join(", ")})${known === undefined ? ", no baseline" : `, baseline ${known} ms`}`);
    log(Number.isFinite(value), `latency measured: ${name}`, `${value} ms${known === undefined ? "" : `, baseline ${known} ms`}`);
  };

  try {
    await h.updateSettings({ keyboardProfile: "vim", theme: "dark", language: "en", shortcuts: {}, animations: false });
    await setWindowSize(1320, 900);

    // 1 stage 2 of 4 added lines and commit
    await scenario("1", "stage 2 lines and commit", "keyboard", async () => {
      await press("Enter"); await need(() => $(".gc-file.selected") && diffRows().some((r) => text(r).includes("added a")), "patch of edit.txt");
      await press("l"); await press("]");
      for (let i = 0; i < 12 && !text($(".gc-diff-line.current") ?? undefined).includes("added a"); i++) await press("j");
      await press("v"); await press("j"); await press("Enter");
      await need(() => $$(".gc-file").length === 2 && !$(".gc-files .gc-file:disabled"), "file in both lists");
      await press("c"); await type("stage two lines"); await press("Enter", { ctrlKey: true });
    }, committedLines);
    await scenario("1", "stage 2 lines and commit", "mouse", async () => {
      await click(() => $(".gg-working"), "Working Changes");
      const box = (line: string) => () => diffRows().find((r) => text(r).includes(line))?.querySelector("input");
      await click(box("added a"), "checkbox of line 1"); await click(box("added b"), "checkbox of line 2");
      await click(button("Stage selection", ".gc-selection-tools"), "Stage selection");
      await need(() => $$(".gc-file").length === 2, "file in both lists");
      await click(() => $("textarea[aria-label='Commit message']"), "message"); await type("stage two lines");
      await click(() => $$<HTMLButtonElement>(".gc-commit button").find((b) => text(b).startsWith("Commit")), "Commit");
    }, committedLines);

    // 2 squash the last three commits under a new message
    let before: { id: string; parents: string[] }[] = [];
    const squashed = async () => { const now = await subjects(2); return now[0].message.trim() === NEW_MESSAGE && now[1].id === before[3].id; };
    await scenario("2", "squash last 3 with a new message", "keyboard", async () => {
      before = await subjects(4);
      await press("j");
      for (let i = 0; i < 2; i++) {
        const head = (await repo()).head;
        await press("s"); await modal("Rewrite selected commit?"); await press("Enter"); await modalGone();
        for (let tries = 0; tries < 60 && (await repo()).head === head; tries++) await sleep(150);
        await ready(); await sleep(400);
      }
      await press("r"); await modal("Edit commit message");
      steps.keys++; // Ctrl+A: the field opens with the old messages; the typed text replaces them.
      await type(NEW_MESSAGE); await press("Enter", { ctrlKey: true });
    }, squashed);
    await scenario("2", "squash last 3 with a new message", "mouse", async () => {
      before = await subjects(4);
      for (let i = 0; i < 2; i++) {
        const head = (await repo()).head;
        await rightClick(() => $(`.git-graph-lines [data-commit="${head}"]`), "HEAD"); await hover("Rewrite history"); await choose("Squash into preceding commit…");
        await click(button("Rewrite commit", ".modal"), "Rewrite commit"); await modalGone();
        for (let tries = 0; tries < 60 && (await repo()).head === head; tries++) await sleep(150);
        await ready(); await sleep(400);
      }
      const head = (await repo()).head;
      await rightClick(() => $(`.git-graph-lines [data-commit="${head}"]`), "HEAD"); await hover("Rewrite history"); await choose("Reword commit…");
      await modal("Edit commit message"); steps.keys++;
      await type(NEW_MESSAGE); await click(button("Save message", ".modal"), "Save message");
    }, squashed);

    // 3 cherry-pick the tip of another branch
    const picked = async () => (await first()) === FEATURE_TIP && (await repo()).branch === "main";
    await scenario("3", "cherry-pick from another branch", "keyboard", async () => {
      await press("C", { shiftKey: true }); await need(() => $(".gp-step"), "choice of the commit");
      for (let i = 0; i < 6 && !$(".gp-row.on")?.textContent?.startsWith("feature"); i++) await press("ArrowDown");
      await press("Enter");
    }, picked);
    await scenario("3", "cherry-pick from another branch", "mouse", async () => {
      await rightClick(() => row(FEATURE_TIP), "feature tip"); await hover("Integrate"); await choose("Cherry-pick onto the current branch…");
      await click(button("Cherry-pick", ".modal"), "Cherry-pick");
    }, picked);

    // 9 cherry-pick three commits of another branch as one operation
    const pickedAll = async () => (await subjects(3)).map((c) => c.message.trim()).join() === "feature: add feat-3,feature: add feat-2,feature: add feat-1" && !(await repo()).operation;
    await scenario("9", "cherry-pick three commits", "keyboard", async () => {
      await press("h");
      for (let i = 0; i < 6 && !document.activeElement?.textContent?.startsWith("feature"); i++) await press("j");
      await press("l"); await press("v"); await press("j"); await press("j"); await press("C", { shiftKey: true });
      await modal("Cherry-pick onto main?"); await press("Enter");
    }, pickedAll);
    await scenario("9", "cherry-pick three commits", "mouse", async () => {
      await click(() => row("feature: add feat-3"), "first commit");
      const last = await need(() => row("feature: add feat-1"), "last commit"); steps.clicks++; h.click(last, { shiftKey: true }); await sleep(150);
      await rightClick(() => row("feature: add feat-2"), "chosen commits"); await choose("Cherry-pick 3 commits onto the current branch…");
      await click(button("Cherry-pick", ".modal"), "Cherry-pick");
    }, pickedAll);

    // 4 who changed line 5 of work.txt, and that commit in the graph
    const jumped = async () => !!$(".git-graph-lines .gg-row.selected")?.textContent?.includes("add work and edit");
    const blameLine = () => $$<HTMLElement>(".gcd-blame-line").find((r) => text(r).endsWith("work line 5"))?.querySelector("button");
    await scenario("4", "who changed line X, jump to its commit", "keyboard", async () => {
      await press("j"); await need(() => $(".gcd-file.selected"), "commit details"); await press("l"); await need(() => document.activeElement?.closest(".git-commit-details"), "focus in the details"); await press("b");
      await click(blameLine, "blame line"); // no key selects a blame line
    }, jumped);
    await scenario("4", "who changed line X, jump to its commit", "mouse", async () => {
      await click(await headRow(), "HEAD"); await click(button("Blame", ".git-commit-details"), "Blame"); await click(blameLine, "blame line");
    }, jumped);

    // 5 rebase onto a branch that conflicts, take the side of main, continue
    let rival = "";
    const rebased = async () => { const r = await repo(); if (r.operation || r.branch !== "main") return false; const [tip, parent] = await subjects(2); const blob = await invoke<string>("git_commit_content", { path, commit: tip.id, file: "work.txt" }); return parent.id === rival && blob.includes("main side") && !blob.includes("<<<<<<<"); };
    await scenario("5", "resolve one conflict and continue the rebase", "keyboard", async () => {
      rival = (await repo()).refs.find((r) => r.name === "rival")!.id;
      await press("h");
      for (let i = 0; i < 6 && !document.activeElement?.textContent?.startsWith("rival"); i++) await press("j");
      await press("r"); await modal("Rebase commit?"); await press("Enter"); await modalGone();
      await need(() => sideRow("work.txt"), "conflicted file", 20000); await ready(); await sleep(400);
      const hints = text($(".git-graph > footer") ?? undefined);
      log(hints.includes("mark resolved") && !hints.includes("r rebase"), "the keys follow the Conflicts tab when a rebase stops", hints);
      log(!!document.activeElement?.closest(".gg-side-row,.gm"), "the conflicted file has the keyboard focus when a rebase stops", document.activeElement?.className ?? "");
      const onFile = () => document.activeElement?.closest(".gg-side-row")?.textContent?.includes("work.txt");
      if (!document.activeElement?.closest(".gm") && !onFile()) { if (!document.activeElement?.closest(".gg-sidebar")) await press("h"); if (!onFile()) await press("j"); }
      await need(() => $(".gm"), "merge editor");
      if (!document.activeElement?.closest(".gm")) await click(() => $(".gm"), "merge editor");
      await press("t"); await press("Enter", { ctrlKey: true });
      await need(() => ($(".gg-operation button") as HTMLButtonElement | null)?.disabled === false, "Continue", 15000); await ready(); await sleep(300);
      focused();
      await press("m"); await need(() => $(".menu"), "operation menu"); await press("j"); await press("Enter");
    }, rebased);
    await scenario("5", "resolve one conflict and continue the rebase", "mouse", async () => {
      rival = (await repo()).refs.find((r) => r.name === "rival")!.id;
      await rightClick(() => sideRow("rival"), "rival"); await hover("Rewrite history"); await choose("Rebase current branch onto selection…");
      await click(button("Rebase", ".modal"), "Rebase"); await modalGone();
      await click(() => sideRow("work.txt"), "conflicted file"); await click(button("Theirs", ".gm-toolbar"), "side of main");
      await click(() => $(".gm-confirm"), "Confirm"); await click(button("Continue", ".gg-operation"), "Continue");
    }, rebased);

    // 6 undo a bad amend, 8 undo a bad rewrite (a dropped commit), both made in Gall
    for (const [id, title] of [["6", "undo a bad amend"], ["8", "undo a bad rebase"]] as const) {
      let wanted = "", bad = "";
      const spoil = async () => {
        wanted = (await repo()).head;
        if (id === "6") await invoke("git_index_action", { path, action: "amend", file: null, message: "worse amend", expected: (await invoke<{ token: string }>("git_workspace", { path })).token });
        else await invoke("git_commit_edit", { path, target: wanted, action: "drop", expected: (await repo()).token });
        bad = (await repo()).head;
        if (bad === wanted) throw new Error("nothing to undo");
      };
      const undone = async () => (await repo()).head === wanted && wanted !== bad;
      await scenario(id, title, "keyboard", async () => { await press("u"); await modal("Undo "); await press("Enter"); }, undone, spoil);
      await scenario(id, title, "mouse", async () => {
        await click(() => $(".gg-actions"), "Actions"); await choose("Undo the last operation…");
        await click(button("Undo", ".modal"), "Undo");
      }, undone, spoil);
    }
    // The timeline: both states of 8 with the keyboard, then the journal of the mouse route with the mouse.
    await open("8k");
    const redone = (await repo()).head;
    h.key("U", { shiftKey: true }, focused()); await need(() => $(".gj-node"), "timeline");
    log(!!$(".gj-node.undone") && $(".gj-node.current.selected")?.textContent?.includes("Before these operations") === true, "U opens the timeline with the undone operation and the current state", $(".gj-list")?.textContent ?? "");
    await shot("baseline-timeline");
    h.key("k", {}, focused()); await sleep(150);
    log(/Redoes 1:main [0-9a-f]{8} → [0-9a-f]{8}/.test($(".gj-foot")?.textContent ?? ""), "the timeline previews which reference moves", $(".gj-foot")?.textContent ?? "");
    h.key("Enter", {}, focused()); await modal("Redo "); h.key("Enter", {}, $(".modal")); await modalGone(); await ready();
    log(!!await until(() => $(".gj-node.current")?.textContent?.includes("History rewrite"), 8000) && (await repo()).head !== redone, "Enter goes to the chosen state", $(".gj-list")?.textContent ?? "");
    h.key("Escape", {}, $(".gj")); await sleep(300);
    log(!$(".gj") && !!$(".git-graph") && !!document.activeElement?.closest(".git-graph"), "Esc closes the timeline and keeps the window");
    h.key("r", { ctrlKey: true }, focused()); await sleep(400);
    log(!$(".modal") && !!$(".toast"), "Ctrl+r says that there is nothing to redo", $(".toast")?.textContent ?? "");

    // 10 the same amend made outside Gall, which only the reflog knows
    {
      const id = "10", title = "undo an amend made outside Gall", good = "tip: edit work line 20", mode = "s";
      let wanted = "", bad = "";
      const find = async () => {
        // The old commit is only in the reflog: the row with the subject that is not the current HEAD.
        bad = (await repo()).head;
        await click(() => $(".gg-reflog input"), "Reflog");
        const old = await need(() => $$<HTMLElement>(".git-graph-lines [data-commit]").find((r) => r.textContent?.includes(good) && r.dataset.commit !== bad), "old commit", 15000);
        wanted = old.dataset.commit!; await ready(); await sleep(250);
        return old;
      };
      const undone = async () => (await repo()).head === wanted && wanted !== bad;
      await scenario(id, title, "keyboard", async () => {
        await find(); // no key toggles the reflog
        await click(() => $(".git-graph-lines"), "commit list"); // the focus is on the checkbox
        for (let i = 0; i < 12 && $<HTMLElement>(".git-graph-lines .gg-row.selected")?.dataset.commit !== wanted; i++) await press("j");
        await press("g"); await modal("Reset main"); await press(mode);
      }, undone);
      await scenario(id, title, "mouse", async () => {
        await find();
        await rightClick(() => $(`.git-graph-lines [data-commit="${wanted}"]`), "old commit"); await hover("Integrate"); await choose("Reset to commit…");
        await click(() => $(`.modal [data-id='soft']`), "reset mode");
      }, undone);
    }

    // 7 find a named stash and apply it
    const applied = async () => { const r = await repo(); const w = await invoke<{ changes: { path: string }[] }>("git_workspace", { path }); return r.stashes.length === 4 && w.changes.some((c) => c.path === "edit.txt") && (await invoke<{ text: string }>("git_patch_selection", { path, file: "edit.txt", staged: false })).text.includes("stash 2"); };
    await scenario("7", "find a named stash a week later and apply it", "keyboard", async () => {
      await press("z"); await need(() => sideRow(STASH), "stash list"); await sleep(150);
      for (let i = 0; i < 6 && !document.activeElement?.textContent?.includes(STASH); i++) await press("j");
      await press(" "); await modal("Apply stash?"); await press("Enter");
    }, applied);
    await scenario("7", "find a named stash a week later and apply it", "mouse", async () => {
      await click(button("Stashes", ".gg-side-tabs"), "Stashes"); await rightClick(() => sideRow(STASH), "stash"); await choose("Apply stash…"); await click(button("Apply", ".modal"), "Apply");
    }, applied);

    // Latencies: the application is warm (every repository was opened once), the numbers are medians.
    for (const name of ["c200", "c2000"]) {
      const times: number[] = [];
      for (let i = 0; i < 6; i++) {
        A.openOverlay(null); await sleep(350);
        times.push(await painted(() => A.openOverlay({ type: "gitGraph", path: at(name) }), () => $(".git-graph-lines [data-commit]")));
      }
      metric(`open to first graph paint, ${name.slice(1)} commits`, times.slice(1));
    }
    path = at("c2000"); await ready();
    for (let tries = 0; tries < 12; tries++) { const more = $(".gg-load-more") as HTMLButtonElement | null; if (!more) break; const count = $$(".git-graph-lines [data-commit]").length; h.click(more); await until(() => $$(".git-graph-lines [data-commit]").length > count || !$(".gg-load-more"), 20000); await ready(); }
    log($$(".git-graph-lines [data-commit]").length >= 2000, "2000 commits are listed", String($$(".git-graph-lines [data-commit]").length));
    $<HTMLElement>(".git-graph")?.focus(); h.key("j"); await sleep(600);
    const moves: number[] = [];
    for (let i = 0; i < 25; i++) {
      const selected = () => $<HTMLElement>(".git-graph-lines .gg-row.selected")?.dataset.commit, from = selected();
      moves.push(await painted(() => h.key("j"), () => selected() !== from));
      await sleep(120);
    }
    metric("key to selection in a list of 2000 commits", moves);
    await shot("baseline-2000-commits");
    const palettes: number[] = [], ready60: number[] = [];
    for (let i = 0; i < 6; i++) {
      // React renders the list in the microtasks after the key event, before the browser paints the next frame.
      const started = performance.now(); h.key(":", { shiftKey: true });
      for (let turn = 0; turn < 20 && !$(".gp-row"); turn++) await Promise.resolve();
      ready60.push($(".gp-row") ? performance.now() - started : Infinity); await sleep(120);
      h.key("Escape", {}, $(".gp-input input")); await until(() => !$(".gp")); await sleep(120);
      palettes.push(await painted(() => h.key(":", { shiftKey: true }), () => $(".gp-row"))); await sleep(120);
      h.key("Escape", {}, $(".gp-input input")); await until(() => !$(".gp")); await sleep(120);
    }
    metric("key to the Git command palette on 2000 commits", palettes.slice(1));
    log(median(ready60) < 1000 / 60, "the palette is ready for the next frame", `${median(ready60).toFixed(1)} ms (${ready60.map((v) => v.toFixed(1)).join(", ")})`);
    A.openOverlay(null); await sleep(300); A.openOverlay({ type: "gitGraph", path }); await need(() => $(".git-graph-lines [data-commit]"), "history"); await ready();
    $<HTMLElement>(".git-graph")?.focus();
    const changes: number[] = [], refresh: number[] = [];
    for (let i = 0; i < 5; i++) {
      changes.push(await painted(() => h.key("Enter"), () => $(".gc-file"))); await sleep(400);
      const panel = () => $<HTMLElement>(".git-changes")!, stagedRows = () => $$(".gc-state.staged").length, was = stagedRows();
      panel().focus();
      refresh.push(await painted(() => h.key(" ", {}, panel()), () => stagedRows() !== was)); await ready(); await sleep(400);
      const now = stagedRows(); panel().focus(); h.key("u", {}, panel()); await until(() => stagedRows() !== now, 8000); await ready(); await sleep(300);
      h.key("Escape", {}, panel()); await until(() => !$(".git-changes")); await sleep(250); $<HTMLElement>(".git-graph")?.focus(); h.key("k"); await sleep(250); $<HTMLElement>(".git-graph")?.focus();
    }
    metric("key to open Working Changes", changes);
    metric("key to refreshed Working Changes (stage a file)", refresh);
  } catch (error) {
    log(false, "git baseline", error instanceof Error ? error.stack ?? error.message : String(error));
    await shot("baseline-failure");
  }
  await h.finish();
}
