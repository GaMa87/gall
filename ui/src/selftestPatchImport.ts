// Graphical self-test for WN-133: the Apply a patch dialog in the running app. Patches written by GNU diff in the three
// formats are applied to copies of one file (opened from the folder menu, from a focused patch file and with the file
// given); a preview with problems applies nothing; a file changed between Check and Apply and a failed write are
// refused, the file already written put back; Esc leaves a field before it closes; the compare window applies the patch
// of its text and the dialog closes back to it. The fixture is made by scripts/selftest.sh --patch-import.
import { join, parent } from "./core/path";
import * as A from "./state/actions";
import type { gitSelftest } from "./selftest";

type Helpers = typeof gitSelftest;

export async function runPatchImportSelfTest(h: Helpers, rootDir: string) {
  const { $, $$, key, click, mouse, typeInto, until, sleep, shot, log, api, focusPane } = h;
  const P = join(parent(rootDir) ?? rootDir, "patch-import");
  const at = (...parts: string[]) => parts.reduce((path, part) => join(path, part), P);
  const read = async (path: string) => (await api.readTextFile(path)).text;
  const original = await read(at("orig", "f.txt")), patched = await read(at("new", "f.txt"));

  const dialog = () => $('.sm[role="dialog"]');
  const textarea = () => $<HTMLTextAreaElement>(".sm textarea");
  const field = (label: string) => $$<HTMLLabelElement>(".sm label.sm-row").find((l) => l.querySelector("span")?.textContent === label)?.querySelector("input") ?? null;
  const button = (label: string) => $$<HTMLButtonElement>(".sm .sm-actions button").find((b) => b.textContent === label) ?? null;
  const files = () => $$(".pi-files li").map((li) => li.textContent ?? "");
  const problems = () => $$(".pi-problems li").map((li) => li.textContent ?? "");
  const alert = () => $('.sm .gm-note[role="alert"]')?.textContent ?? "";
  const status = () => $('.pi-result [role="status"]')?.textContent ?? "";
  const previewed = () => until(() => files().length > 0 || problems().length > 0, 5000);
  const idle = () => until(() => !!button("Check") && !button("Check")!.disabled, 5000);
  const close = async () => {
    A.openOverlay(null);
    await until(() => !dialog() && !$(".diffv"), 3000);
    await sleep(100);
  };
  const open = async (o: { root?: string; file?: string; text?: string }) => {
    await close();
    A.openOverlay({ type: "applyPatch", ...o });
    const ready = await until(() => dialog(), 3000);
    if (o.file) await until(() => (textarea()?.value ?? "").length > 0, 3000);
    return ready;
  };
  const rowOf = (name: string) => $$(".browser .fp-list [data-path]").find((r) => r.getAttribute("data-path")!.replace(/\\/g, "/").endsWith("/" + name)) ?? null;
  const goTo = async (folder: string, name: string) => {
    A.navigate(0, folder);
    const row = await until(() => rowOf(name), 4000);
    if (!row) throw new Error(`${name} not listed in ${folder}`);
    click(row);
    await focusPane(0);
    click(row);
    await sleep(150);
    return row;
  };

  try {
    await h.updateSettings({ theme: "dark" });
    await h.setWindowSize(1280, 860);

    // ── Unified, from the menu of the patch file; its hunks are found two lines later than stated ──
    const unified = await goTo(at("uni"), "fix.patch");
    mouse(unified, "contextmenu", { button: 2 });
    const item = await until(() => $$(".menu .mi").find((x) => x.textContent?.includes("Apply this patch")), 3000);
    log(!!item, "patch-import: a .patch file offers Apply this patch… in its menu");
    if (item) click(item);
    await until(() => dialog() && (textarea()?.value ?? "").length > 0, 3000);
    log(field("Folder")?.value === at("uni"), "patch-import: the menu opens the dialog on the folder of the file", field("Folder")?.value ?? "");
    log((textarea()?.value ?? "").startsWith("--- a/f.txt"), "patch-import: the menu reads the text of the patch file", (textarea()?.value ?? "").slice(0, 40));
    log((dialog()?.textContent ?? "").includes("fix.patch"), "patch-import: the dialog names the file the text came from");
    log(await until(() => (dialog()?.textContent ?? "").includes("Recognised: unified"), 2000) !== null, "patch-import: the unified format is recognised");
    await shot("pi-01-unified-loaded");
    key("Enter", {}, field("Folder"));
    await previewed();
    log(files().length === 1 && files()[0].includes("2 lines later than stated"), "patch-import: Enter in a field checks; the preview says where the hunks were found", files().join(" | ") || problems().join(" | "));
    await shot("pi-02-unified-preview");
    key("Enter", { ctrlKey: true }, textarea());
    await until(() => status() || alert(), 5000);
    log(status().includes("Applied to 1 file:") && status().includes("f.txt"), "patch-import: Ctrl+Enter applies a checked patch (one file, singular)", status() || alert());
    log(await read(at("uni", "f.txt")) === "added 1\nadded 2\n" + patched, "patch-import: unified patch written below the moved lines");
    textarea()?.focus();
    key("Escape", {}, textarea());
    await sleep(100);
    log(!!dialog() && document.activeElement === dialog(), "patch-import: Esc in a field leaves the field and keeps the dialog");
    key("Escape", {}, dialog());
    log(await until(() => !dialog(), 2000) !== null, "patch-import: Esc on the dialog closes it");

    // ── Context, with the file given ──
    await open({ root: at("ctx"), file: at("ctx", "fix.patch") });
    log(await until(() => (dialog()?.textContent ?? "").includes("Recognised: context"), 2000) !== null, "patch-import: the context format is recognised");
    click(button("Check")!);
    await previewed();
    log(files().length === 1 && problems().length === 0, "patch-import: context patch previews one file", files().join(" | ") || problems().join(" | "));
    await shot("pi-03-context-preview");
    click(button("Apply")!);
    await until(() => status() || alert(), 5000);
    log(await read(at("ctx", "f.txt")) === patched, "patch-import: context patch applied", alert());

    // ── Normal, from the focused patch file; it has no file name, so the dialog asks for one ──
    await close();
    await goTo(at("nor"), "fix.diff");
    const { run: runAction } = await import("./state/run");
    runAction("applyPatch");
    await until(() => dialog() && (textarea()?.value ?? "").length > 0, 3000);
    log((textarea()?.value ?? "").startsWith("5c5"), "patch-import: the action opens on the focused patch file", (textarea()?.value ?? "").slice(0, 20));
    log(!!(await until(() => field("File"), 2000)), "patch-import: a normal diff shows the File field once its format is recognised");
    click(button("Check")!);
    await previewed();
    log(problems().some((p) => p.includes("without a file name")), "patch-import: without a file a normal diff is a problem", problems().join(" | "));
    log(!!button("Apply")?.disabled, "patch-import: Apply is disabled while there is a problem");
    typeInto(field("File")!, "f.txt");
    await sleep(50);
    log(!!button("Apply")?.disabled && (dialog()?.textContent ?? "").includes("changed since the check"), "patch-import: a setting changed after the check disables Apply");
    key("Enter", {}, field("File"));
    await idle();
    await until(() => files().length > 0, 5000);
    await shot("pi-04-normal-preview");
    click(button("Apply")!);
    await until(() => status() || alert(), 5000);
    log(await read(at("nor", "f.txt")) === patched, "patch-import: normal patch applied to the file given", alert());

    // ── A patch that does not fit: problems listed, nothing written, Ctrl+Enter checks again ──
    const bad = await read(at("bad", "f.txt"));
    await open({ root: at("bad"), file: at("bad", "fix.patch") });
    click(button("Check")!);
    await previewed();
    log(problems().length > 0 && files().length === 0, "patch-import: a hunk that does not fit is listed as a problem", problems().join(" | "));
    log(!!button("Apply")?.disabled, "patch-import: a preview with problems cannot be applied");
    key("Enter", { ctrlKey: true }, dialog());
    await idle();
    await shot("pi-05-problems");
    log(await read(at("bad", "f.txt")) === bad, "patch-import: a preview with problems writes nothing");

    // ── A file changed between Check and Apply ──
    await open({ root: at("race"), file: at("race", "fix.patch") });
    click(button("Check")!);
    await previewed();
    const changed = original + "changed after the check\n";
    await api.saveText(at("race", "f.txt"), changed);
    click(button("Apply")!);
    await until(() => alert(), 5000);
    log(/changed since (the preview|it was checked)/.test(alert()), "patch-import: a file changed since the check is refused", alert());
    log(await read(at("race", "f.txt")) === changed, "patch-import: the changed file is left as it is");
    await shot("pi-06-changed-file");

    // ── A failed write: the file already written is put back ──
    await open({ root: at("wr"), file: at("wr", "fix.patch") });
    click(button("Check")!);
    await previewed();
    log(files().length === 2, "patch-import: a patch of two files previews both", files().join(" | ") || problems().join(" | "));
    if (files().length === 2) {
      click(button("Apply")!);
      await until(() => alert(), 5000);
      log(alert().length > 0, "patch-import: a write that fails is reported", alert());
      log(await read(at("wr", "one", "a.txt")) === original, "patch-import: the file written before the failure is put back");
      await shot("pi-07-write-failure");
    }

    // ── The compare window: p applies the patch of the text on screen; the dialog closes back to the comparison ──
    await close();
    A.openOverlay({ type: "diff", a: { kind: "file", path: at("cmp", "left.txt") }, b: { kind: "file", path: at("cmp", "right.txt") } });
    await until(() => $(".diffv .dv-row"), 6000);
    log(($(".diffv .dv-foot")?.textContent ?? "").includes("apply patch"), "patch-import: the compare key help lists p");
    log(!!$('.diffv button[aria-label^="Apply this patch to a folder"]'), "patch-import: the compare toolbar has the apply button");
    key("p", {}, $(".diffv"));
    await until(() => dialog() && (textarea()?.value ?? "").length > 0, 4000);
    log(field("Folder")?.value === at("cmp"), "patch-import: the compare window opens the dialog on the folder of the left file", field("Folder")?.value ?? "");
    log((textarea()?.value ?? "").includes("a/left.txt"), "patch-import: the dialog holds the patch of the comparison", (textarea()?.value ?? "").slice(0, 60));
    click(button("Check")!);
    await previewed();
    await shot("pi-08-from-compare");
    key("Enter", { ctrlKey: true }, dialog());
    await until(() => status() || alert(), 5000);
    log(await read(at("cmp", "left.txt")) === patched, "patch-import: the patch of the comparison turns the left file into the right", alert());
    key("Escape", {}, dialog());
    log(await until(() => !dialog() && $(".diffv"), 3000) !== null, "patch-import: closing the dialog comes back to the comparison");
  } catch (e) {
    log(false, "patch-import: scenario threw", String(e instanceof Error ? e.stack ?? e.message : e));
  } finally {
    await close().catch(() => {});
  }
}
