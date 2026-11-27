import { join } from "./core/path";
import * as A from "./state/actions";
import { resetCompareTabs } from "./state/compareTabs";
import type { gitSelftest } from "./selftest";

export async function runDiffPermissionSelfTest(h: typeof gitSelftest, root: string) {
  const { $, $$, key, click, until, sleep, shot, log, api } = h;
  const left = join(root, "left.txt"), right = join(root, "right.txt");
  const otherLeft = join(root, "other-left.txt"), otherRight = join(root, "other-right.txt");
  const file = (path: string) => ({ kind: "file" as const, path });
  const open = async (a: string, b: string) => {
    A.openOverlay({ type: "diff", a: file(a), b: file(b) });
    await until(() => $(".diffv .dv-row"), 6000);
    await sleep(200);
  };
  const rootElement = () => $<HTMLElement>(".diffv")!;
  const permission = () => $<HTMLInputElement>(".dv-allow-changes input")!;
  const copyButtons = () => $$(".dv-merge button[title^='Copy this change']");

  resetCompareTabs();
  await h.updateSettings({ theme: "dark", language: "en", vimKeys: false, keyboardProfile: "explorer" });
  await h.setWindowSize(1280, 860);
  const original = (await api.readTextFile(right)).text;
  await open(left, right);
  log(!!permission() && !permission().checked && copyButtons().length === 0, "diff permission: initially read-only");
  await shot("diff-permission-read-only");
  key("ArrowRight", { altKey: true }, rootElement());
  await sleep(200);
  log(!$(".dv-file.r .dv-dirty") && (await api.readTextFile(right)).text === original, "diff permission: keyboard copy is blocked");

  click(permission());
  await sleep(100);
  log(permission().checked && copyButtons().length === 2, "diff permission: enabling shows copy buttons");
  await shot("diff-permission-enabled");
  key("ArrowRight", { altKey: true }, rootElement());
  log(!!(await until(() => $(".dv-file.r .dv-dirty"), 3000)), "diff permission: enabled copy creates a draft");
  click(permission());
  await sleep(100);
  key("s", { ctrlKey: true }, rootElement());
  await sleep(200);
  log(!!$(".dv-file.r .dv-dirty") && (await api.readTextFile(right)).text === original, "diff permission: revocation blocks save and keeps draft");
  click(permission());
  await sleep(100);
  key("s", { ctrlKey: true }, rootElement());
  log(!!(await until(() => !$(".dv-file.r .dv-dirty"), 4000)) &&
    (await api.readTextFile(right)).text === (await api.readTextFile(left)).text, "diff permission: enabled save writes copied content");

  await open(otherLeft, otherRight);
  log(!permission().checked, "diff permission: another comparison starts read-only");
  await h.updateSettings({ keyboardProfile: "vim", vimKeys: true });
  key("ArrowRight", { altKey: true }, rootElement());
  await sleep(100);
  log(!$(".dv-file.r .dv-dirty"), "diff permission: Vim profile also blocks read-only copies");
  click(permission());
  await sleep(100);
  key("ArrowRight", { altKey: true }, rootElement());
  log(!!(await until(() => $(".dv-file.r .dv-dirty"), 3000)), "diff permission: Vim profile permits enabled copies");
  key("z", { ctrlKey: true }, rootElement());
  await until(() => !$(".dv-file.r .dv-dirty"), 3000);
  key("Tab", { ctrlKey: true }, rootElement());
  await until(() => $$(".cmp-tab")[0]?.classList.contains("on"), 4000);
  log(!permission().checked, "diff permission: returning to a comparison starts read-only");

  const firstOriginal = (await api.readTextFile(otherLeft)).text;
  const secondOriginal = (await api.readTextFile(otherRight)).text;
  const activeTab = () => $<HTMLButtonElement>(".cmp-tab.on [role=tab]");
  const discardPrompt = () => $(".modal h3")?.textContent?.trim() === "Discard unsaved changes?";
  const answerDiscard = async (id: "cancel" | "ok") => {
    const button = await until(() => $<HTMLButtonElement>(`.modal button[data-id="${id}"]`), 3000);
    if (button) click(button);
    await sleep(120);
  };
  const askToDiscard = async () => !!(await until(() => discardPrompt() ? $(".modal") : null, 3000));
  const draftFilesUnchanged = async () => (await api.readTextFile(otherLeft)).text === firstOriginal
    && (await api.readTextFile(otherRight)).text === secondOriginal;
  const makeDraft = async () => {
    const toggle = permission();
    if (toggle && !toggle.checked) click(toggle);
    await sleep(80);
    key("ArrowRight", { altKey: true }, rootElement());
    return !!(await until(() => $(".dv-file .dv-dirty"), 3000));
  };

  await open(otherLeft, otherRight);
  log($$(".cmp-tab").length === 2 && activeTab()?.title === "other-left.txt ↔ other-right.txt",
    "diff tab guard: opens a second comparison for navigation checks", `tabs=${$$(".cmp-tab").length}`);
  log(await makeDraft(), "diff tab guard: copied block creates an unsaved draft");
  key("Tab", { ctrlKey: true }, rootElement());
  const ctrlTabPrompt = await askToDiscard();
  if (ctrlTabPrompt) await answerDiscard("cancel");
  log(ctrlTabPrompt && activeTab()?.title === "other-left.txt ↔ other-right.txt" && !!$(".dv-dirty"),
    "diff tab guard: cancelling Ctrl+Tab keeps the draft");

  key("g", {}, rootElement());
  key("t", {}, rootElement());
  const vimPrompt = await askToDiscard();
  if (vimPrompt) await answerDiscard("cancel");
  log(vimPrompt && activeTab()?.title === "other-left.txt ↔ other-right.txt" && !!$(".dv-dirty"),
    "diff tab guard: cancelling Vim gt keeps the draft");

  const otherTab = $<HTMLButtonElement>(".cmp-tab:not(.on) [role=tab]");
  if (otherTab) click(otherTab);
  const mouseSwitchPrompt = await askToDiscard();
  if (mouseSwitchPrompt) await answerDiscard("cancel");
  log(mouseSwitchPrompt && activeTab()?.title === "other-left.txt ↔ other-right.txt" && !!$(".dv-dirty"),
    "diff tab guard: cancelling a mouse tab switch keeps the draft");

  const activeClose = $<HTMLButtonElement>(".cmp-tab.on .cmp-tab-x");
  if (activeClose) click(activeClose);
  const mouseClosePrompt = await askToDiscard();
  if (mouseClosePrompt) await answerDiscard("cancel");
  log(mouseClosePrompt && activeTab()?.title === "other-left.txt ↔ other-right.txt" && !!$(".dv-dirty"),
    "diff tab guard: cancelling the active-tab close keeps the draft");

  const focusedTab = activeTab();
  if (focusedTab) key("Delete", {}, focusedTab);
  const deletePrompt = await askToDiscard();
  if (deletePrompt) await answerDiscard("cancel");
  log(deletePrompt && activeTab()?.title === "other-left.txt ↔ other-right.txt" && !!$(".dv-dirty"),
    "diff tab guard: cancelling keyboard tab close keeps the draft");

  const confirmClose = $<HTMLButtonElement>(".cmp-tab.on .cmp-tab-x");
  if (confirmClose) click(confirmClose);
  const discardPromptShown = await askToDiscard();
  if (discardPromptShown) await answerDiscard("ok");
  await until(() => activeTab()?.title === "left.txt ↔ right.txt", 4000);
  log(discardPromptShown && await draftFilesUnchanged(),
    "diff tab guard: explicit discard switches tabs without writing files");

  await open(otherLeft, otherRight);
  const cleanOther = $<HTMLButtonElement>(".cmp-tab:not(.on) [role=tab]");
  if (cleanOther) click(cleanOther);
  await until(() => activeTab()?.title === "left.txt ↔ right.txt", 4000);
  await sleep(160);
  const cleanClose = $<HTMLButtonElement>(".cmp-tab.on .cmp-tab-x");
  if (cleanClose) click(cleanClose);
  await until(() => $(".dv-file.l")?.title === otherLeft && $(".dv-file.r")?.title === otherRight, 4000);
  await sleep(160);
  log(!$(".modal") && $(".dv-file.l")?.title === otherLeft && $(".dv-file.r")?.title === otherRight,
    "diff tab guard: switching and closing clean tabs needs no confirmation");

  await open(left, right);
  const secondDraftTab = $<HTMLButtonElement>(".cmp-tab:not(.on) [role=tab]");
  if (secondDraftTab) click(secondDraftTab);
  await until(() => activeTab()?.title === "other-left.txt ↔ other-right.txt", 4000);
  await h.updateSettings({ keyboardProfile: "explorer", vimKeys: false });
  log(await makeDraft(), "diff tab guard: copied block creates a draft with Vim keys disabled");
  key("Tab", { ctrlKey: true }, rootElement());
  const explorerPrompt = await askToDiscard();
  if (explorerPrompt) await answerDiscard("cancel");
  key("g", {}, rootElement());
  key("t", {}, rootElement());
  await sleep(120);
  log(explorerPrompt && !discardPrompt() && activeTab()?.title === "other-left.txt ↔ other-right.txt" && !!$(".dv-dirty"),
    "diff tab guard: Ctrl+Tab cancellation retains the draft and gt is inactive without Vim keys");

  const keyboardClose = $<HTMLButtonElement>(".cmp-tab.on [role=tab]");
  if (keyboardClose) key("Delete", {}, keyboardClose);
  const keyboardDiscard = await askToDiscard();
  if (keyboardDiscard) await answerDiscard("ok");
  await until(() => activeTab()?.title === "left.txt ↔ right.txt", 4000);
  log(keyboardDiscard && await draftFilesUnchanged(),
    "diff tab guard: explicit keyboard close discards only in-memory edits without saving");
}
