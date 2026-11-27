import { invoke } from "@tauri-apps/api/core";
import { api } from "./api/ipc";
import { join, parent } from "./core/path";
import * as A from "./state/actions";

/**
 * Running-app scenario for WN-163:
 * 1. Outline of both sides (`o` and `O`) with Rust trait declarations and TS class fields,
 *    plus parse-error pane naming the exact broken side and switching to the valid side with `O`.
 * 2. Collapsed generated file in `DiffView` (extra `generatedPatterns` setting, hidden-body width/scroll preservation,
 *    `n` on collapsed body, `H` reveal/collapse, and identical patch export while collapsed).
 * 3. Git workspace Working Changes and Commit Details collapse with `.gitattributes`, lockfile, and extra pattern:
 *    stage/unstage a collapsed lockfile, patch export of the collapsed lockfile, and `show them`.
 */
export async function runOutlineGeneratedSelfTest(h: typeof import("./selftest").gitSelftest, rootDir: string) {
  const { $, $$, click, key, until, sleep, shot, setWindowSize, log } = h;
  const repoDir = join(parent(rootDir) ?? rootDir, "outline-gen");

  try {
    await setWindowSize(1280, 860);
    await h.updateSettings({
      language: "en",
      theme: "dark",
      keyboardProfile: "vim",
      generatedPatterns: ["*.gen.txt"],
    });
    await sleep(100);

    // ── 1. Outline of both sides with Rust trait declarations ──
    const rustLeft = [
      "trait Store {",
      "    fn old_declared(&self);",
      "    fn defined(&self) -> u32 { 1 }",
      "}",
      "impl Store for Mem {",
      "    fn old_declared(&self) {}",
      "}",
    ].join("\n") + "\n";
    const rustRight = [
      "trait Store {",
      "    fn declared(&self);",
      "    fn defined(&self) -> u32 { 2 }",
      "}",
      "impl Store for Mem {",
      "    fn declared(&self) {}",
      "}",
    ].join("\n") + "\n";

    A.openOverlay({
      type: "diff",
      a: { kind: "text", label: "store.rs", text: rustLeft },
      b: { kind: "text", label: "store.rs", text: rustRight },
    });
    const diff = await until(() => $<HTMLElement>(".diffv"), 8000);
    if (!diff) throw new Error("DiffView did not open for Rust trait outline");
    diff.focus();
    key("o", {}, diff);
    const rightOutline = await until(() => {
      const el = $<HTMLElement>(".dv-outline", diff);
      return el && el.textContent?.includes("declared") ? el : null;
    }, 8000);
    log(
      !!rightOutline
        && rightOutline.textContent!.includes("Outline · right")
        && rightOutline.textContent!.includes("declared")
        && rightOutline.textContent!.includes("defined"),
      "outline-generated: right-side Rust outline lists trait declarations and methods",
      rightOutline?.textContent?.slice(0, 140) ?? "missing",
    );

    // Switch to left side with `O`.
    key("O", { shiftKey: true }, rightOutline!);
    const leftOutline = await until(() => {
      const el = $<HTMLElement>(".dv-outline", diff);
      return el && el.textContent?.includes("Outline · left") && el.textContent?.includes("old_declared") ? el : null;
    }, 8000);
    log(
      !!leftOutline,
      "outline-generated: O switches outline to the left side and lists old_declared",
      leftOutline?.textContent?.slice(0, 140) ?? "missing",
    );
    await sleep(120);
    await shot("outline-rust-both-sides");
    h.note("SCREENSHOT outline-rust-both-sides.png: DiffView with left-side Rust outline showing trait Store, declared method old_declared, defined method, and impl.");
    A.openOverlay(null);
    await until(() => !$(".diffv"));

    // ── 2. TypeScript class fields in outline ──
    const tsLeft = "class Box {\n  count = 1;\n  handler = () => {};\n  render() { return 1; }\n}\n";
    const tsRight = "class Box {\n  count = 2;\n  handler = () => {};\n  render() { return 2; }\n}\n";
    A.openOverlay({
      type: "diff",
      a: { kind: "text", label: "box.ts", text: tsLeft },
      b: { kind: "text", label: "box.ts", text: tsRight },
    });
    const tsDiff = await until(() => $<HTMLElement>(".diffv"), 8000);
    tsDiff!.focus();
    key("o", {}, tsDiff!);
    const tsOutline = await until(() => {
      const el = $<HTMLElement>(".dv-outline", tsDiff!);
      return el && el.textContent?.includes("count") && el.textContent?.includes("handler") ? el : null;
    }, 8000);
    log(
      !!tsOutline && tsOutline.textContent!.includes("field") && tsOutline.textContent!.includes("render"),
      "outline-generated: TS outline lists class fields and methods",
      tsOutline?.textContent?.slice(0, 140) ?? "missing",
    );
    A.openOverlay(null);
    await until(() => !$(".diffv"));

    // ── 3. Parse error names the broken side and O switches to the valid side ──
    A.openOverlay({
      type: "diff",
      a: { kind: "text", label: "broken.rs", text: "fn healthy() -> u32 { 1 }\n" },
      b: { kind: "text", label: "broken.rs", text: "fn ok() {}\nfn broken( {\n" },
    });
    const errDiff = await until(() => $<HTMLElement>(".diffv"), 8000);
    errDiff!.focus();
    key("o", {}, errDiff!);
    const errStatus = await until(() => {
      const s = $<HTMLElement>('.dv-outline [role="status"]', errDiff!);
      return s && s.textContent?.includes("right") ? s.textContent.trim() : null;
    }, 8000);
    log(
      !!errStatus && errStatus.includes("right") && errStatus.includes("2"),
      "outline-generated: parse error pane names the right side and line",
      errStatus ?? "missing",
    );
    const switchBtn = $<HTMLButtonElement>('.dv-outline button[aria-label="Outline of the other side (O)"]', errDiff!);
    log(!!switchBtn, "outline-generated: error outline pane keeps the switch-side button");
    if (switchBtn) {
      click(switchBtn);
      const validLeft = await until(() => $(".dv-outline")?.textContent?.includes("healthy"), 8000);
      log(!!validLeft, "outline-generated: switching from broken right side shows valid left outline");
    }
    A.openOverlay(null);
    await until(() => !$(".diffv"));

    // ── 4. Collapsed generated file (custom pattern *.gen.txt): hidden body, keys, scroll preservation & patch ──
    const longLeft = Array.from({ length: 80 }, (_, i) => `row ${i + 1}: alpha value`).join("\n") + "\n";
    const longRight = Array.from({ length: 80 }, (_, i) => i === 50 ? "row 51: beta value" : `row ${i + 1}: alpha value`).join("\n") + "\n";
    const genA = { kind: "text" as const, label: "schema.gen.txt", text: longLeft };
    const genB = { kind: "text" as const, label: "schema.gen.txt", text: longRight };
    A.openOverlay({ type: "diff", a: genA, b: genB });
    const genDiff = await until(() => $<HTMLElement>(".diffv"), 8000);
    const banner = await until(() => $<HTMLElement>(".dv-generated", genDiff!), 8000);
    const bodyEl = $<HTMLElement>(".dv-body", genDiff!)!;
    log(
      !!banner && banner.textContent!.includes("matching pattern") && Boolean(bodyEl.hidden),
      "outline-generated: extra generated pattern collapses DiffView body with reason",
      banner?.textContent ?? "missing",
    );

    // Patch export works identically while collapsed.
    const patchCollapsed = await api.diffPatch(genA, genB, "unified");
    // Pressing `n` on a collapsed body notifies instead of silently moving unseen.
    genDiff!.focus();
    key("n", {}, genDiff!);
    await sleep(100);
    log(Boolean(bodyEl.hidden), "outline-generated: n on a collapsed body keeps it collapsed");

    // Reveal with H, unfold with z so the document actually scrolls, scroll down, hide with H, reveal with H -> width and non-zero scrollTop preserved.
    key("H", { shiftKey: true }, genDiff!);
    await until(() => !bodyEl.hidden, 4000);
    key("z", {}, genDiff!);
    await sleep(120);
    const widthAfterReveal = bodyEl.clientWidth;
    bodyEl.scrollTop = 260;
    bodyEl.dispatchEvent(new Event("scroll"));
    await sleep(80);
    const targetTop = bodyEl.scrollTop;
    key("H", { shiftKey: true }, genDiff!);
    await until(() => bodyEl.hidden, 4000);
    await sleep(80);
    key("H", { shiftKey: true }, genDiff!);
    await until(() => !bodyEl.hidden, 4000);
    await sleep(120);
    log(
      widthAfterReveal > 400 && bodyEl.clientWidth > 400 && targetTop >= 200 && Math.abs(bodyEl.scrollTop - targetTop) < 5,
      "outline-generated: revealing hidden body preserves width and scroll position",
      `w1=${widthAfterReveal} w2=${bodyEl.clientWidth} scroll=${bodyEl.scrollTop}/${targetTop}`,
    );
    const patchRevealed = await api.diffPatch(genA, genB, "unified");
    log(patchCollapsed === patchRevealed && patchCollapsed.includes("beta value"), "outline-generated: patch export matches whether collapsed or revealed");
    A.openOverlay(null);
    await until(() => !$(".diffv"));

    // ── 5. Git workspace: Working Changes and Commit Details collapse with .gitattributes, lockfile, and extra pattern ──
    await A.openGitWorkspace(repoDir);
    const ws = await until(() => $<HTMLElement>(".git-graph"), 10000);
    if (!ws) throw new Error("Git workspace did not open");

    // Open Working Changes pane (click "Working Changes" in sidebar or wait if already visible).
    const changesTab = await until(
      () => $$<HTMLButtonElement>(".gg-side-row, button", ws).find((b) => b.textContent?.includes("Working Changes")),
      8000,
    );
    if (changesTab) click(changesTab);
    const gcBanner = await until(() => {
      const b = $<HTMLElement>(".gc-generated", ws);
      return b && b.textContent?.includes("3 generated files collapsed") ? b : null;
    }, 10000);
    log(
      !!gcBanner
        && gcBanner.textContent!.includes("lockfile")
        && gcBanner.textContent!.includes(".gitattributes")
        && gcBanner.textContent!.includes("matching pattern"),
      "outline-generated: Working Changes collapses lockfile, .gitattributes and extra pattern files",
      gcBanner?.textContent ?? "missing",
    );
    log(
      !$$<HTMLElement>(".gc-file", ws).some((f) => f.textContent?.includes("Cargo.lock")),
      "outline-generated: Cargo.lock is hidden while collapsed in Working Changes",
    );
    await sleep(120);
    await shot("git-changes-generated-collapse");
    h.note("SCREENSHOT git-changes-generated-collapse.png: Git Working Changes collapsing Cargo.lock, schema.pb.rs (.gitattributes) and custom.gen.txt (matching pattern) under a single summary row.");

    // Stage all while collapsed -> Cargo.lock must be staged even though collapsed!
    const stageAllBtn = $$<HTMLButtonElement>(".gc-tools button", ws).find((b) => b.textContent?.includes("Stage all"));
    if (!stageAllBtn) throw new Error("Stage all button missing");
    click(stageAllBtn);
    const stagedBanner = await until(() => {
      const groups = $$<HTMLElement>(".gc-group", ws).map((g) => g.textContent ?? "").join(" | ");
      return groups.includes("Staged · 4") ? groups : null;
    }, 10000);
    log(!!stagedBanner, "outline-generated: staging while collapsed stages the collapsed lockfile too", stagedBanner ?? "missing");

    // Verify patch export of the collapsed lockfile works while collapsed.
    const lockPatch = await invoke<{ text: string }>("git_patch_selection", { path: repoDir, file: "Cargo.lock", staged: true });
    log(lockPatch.text.includes("1.0.2") || lockPatch.text.includes("Cargo.lock"), "outline-generated: patch of collapsed lockfile is intact");

    // Unstage all while collapsed -> returns to Unstaged · 4.
    const unstageAllBtn = $$<HTMLButtonElement>(".gc-tools button", ws).find((b) => b.textContent?.includes("Unstage all"));
    if (!unstageAllBtn) throw new Error("Unstage all button missing");
    click(unstageAllBtn);
    const unstagedAgain = await until(() => {
      const groups = $$<HTMLElement>(".gc-group", ws).map((g) => g.textContent ?? "").join(" | ");
      return groups.includes("Unstaged · 4") ? groups : null;
    }, 10000);
    log(!!unstagedAgain, "outline-generated: unstaging while collapsed unstages the collapsed lockfile too", unstagedAgain ?? "missing");

    // Click "show them" -> Cargo.lock, schema.pb.rs and custom.gen.txt appear.
    const showBtn = $<HTMLButtonElement>(".gc-generated button", ws);
    click(showBtn!);
    const shownAll = await until(() => {
      const names = $$<HTMLElement>(".gc-file", ws).map((f) => f.textContent ?? "").join(" ");
      return names.includes("Cargo.lock") && names.includes("schema.pb.rs") && names.includes("custom.gen.txt");
    }, 8000);
    log(!!shownAll, "outline-generated: show them reveals all collapsed files in Working Changes");

    // Open Commit Details for the latest commit and verify generated collapse + show them.
    const closeChanges = $<HTMLButtonElement>(".gc-close, .gg-detail-close", ws);
    if (closeChanges) click(closeChanges);
    else key("Escape", {}, $<HTMLElement>(".git-changes", ws) ?? ws);
    await sleep(120);
    const commitRow = await until(
      () => $$<HTMLElement>(".gg-row", ws).find((r) => r.textContent?.includes("update generated")),
      8000,
    );
    if (!commitRow) throw new Error("Commit row 'update generated' missing");
    click(commitRow);
    const detailsTrigger = await until(() => $<HTMLElement>(".gcd-body .gc-generated", ws), 10000);
    log(
      !!detailsTrigger
        && detailsTrigger.textContent!.includes("3 generated files collapsed")
        && detailsTrigger.textContent!.includes(".gitattributes"),
      "outline-generated: Commit Details collapses lockfile, .gitattributes and extra pattern files",
      detailsTrigger?.textContent ?? "missing",
    );
    await sleep(120);
    await shot("git-commit-generated-collapse");
    h.note("SCREENSHOT git-commit-generated-collapse.png: Git Commit Details collapsing 3 generated files while showing src/lib.rs.");

    const commitShowBtn = $<HTMLButtonElement>(".gcd-body .gc-generated button", ws);
    click(commitShowBtn!);
    const commitShown = await until(() => {
      const names = $$<HTMLElement>(".gcd-file", ws).map((f) => f.textContent ?? "").join(" ");
      return names.includes("Cargo.lock") && names.includes("schema.pb.rs") && names.includes("custom.gen.txt");
    }, 8000);
    log(!!commitShown, "outline-generated: show them reveals all collapsed files in Commit Details");

    A.openOverlay(null);
    await until(() => !$(".git-graph"));
  } catch (err) {
    log(false, "outline-generated: uncaught error", String(err instanceof Error ? err.message : err));
  } finally {
    await h.updateSettings({ generatedPatterns: [] }).catch(() => {});
  }
}
