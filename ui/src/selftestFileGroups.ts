import type { gitSelftest } from "./selftest";
import type { GroupMode, GroupOrder } from "./api/types";
import { api } from "./api/ipc";
import { parent } from "./core/path";
import { controller } from "./state/controllers";
import { run } from "./state/run";
import * as A from "./state/actions";

/** The grouping of the file list by type: every mode and order, headers with keys and mouse, memory and edge cases. */
export async function runFileGroupsSelfTest(t: typeof gitSelftest) {
  const { $, $$, until, sleep, shot, log, updateSettings, focusPane, click, key, finish } = t;
  const errors: string[] = [];
  window.addEventListener("error", (e) => errors.push(e.message));
  window.addEventListener("unhandledrejection", (e) => errors.push(String(e.reason)));
  try {
    const start = A.activePath(0), sep = start.includes("\\") ? "\\" : "/";
    const root = parent(start)?.endsWith("td") ? parent(start)! : start;
    const at = (name: string) => `${root}${sep}${name}`;
    const pane = () => $$(".browser")[0].querySelector<HTMLElement>(".folder-pane")!;
    const heads = () => Array.from(pane().querySelectorAll<HTMLElement>("[data-group]"));
    const head = (id: string) => heads().find((h) => h.dataset.group === id);
    const ids = () => heads().map((h) => h.dataset.group).join(" ");
    const names = () => Array.from(pane().querySelectorAll<HTMLElement>("[data-path]")).map((r) => r.dataset.path!.split(/[/\\]/).pop()!);
    /** Rows and headers as they are on screen. */
    const lines = () => Array.from(pane().querySelectorAll<HTMLElement>("[data-group], [data-path]")).sort((a, b) => a.getBoundingClientRect().top - b.getBoundingClientRect().top).map((el) => el.dataset.group ? `[${el.dataset.group}]` : el.dataset.path!.split(/[/\\]/).pop()!).join(" ");
    const selected = () => Array.from(pane().querySelectorAll<HTMLElement>(".row.selected, .grid-cell.selected")).map((r) => r.dataset.path!.split(/[/\\]/).pop()!).join(" ");
    const cursor = () => pane().querySelector<HTMLElement>(".fp-group.focused")?.dataset.group ?? "";
    const press = async (k: string, opts: KeyboardEventInit = {}) => { pane().focus(); key(k, opts, pane()); await sleep(140); };
    const go = async (name: string) => { A.navigate(0, at(name)); await until(() => A.activePath(0) === at(name) && controller(0)?.path() === at(name), 5000); await sleep(500); };
    const group = async (mode: GroupMode, order: GroupOrder = "name", descending = false) => { await api.setPrefs(A.activePath(0), { group: mode, groupOrder: order, sortKey: "name", descending, collapsed: [] }); await updateSettings({ ageSaturation: 0.55 }); const p = A.activePath(0); A.navigate(0, root); await sleep(250); A.navigate(0, p); await sleep(600); };
    await updateSettings({ keyboardProfile: "vim", rememberFolderViews: true, adaptiveLayout: true, groupHeaders: true, groupByKind: false, showHidden: true });
    await focusPane(0);

    // Every mode and every order, with the same folder.
    await go("mixed");
    const expected: [GroupMode, GroupOrder, string][] = [
      ["none", "name", ""], ["folders", "name", ""],
      ["type", "name", "ext:gz ext:md ext:png ext:txt dot none dir"], ["foldersType", "name", "dir ext:gz ext:md ext:png ext:txt dot none"],
      ["foldersType", "count", "dir ext:png none ext:txt ext:gz ext:md dot"], ["foldersType", "size", "dir ext:gz ext:txt ext:png none dot ext:md"],
      ["foldersType", "recent", "dir ext:md ext:txt ext:png ext:gz dot none"],
    ];
    for (const [mode, order, want] of expected) {
      await group(mode, order);
      log(ids() === want, `${mode} by ${order}: the groups in order`, ids() || lines());
      await shot(`groups-${mode}-${order}`);
    }
    await group("foldersType", "name", true);
    log(ids() === expected[3][2] && names().join(" ").includes("c10.png c2.png b.PNG a.png"), "descending turns the order inside the groups, not the groups", lines());
    await updateSettings({ groupByKind: true }); await sleep(400);
    log(ids() === "dir kind:archive kind:code kind:document kind:image kind:other", "kinds instead of extensions", ids()); await shot("groups-kinds");
    await updateSettings({ groupByKind: false, groupHeaders: false }); await sleep(400);
    log(!heads().length && names().join(" ").startsWith("src Docs pack.tar.gz x.md c10.png"), "without headers: flat, still grouped", lines()); await shot("groups-flat");
    await updateSettings({ groupHeaders: true });
    await group("foldersType");
    log(/PNG4.*450/.test(head("ext:png")?.textContent?.replace(/\s/g, "") ?? ""), "a header has the type, the count and the total size", head("ext:png")?.textContent ?? "");
    const row = pane().querySelector<HTMLElement>("[data-path]")!.getBoundingClientRect(), header = heads()[0].getBoundingClientRect();
    log(Math.abs(header.bottom - row.top) < 1.5 && header.height >= row.height, "headers and rows do not overlap", `${header.bottom} ${row.top}`);

    const size = (el: Element | null | undefined) => el?.getBoundingClientRect().right ?? 0;
    const cols = pane().querySelectorAll("[data-path] .col"), sizeCell = cols[cols.length - 1];
    log(Math.abs(size(head("ext:png")?.querySelector(".size")) - size(sizeCell)) < 1.5, "the total of a group stands under the Size column", `${size(head("ext:png")?.querySelector(".size"))} ${size(sizeCell)}`);

    // Keys.
    await press("g"); await press("g");
    log(selected() === "Docs", "gg goes to the first entry", selected());
    await press("k"); log(cursor() === "dir" && !selected(), "k above it is the header", cursor());
    await press("h"); log(!names().includes("Docs") && cursor() === "dir" && A.activePath(0) === at("mixed"), "h on a header collapses the group", lines());
    await press("j"); log(cursor() === "ext:gz", "j goes to the next header", cursor());
    await press("k"); await press("l"); log(names().includes("Docs") && cursor() === "dir", "l on a collapsed header opens it", lines());
    await press("l"); log(selected() === "Docs", "l on an open header goes to its first entry", selected());
    await press("j"); await press("z"); await press("a");
    log(!names().includes("src") && cursor() === "dir", "za collapses the group of the cursor", cursor());
    await press("Enter"); log(names().includes("src"), "Enter on a header opens it", lines());
    await press("z"); await press("M", { shiftKey: true }); log(!names().length && heads().length === 7, "zM collapses all groups", lines()); await shot("groups-collapsed");
    log((await api.resolvePrefs(at("mixed"))).collapsed.length === 7, "collapsed groups are remembered for the folder");
    await go("single"); await go("mixed");
    log(!names().length && heads().length === 7 && cursor() === "dir", "and collapsed again when the folder is opened", `${lines()} ${cursor()}`);
    // The quick filter opens collapsed groups only when nothing else matches.
    await press("/"); for (const c of "png") await press(c);
    log(names().length === 4, "the filter finds entries of collapsed groups", lines());
    await press("Escape"); await sleep(200);
    await press("z"); await press("R", { shiftKey: true }); log(names().length === 14, "zR expands all groups", lines());
    controller(0)!.select([`${at("mixed")}${sep}a.png`], `${at("mixed")}${sep}a.png`); await sleep(300);
    await press("z"); await press("a");
    controller(0)!.select([`${at("mixed")}${sep}c2.png`], `${at("mixed")}${sep}c2.png`); await sleep(500);
    log(selected() === "c2.png", "selecting an entry of a collapsed group opens the group", `${selected()} | ${lines()}`);

    // Selection across groups: v with motions, Shift with the mouse, Shift with arrows.
    controller(0)!.select([`${at("mixed")}${sep}x.md`], `${at("mixed")}${sep}x.md`); await sleep(300);
    await press("v"); await press("j"); await press("j");
    log(selected() === "x.md a.png b.PNG", "visual mode selects across headers", selected()); await press("Escape");
    const rowOf = (name: string) => Array.from(pane().querySelectorAll<HTMLElement>("[data-path]")).find((r) => r.dataset.path!.endsWith(sep + name))!;
    click(rowOf("c10.png")); await sleep(150); click(rowOf("notes.txt"), { shiftKey: true }); await sleep(200);
    log(selected() === "c10.png notes.txt", "Shift+click selects across groups", selected()); await shot("groups-selection");
    click(head("ext:txt")!); await sleep(300);
    log(!names().includes("notes.txt") && !selected().includes("notes.txt"), "a click collapses; hidden entries are not selected", `${selected()} | ${lines()}`);
    click(head("ext:txt")!); await sleep(300);
    log(names().includes("notes.txt"), "a second click expands");

    // Without Vim keys: arrows and the cycle key.
    await updateSettings({ keyboardProfile: "explorer" }); await sleep(300);
    await press("Home"); await press("ArrowUp"); await press("ArrowLeft");
    log(cursor() === "dir" && !names().includes("Docs"), "Left on a header collapses it", lines());
    await press("ArrowRight"); log(names().includes("Docs"), "Right opens it");
    await press("ArrowLeft"); await press("ArrowLeft"); await sleep(500);
    log(A.activePath(0) === root, "Left on a collapsed header goes to the parent folder", A.activePath(0));
    await go("mixed"); await press("F7", { ctrlKey: true }); await sleep(500);
    log((await api.resolvePrefs(at("mixed"))).group === "none" && !heads().length, "Ctrl+F7 goes to the next grouping", lines());
    run("groupNext"); await sleep(500); run("toggleFoldersOnTop"); await sleep(500);
    log((await api.resolvePrefs(at("mixed"))).group === "none", "Folders on top switches only the folders");
    // The menus show the mode, its scope and the choices.
    click(pane().querySelector(".fp-pill button")!); await sleep(300);
    const item = () => Array.from(document.querySelectorAll<HTMLElement>(".mi")).find((m) => m.textContent?.startsWith("Group by"));
    log(!!item(), "the sort control has the grouping");
    if (item()) { item()!.dispatchEvent(new MouseEvent("mouseover", { bubbles: true })); item()!.dispatchEvent(new MouseEvent("mousemove", { bubbles: true })); item()!.dispatchEvent(new MouseEvent("mouseenter", { bubbles: false })); await sleep(500); }
    log(!!Array.from(document.querySelectorAll(".mi")).find((m) => m.textContent?.includes("Set for this folder and those below")), "and says where it applies"); await shot("groups-menu");
    key("Escape"); await sleep(150); key("Escape"); await sleep(200);

    // Views, edge cases.
    await group("foldersType");
    for (const view of ["grid", "thumbs", "names", "powershell"]) {
      await api.setPrefs(at("mixed"), { view }); await go("single"); await go("mixed");
      log(heads().length > 3 && names().length > 3, `${view}: headers and entries`, lines()); await shot(`groups-view-${view}`);
    }
    await api.setPrefs(at("mixed"), { view: "default" });
    for (const name of ["empty", "single", "many"]) {
      await go(name); await group("foldersType");
      const n = heads().length;
      log(name === "empty" ? n === 0 && !!$(".empty-msg") : name === "single" ? ids() === "ext:txt" : n > 3, `${name}: headers`, ids().slice(0, 80));
      await shot(`groups-${name}`);
    }
    run("groupCollapseAll"); await sleep(400); await press("End"); await sleep(400);
    log(cursor() === "rest", "End goes to the last header when all groups are collapsed", cursor());
    log(heads().some((h) => h.dataset.group === "rest" && /Other \(100 rarer types\)/.test(h.textContent ?? "")), "the rarest of 300 types share the last group", ids()); await shot("groups-many-end");
    log(!errors.length, "no errors", errors.join("; "));
  } catch (e) {
    log(false, "the self-test ran to its end", String(e));
  }
  await finish();
}
