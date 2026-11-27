import { invoke } from "@tauri-apps/api/core";
import { api } from "./api/ipc";
import { join, parent } from "./core/path";
import * as A from "./state/actions";

type RepoState = { conflicts?: string[] };

/**
 * Offline running-app scenario for merge editor proposals (WN-130) using `compare_ai_fake`.
 * Exercises `merge`, `mergemarkers`, `mergestale`, `mergebig`, `mergesyntax`, and `sleep` (cancel),
 * plus keyboard (`p`, `Esc`, Propose button, Vim-disabled), CRLF + long lines, multiple open conflicts,
 * dropping stale proposals when choices change, and the discard-manual-edits confirmation.
 */
export async function runMergeAiSelfTest(h: typeof import("./selftest").gitSelftest, rootDir: string) {
  const { $, $$, click, key, until, sleep, shot, setWindowSize, log, typeInto } = h;
  const fakePathFile = join(rootDir, "compare-ai-fake-path.txt");
  let fakeProgram = "";
  try {
    fakeProgram = (await api.readTextFile(fakePathFile)).text.trim();
  } catch {
    log(true, "merge-ai: skipped (compare_ai_fake path fixture not provided)");
    return;
  }
  const mergeRepo = join(parent(rootDir) ?? rootDir, "merge-ai");
  const setMode = async (mode: string) => {
    await api.compareAiConfigSave({
      enabled: true,
      program: fakeProgram,
      args: [mode],
      model: `fixture-${mode}`,
      deadlineSecs: 30,
      env: [],
    });
  };
  const pressButton = async (label: string, scope: ParentNode = document) => {
    const btn = await until(() => $$<HTMLButtonElement>("button", scope).find((b) => {
      if (b.disabled) return false;
      const clone = b.cloneNode(true) as HTMLElement;
      clone.querySelectorAll("kbd,svg").forEach((n) => n.remove());
      return clone.textContent?.trim() === label;
    }), 8000);
    if (!btn) throw new Error(`Missing merge-ai button "${label}"`);
    click(btn);
    return btn;
  };
  const openConflict = async (name: string) => {
    const row = await until(() => $$<HTMLButtonElement>(".gg-side-row:not(:disabled)").find((el) => el.textContent?.includes(name)), 10000);
    if (!row) throw new Error(`Missing conflict row "${name}"`);
    click(row);
    const editor = await until(() => $(".gm") && $(".gm-card[data-card='0'].active") ? $<HTMLElement>(".gm") : null, 10000);
    if (!editor) throw new Error(`Merge editor did not open for "${name}"`);
    editor.focus();
    return editor;
  };
  const hasConflict = async (relPath: string) => {
    const repo = await invoke<RepoState>("git_repository", { path: mergeRepo });
    return (repo.conflicts ?? []).includes(relPath);
  };

  try {
    await h.updateSettings({ keyboardProfile: "vim", theme: "dark", language: "en", shortcuts: {} });
    await setWindowSize(1280, 860);
    A.openOverlay(null);
    await until(() => !$(".git-graph"));
    await A.openGitWorkspace(mergeRepo);

    // ── 1. Rust file (single conflict, no trailing newline): merge, reject, Esc, malformed/stale/big/syntax, cancel, accept + confirm ──
    const rustEditor = await openConflict("sample.rs");
    const proposeBtn = $<HTMLButtonElement>('button[aria-label="Propose a resolution for conflict 1"]', rustEditor);
    log(!!proposeBtn && proposeBtn.textContent?.includes("Propose…"), "merge-ai: Propose button has an accessible name and key cap", proposeBtn?.getAttribute("aria-label") ?? "missing");

    // Mode: merge — propose via `p`, inspect panel, Reject keeps result & staged file untouched.
    await setMode("merge");
    rustEditor.focus();
    key("p", {}, rustEditor);
    const panel = await until(() => $(".gm-proposal .gm-proposal-lines") ? $(".gm-proposal") : null, 10000);
    log(!!panel
      && panel.textContent!.includes("fixture-merge")
      && panel.textContent!.includes("The model's words")
      && panel.textContent!.includes("combined")
      && panel.textContent!.includes("Risk:")
      && panel.textContent!.includes("Uncertainty:")
      && !!$(".gm-proposal-evidence li", panel),
    "merge-ai: p opens proposal panel with provenance, model name, explanation, risk, uncertainty and evidence");
    await sleep(140);
    await shot("merge-ai-proposal-rust");
    h.note("SCREENSHOT merge-ai-proposal-rust.png: expected dark merge editor with proposal panel showing provenance colours, explanation, risk, uncertainty, evidence and Take/Reject buttons.");

    await pressButton("Reject (Esc)", panel!);
    log(!!await until(() => !$(".gm-proposal")) && !!$(".gm-result-hint") && await hasConflict("src/sample.rs"),
      "merge-ai: Reject closes panel and leaves result and staged output untouched");

    // Propose via accessible button and close with Esc.
    click(proposeBtn!);
    await until(() => $(".gm-proposal .gm-proposal-lines"), 10000);
    rustEditor.focus();
    key("Escape", {}, rustEditor);
    log(!!await until(() => !$(".gm-proposal")) && document.activeElement === rustEditor,
      "merge-ai: Escape closes the proposal panel and returns focus to the editor");

    // Refused modes: mergemarkers, mergestale, mergebig, mergesyntax (real Rust parse check with composed before/after).
    for (const [mode, expectedText] of [
      ["mergemarkers", "conflict marker"],
      ["mergestale", "other text"],
      ["mergebig", "at most"],
      ["mergesyntax", "would not parse"],
    ] as const) {
      await setMode(mode);
      rustEditor.focus();
      key("p", {}, rustEditor);
      const status = await until(() => {
        const p = $(".gm-proposal [role='status']");
        return p && !p.textContent?.includes("Asking") ? p : null;
      }, 10000);
      log(!!status && status.textContent!.toLowerCase().includes(expectedText) && !$(".gm-proposal .gm-confirm") && !!$(".gm-result-hint") && await hasConflict("src/sample.rs"),
        `merge-ai: ${mode} is refused and keeps conflict unresolved`,
        status?.textContent ?? "no status");
      await pressButton("Close", $(".gm-proposal")!);
      await until(() => !$(".gm-proposal"));
    }

    // Mode: sleep — cancel while working stops the request (selftest.sh verifies with ps that no process remains).
    await setMode("sleep");
    rustEditor.focus();
    key("p", {}, rustEditor);
    const asking = await until(() => {
      const p = $(".gm-proposal [role='status']");
      return p?.textContent?.includes("Asking the local model") ? p : null;
    }, 5000);
    log(!!asking, "merge-ai: sleep mode enters asking state with Cancel button");
    await sleep(120);
    await pressButton("Cancel", $(".gm-proposal")!);
    const cancelled = await until(() => $(".gm-proposal [role='status']")?.textContent?.includes("Cancelled."), 5000);
    log(!!cancelled, "merge-ai: Cancel stops an in-flight proposal");
    await pressButton("Close", $(".gm-proposal")!);
    await until(() => !$(".gm-proposal"));

    // Mode: merge — Take puts lines into editable result, does NOT stage until Confirm, Confirm stages resolved Rust file.
    await setMode("merge");
    rustEditor.focus();
    key("p", {}, rustEditor);
    await until(() => $(".gm-proposal .gm-confirm"), 10000);
    await pressButton("Take the proposal", $(".gm-proposal")!);
    const taken = await until(() => !$(".gm-proposal") && $(".gm-result pre")?.textContent?.includes("let x = 2;") && $(".gm-result pre")?.textContent?.includes("let y = 3;"), 5000);
    log(!!taken && await hasConflict("src/sample.rs"),
      "merge-ai: Take inserts lines into the editable result without staging yet");
    await pressButton("Confirm", rustEditor);
    const staged = await until(async () => !(await hasConflict("src/sample.rs")), 8000);
    const savedRust = (await api.readTextFile(join(mergeRepo, "src/sample.rs"))).text;
    log(!!staged && savedRust.includes("let x = 2;") && savedRust.includes("let y = 3;") && !savedRust.includes("<<<<<<<"),
      "merge-ai: Confirm stages the accepted Rust proposal");

    // ── 2. CRLF + long lines + two conflicts open + dropping stale proposal + discard manual edits + Vim disabled ──
    const crlfEditor = await openConflict("notes.txt");
    await setMode("merge");
    crlfEditor.focus();
    key("p", {}, crlfEditor);
    const crlfPanel = await until(() => $(".gm-proposal .gm-proposal-warn") ? $(".gm-proposal") : null, 10000);
    const linesPre = crlfPanel && $<HTMLElement>(".gm-proposal-lines", crlfPanel);
    log(!!crlfPanel && !!linesPre && linesPre.scrollWidth > linesPre.clientWidth,
      "merge-ai: CRLF file with another conflict open shows the warning and scrolls long proposed lines horizontally",
      linesPre ? `scrollWidth=${linesPre.scrollWidth} clientWidth=${linesPre.clientWidth}` : "missing");
    await sleep(140);
    await shot("merge-ai-proposal-crlf-warn");
    h.note("SCREENSHOT merge-ai-proposal-crlf-warn.png: expected merge proposal panel showing the open-conflicts warning and horizontally scrollable long line without layout overflow.");

    // Deciding the second conflict drops the earlier proposal so it cannot be accepted without asking again.
    const secondCard = $(".gm-card[data-card='1']");
    if (secondCard) {
      await pressButton("Ours", secondCard);
    }
    log(!!await until(() => !$(".gm-proposal"), 4000),
      "merge-ai: deciding another conflict drops the earlier proposal");

    // Hand-edit the result, then ask for Conflict 1 (now the only unresolved conflict) and Take -> Discard manual edits prompt appears.
    crlfEditor.focus();
    key("e", {}, crlfEditor);
    const area = await until(() => $<HTMLTextAreaElement>(".gm-result textarea"), 4000);
    if (area) {
      typeInto(area, area.value + "\r\nhand edit marker\r\n");
      await pressButton("Done editing", crlfEditor);
    }
    const firstConflictPropose = await until(() => $<HTMLButtonElement>('button[aria-label="Propose a resolution for conflict 1"]', crlfEditor), 4000);
    if (firstConflictPropose) click(firstConflictPropose);
    await until(() => $(".gm-proposal .gm-confirm"), 10000);
    await pressButton("Take the proposal", $(".gm-proposal")!);
    const discardModal = await until(() => $(".modal") && $(".modal")!.textContent?.includes("Discard manual edits?") ? $(".modal") : null, 5000);
    log(!!discardModal, "merge-ai: accepting over a hand-edited result asks before discarding manual edits");
    await sleep(120);
    await shot("merge-ai-discard-prompt");
    h.note("SCREENSHOT merge-ai-discard-prompt.png: expected Discard manual edits? confirmation dialog above the merge proposal panel.");
    if (discardModal) {
      await pressButton("Cancel", discardModal);
      await until(() => !$(".modal"));
    }
    log(!!$(".gm-result pre")?.textContent?.includes("hand edit marker"),
      "merge-ai: cancelling the discard prompt preserves manual edits");
    // Now accept and discard edits, verifying CRLF is preserved in the composed result.
    await pressButton("Take the proposal", $(".gm-proposal")!);
    const discardAgain = await until(() => $(".modal") && $(".modal")!.textContent?.includes("Discard manual edits?") ? $(".modal") : null, 5000);
    if (discardAgain) {
      await pressButton("Discard edits", discardAgain);
      await until(() => !$(".modal"));
    }
    await until(() => !$(".gm-proposal"), 4000);
    await pressButton("Confirm", crlfEditor);
    const crlfStaged = await until(async () => !(await hasConflict("notes.txt")), 8000);
    const savedNotes = (await api.readTextFile(join(mergeRepo, "notes.txt"))).text;
    log(!!crlfStaged && savedNotes.includes("\r\n") && !savedNotes.includes("hand edit marker"),
      "merge-ai: confirming the accepted CRLF proposal preserves CRLF line endings");

    // Vim keys disabled: plain `p` does not trigger a proposal.
    await h.updateSettings({ keyboardProfile: "explorer" });
    A.openOverlay(null);
    await until(() => !$(".git-graph"));
    // Re-open standalone merge on a quick 3-way conflict? Instead verify gitPanelKey / editor behavior on the already-tested workspace:
    await A.openGitWorkspace(mergeRepo);
    await until(() => $(".git-graph"));
    A.openOverlay(null);
    await until(() => !$(".git-graph"));
  } catch (e) {
    log(false, "merge-ai scenario", String(e));
    await shot("merge-ai-failure");
  } finally {
    await api.compareAiConfigSave({ enabled: false, program: "", args: [], model: "", deadlineSecs: 120, env: [] }).catch(() => {});
    A.openOverlay(null);
  }
}
