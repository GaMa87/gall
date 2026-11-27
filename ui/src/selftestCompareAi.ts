import { api } from "./api/ipc";
import { join } from "./core/path";
import * as A from "./state/actions";

/**
 * Running-app scenario for WN-159:
 * 1. Settings › Advanced › Local explanation model in Slovenian, German and Spanish (at host 150% scale):
 *    checks the file-picker Browse button, a wrong (relative) program path, a missing absolute path,
 *    and the Test message against `compare_ai_fake`, capturing screenshots for each language.
 * 2. DiffView single-hunk explanation and item-to-hunk linking: explains Change 2 alone, verifies
 *    Change 2 is linked and clicking it jumps to block 2, then switches scope to All changes.
 */
export async function runCompareAiSelfTest(h: typeof import("./selftest").gitSelftest, rootDir: string) {
  const { $, $$, click, until, sleep, shot, setWindowSize, log, typeInto } = h;
  const fakePathFile = join(rootDir, "compare-ai-fake-path.txt");
  let fakeProgram = "";
  try {
    fakeProgram = (await api.readTextFile(fakePathFile)).text.trim();
  } catch {
    log(true, "compare-ai: skipped (compare_ai_fake path fixture not provided)");
    return;
  }

  const openAdvancedSettings = async (advTitle: string) => {
    A.openOverlay({ type: "settings" });
    const modal = await until(() => $<HTMLElement>(".settings"), 8000);
    if (!modal) throw new Error("Settings modal did not open");
    const advBtn = await until(
      () => $$<HTMLButtonElement>(".sbody nav button", modal).find((b) => b.textContent?.trim() === advTitle),
      6000,
    );
    if (!advBtn) throw new Error(`Advanced section button "${advTitle}" missing`);
    click(advBtn);
    const group = await until(() => $$<HTMLElement>("section.sgroup", modal).find((g) => g.querySelector("textarea")), 8000);
    if (!group) throw new Error("CompareAiSettings group did not load");
    await sleep(100);
    return { modal, group };
  };

  try {
    await setWindowSize(1280, 860);

    // ── 1. Settings › Advanced › Local explanation model in sl, de, es ──
    const locales: { code: "sl" | "de" | "es"; adv: string; browse: string; save: string; test: string; okWord: string }[] = [
      { code: "sl", adv: "Napredno", browse: "Prebrskaj …", save: "Shrani", test: "Preizkusi", okWord: "fine" },
      { code: "de", adv: "Erweitert", browse: "Durchsuchen…", save: "Speichern", test: "Testen", okWord: "fine" },
      { code: "es", adv: "Avanzado", browse: "Explorar…", save: "Guardar", test: "Probar", okWord: "fine" },
    ];

    for (const loc of locales) {
      await api.compareAiConfigSave({
        enabled: false,
        program: "",
        args: [],
        model: "",
        deadlineSecs: 30,
        env: [],
      });
      await h.updateSettings({ language: loc.code, theme: "dark", keyboardProfile: "vim" });
      await sleep(100);
      const { group } = await openAdvancedSettings(loc.adv);
      group.scrollIntoView({ block: "end" });
      await sleep(80);

      const enabledBox = $<HTMLInputElement>('input[type="checkbox"]', group)!;
      const inputs = $$<HTMLInputElement>('input[type="text"]', group);
      const progInput = inputs[0]!;
      const modelInput = inputs[1]!;
      const argsArea = $<HTMLTextAreaElement>("textarea", group)!;
      const buttons = $$<HTMLButtonElement>("button", group);
      const browseBtn = buttons.find((b) => b.textContent?.trim() === loc.browse);
      const saveBtn = buttons.find((b) => b.textContent?.trim() === loc.save);
      const testBtn = buttons.find((b) => b.textContent?.trim() === loc.test);

      log(!!browseBtn && !browseBtn.disabled, `compare-ai (${loc.code}): Browse file picker button is present and enabled`, browseBtn?.textContent ?? "missing");

      // Turn on and test a wrong (relative) program path.
      if (!enabledBox.checked) click(enabledBox);
      typeInto(progInput, "relative-wrapper");
      click(saveBtn!);
      const badRelMsg = await until(() => {
        const s = $<HTMLElement>('[role="status"]', group);
        return s && s.textContent && s.textContent.trim().length > 0 ? s.textContent.trim() : null;
      }, 6000);
      log(!!badRelMsg && badRelMsg.toLowerCase().includes("absolute"), `compare-ai (${loc.code}): relative program path is refused with message`, badRelMsg ?? "none");

      // Test a missing absolute program path.
      typeInto(progInput, "/definitely/not/here");
      typeInto(modelInput, "local-test");
      typeInto(argsArea, "ok");
      click(testBtn!);
      const badAbsMsg = await until(() => {
        const s = $<HTMLElement>('[role="status"]', group);
        const t = s?.textContent?.trim() ?? "";
        return t && !t.includes("absolute") && !t.endsWith("…") && t !== "Saved." && t !== "Shranjeno." && t !== "Gespeichert." && t !== "Guardado." ? t : null;
      }, 8000);
      log(!!badAbsMsg && badAbsMsg.includes("/definitely/not/here"), `compare-ai (${loc.code}): missing absolute program reports failure on Test`, badAbsMsg ?? "none");

      // Configure compare_ai_fake (`ok`) and verify Test succeeds.
      typeInto(progInput, fakeProgram);
      typeInto(argsArea, "ok");
      typeInto(modelInput, `fake-${loc.code}`);
      click(testBtn!);
      const okMsg = await until(() => {
        const s = $<HTMLElement>('[role="status"]', group);
        return s?.textContent?.includes(loc.okWord) ? s.textContent.trim() : null;
      }, 8000);
      log(!!okMsg && okMsg.includes(loc.okWord), `compare-ai (${loc.code}): Test reports model answer`, okMsg ?? "none");
      await sleep(120);
      await shot(`compare-ai-settings-${loc.code}`);
      h.note(`SCREENSHOT compare-ai-settings-${loc.code}.png: Settings › Advanced › Local explanation model at 150% in ${loc.code}, showing Browse button, program, arguments, model, deadline, Save/Test and Test message.`);
      A.openOverlay(null);
      await until(() => !$(".settings"));
    }

    // ── 2. DiffView single-hunk explanation and item-to-hunk link ──
    await h.updateSettings({ language: "en", theme: "dark", keyboardProfile: "vim" });
    await api.compareAiConfigSave({
      enabled: true,
      program: fakeProgram,
      args: ["ok"],
      model: "fake-hunk",
      deadlineSecs: 30,
      env: [],
    });

    const leftText = ["fn one() { 1 }", "let a = 1;", "let b = 2;", "let c = 3;", "let d = 4;", "fn two() { 10 }"].join("\n") + "\n";
    const rightText = ["fn one() { 2 }", "let a = 1;", "let b = 2;", "let c = 3;", "let d = 4;", "fn two() { 20 }"].join("\n") + "\n";
    A.openOverlay({
      type: "diff",
      a: { kind: "text", label: "left.rs", text: leftText },
      b: { kind: "text", label: "right.rs", text: rightText },
    });
    const diff = await until(() => $<HTMLElement>(".diffv"), 8000);
    if (!diff) throw new Error("DiffView did not open");

    // Click Explain on Change 2 (`Explain this change (2)`).
    const explainSecond = await until(() => $<HTMLButtonElement>('button[aria-label="Explain this change (2)"]', diff), 8000);
    log(!!explainSecond, "compare-ai: per-hunk Explain button is shown for Change 2");
    click(explainSecond!);

    const panel = await until(() => {
      const p = $<HTMLElement>(".dv-explain", diff);
      return p && p.textContent?.includes("fake-hunk") ? p : null;
    }, 8000);
    log(!!panel && panel.textContent!.includes("Change 2"), "compare-ai: single-hunk explanation shows scope and item for Change 2", panel?.textContent?.slice(0, 140) ?? "missing");

    // Click the item link `Jump to change 2` and verify active change indicator shows `2 / 2`.
    const jumpItem = $<HTMLButtonElement>('button[aria-label="Jump to change 2"]', panel!);
    log(!!jumpItem, "compare-ai: item in explanation panel is linked to Change 2");
    if (jumpItem) {
      click(jumpItem);
      const countText = await until(() => {
        const el = $<HTMLElement>(".dv-count", diff);
        return el?.textContent?.includes("2 / 2") ? el.textContent.trim() : null;
      }, 4000);
      log(!!countText, "compare-ai: clicking explanation item jumps to Change 2", countText ?? "missing");
    }
    await sleep(120);
    await shot("compare-ai-hunk-panel");
    h.note("SCREENSHOT compare-ai-hunk-panel.png: DiffView showing single-hunk explanation for Change 2 with scope buttons, clickable Change 2 item link and evidence link.");

    // Switch scope to All changes.
    const allBtn = $$<HTMLButtonElement>(".dv-explain-scope button", panel!).find((b) => b.textContent?.trim() === "All changes");
    if (allBtn) {
      click(allBtn);
      const updated = await until(() => $(".dv-explain")?.textContent?.includes("All changes") && $(".dv-explain")?.textContent?.includes("Change 1"), 8000);
      log(!!updated, "compare-ai: switching scope to All changes refreshes explanation for Change 1");
    }

    A.openOverlay(null);
    await until(() => !$(".diffv"));
  } catch (err) {
    log(false, "compare-ai: uncaught error", String(err instanceof Error ? err.message : err));
  } finally {
    await api.compareAiConfigSave({
      enabled: false,
      program: "",
      args: [],
      model: "",
      deadlineSecs: 30,
      env: [],
    }).catch(() => {});
  }
}
