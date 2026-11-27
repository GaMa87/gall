import { getCurrentWebview } from "@tauri-apps/api/webview";
import { invoke } from "@tauri-apps/api/core";
import * as A from "./state/actions";
import type { Repository } from "./core/gitRepository";

export async function runGitWorkspaceSelfTest(h: typeof import("./selftest").gitSelftest) {
  const { $, $$, key, click, typeInto, until, sleep, shot, setWindowSize, log } = h;
  const button = (text: string, scope: ParentNode = document) => $$<HTMLButtonElement>("button", scope).find((b) => (() => { const label = b.cloneNode(true) as HTMLElement; label.querySelectorAll("kbd,svg,.accel-key").forEach((node) => node.remove()); return label.textContent?.trim() === text; })());
  const press = async (text: string, scope: ParentNode = document) => { const element = await until(() => button(text, scope) ?? $$(".mi", scope).find((row) => $(".lbl", row)?.textContent?.trim() === text)); if (!element) throw new Error(`Missing button ${text}`); click(element); };
  const dialog = async (label: string, value?: string) => {
    await until(() => $(".modal .buttons"));
    if (value !== undefined) { const field = $(".modal textarea, .modal input") as HTMLInputElement | HTMLTextAreaElement; if (!field) throw new Error("Missing dialog input"); typeInto(field, value); await sleep(70); }
    await press(label, $(".modal")!); await until(() => !$(".modal"));
  };
  const settle = async () => { await sleep(180); return until(() => !($(".gg-actions") as HTMLButtonElement)?.disabled, 12000); };
  const history = (text: string) => $$(".git-graph-lines [data-commit]").find((row) => row.textContent?.includes(text));
  let path = "";
  // Windows paths use backslashes and the self-test also runs at a webview zoom chosen by the wrapper (mode "git-workspace@1.5").
  const sep = () => (path.includes("\\") ? "\\" : "/");
  const parentOf = (p: string) => p.slice(0, Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\")));
  // Below 1000 CSS px (a 200 % display with the 1320 px window) the repository panels start hidden; the scenario needs them.
  const showPanels = async () => { if (!$(".gg-sidebar")) { const toggle = await until(() => $("[aria-label='Toggle repository panels']")); if (toggle) { click(toggle); await until(() => $(".gg-sidebar")); } } };
  const baseZoom = () => (window as unknown as { __selftestZoom?: number }).__selftestZoom ?? 1;
  // A narrow window (a half tile, a phone-sized window). The compositor of a self-test run does not resize the window, so
  // the CSS viewport is made narrow with the webview zoom: the window is zoomed until it is about 640 CSS px wide, below
  // the 700 px of the narrow layout rules.
  const narrow = async (on: boolean) => { await getCurrentWebview().setZoom(on ? Math.max(baseZoom(), window.innerWidth * baseZoom() / 640) : baseZoom()).catch(() => {}); await sleep(450); };
  // WN-63: geometry checks that fail on a label cut by its own box, a pill under a button, a select whose chevron covers its
  // text, a white square where two scrollbars meet and a tools panel squeezed to its tab row. `where` names the state.
  const layoutAudit = (where: string) => {
    const problems: string[] = [];
    const name = (el: Element) => `${(el as HTMLElement).className || el.tagName}:${(el.textContent ?? "").trim().slice(0, 24)}`;
    const rect = (el: Element) => el.getBoundingClientRect();
    const hit = (a: DOMRect, b: DOMRect) => a.left < b.right - 0.5 && b.left < a.right - 0.5 && a.top < b.bottom - 0.5 && b.top < a.bottom - 0.5;
    const luminance = (color: string) => { const m = color.match(/[\d.]+/g)?.map(Number) ?? [0, 0, 0, 0]; return { alpha: m[3] ?? 1, value: (0.2126 * m[0] + 0.7152 * m[1] + 0.0722 * m[2]) / 255 }; };
    const canvas = document.createElement("canvas").getContext("2d")!;
    // Text cut by the box of the button, tab or heading that holds it (an ellipsis cuts too).
    for (const el of $$<HTMLElement>(".gg-side-tabs button, .gg-toolbar button, .ga-tabs button, .gcd-toolbar button, .gg-working-title button, .gg-side-row > span:first-child, .gg-side-list h3")) {
      if (el.clientWidth > 0 && el.scrollWidth > el.clientWidth + 1) problems.push(`cut ${name(el)} ${el.scrollWidth}/${el.clientWidth}`);
    }
    const tabs = $(".gg-side-tabs");
    if (tabs) for (const tab of $$(".gg-side-tabs button")) if (rect(tab).right > rect(tabs).right + 1 || rect(tab).left < rect(tabs).left - 1) problems.push(`tab outside its strip ${name(tab)}`);
    // Status indicators of a sidebar row stay inside the row text and never sit under the ⋯ button.
    for (const row of $$(".gg-side-row")) {
      const text = row.querySelector(":scope > span") as HTMLElement | null, more = row.querySelector(".gg-row-more");
      if (!text) continue;
      for (const pill of $$(".gg-current-pill, .gg-track, .gg-gone, .gg-local-only, .gg-synced, .gg-ref-status", text)) {
        if (rect(pill).right > rect(text).right + 1 || rect(pill).bottom > rect(text).bottom + 1) problems.push(`pill cut ${name(pill)}`);
        if (more && hit(rect(pill), rect(more))) problems.push(`pill under ⋯ ${name(pill)}`);
      }
    }
    // A select keeps room for its chevron (right 10 px, 10 px wide) and shows its whole value.
    for (const select of $$<HTMLSelectElement>(".gg-toolbar select, .gcd-toolbar select")) {
      if (select.clientWidth === 0) continue;
      const style = getComputedStyle(select), room = select.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
      canvas.font = `${style.fontStyle} ${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
      const text = select.selectedOptions[0]?.text ?? "";
      if (parseFloat(style.paddingRight) < 24) problems.push(`chevron over text ${name(select)} padding-right ${style.paddingRight}`);
      else if (canvas.measureText(text).width > room + 1) problems.push(`select value cut ${text} ${Math.round(canvas.measureText(text).width)}/${Math.round(room)}`);
    }
    // The square where the two scrollbars of the commit list meet has the colour of the surface around it.
    const lines = $(".git-graph-lines"), surface = $(".git-graph");
    // The pseudo-element is resolved by Chromium (WebView2); WebKit engines may report the host element instead, so only Chromium is asserted.
    if (lines && surface && !/Chrome\//.test(navigator.userAgent)) h.note("SKIP scrollbar corner colour: this engine does not resolve ::-webkit-scrollbar-corner in getComputedStyle");
    else if (lines && surface) {
      const corner = luminance(getComputedStyle(lines, "::-webkit-scrollbar-corner").backgroundColor), around = luminance(getComputedStyle(surface).backgroundColor);
      if (corner.alpha < 0.9 || Math.abs(corner.value - around.value) > 0.2) problems.push(`scrollbar corner colour alpha ${corner.alpha} luminance ${corner.value.toFixed(2)} vs ${around.value.toFixed(2)}`);
    }
    // The tools panel keeps its tabs and some content: every tab is whole and the panel is tall enough to show a body.
    const tools = $(".gg-main > .git-advanced"), main = $(".gg-main");
    if (tools && main) {
      // What is on screen, not what the layout holds: the point must hit the element itself, so a box clipped by an ancestor fails.
      const shown = (el: Element) => [[0.5, 0.15], [0.5, 0.85], [0.1, 0.5], [0.9, 0.5]].every(([x, y]) => { const r = rect(el), hitEl = document.elementFromPoint(r.left + r.width * x, r.top + r.height * y); return !!hitEl && (el === hitEl || el.contains(hitEl)); });
      for (const tab of $$(".ga-tabs button", tools)) if (!shown(tab)) problems.push(`tools tab cut ${name(tab)}`);
      const body = $(".ga-content", tools), visibleBody = body ? Math.min(rect(body).bottom, rect(main).bottom) - rect(body).top : 0;
      if (visibleBody < 60) problems.push(`tools content shows ${Math.round(visibleBody)} px`);
      const head = $(".gg-workspace > header");
      if (head) h.note(`METRICS header ${where}: padding ${getComputedStyle(head).paddingTop}/${getComputedStyle(head).paddingBottom} children ${Array.from(head.children).map((child) => `${child.tagName}.${(child as HTMLElement).className}=${Math.round(rect(child).height)}`).join(" ")} buttons ${$$("button", head).map((b) => Math.round(rect(b).height)).join(",")}`);
      h.note(`METRICS ${where}: viewport ${window.innerWidth}x${window.innerHeight} header ${Math.round(rect($(".gg-workspace > header") ?? tools).height)} toolbar ${Math.round(rect($(".gg-toolbar") ?? tools).height)} main ${Math.round(rect(main).height)} lines ${Math.round(rect($(".git-graph-lines") ?? tools).height)} tools ${Math.round(rect(tools).height)} body ${Math.round(visibleBody)} footer ${Math.round(rect($(".gg-workspace footer") ?? tools).height)} output ${Math.round(rect($(".gg-output") ?? tools).height)}`);
    }
    const graph = $(".git-graph") as HTMLElement | null;
    if (graph && graph.scrollWidth > graph.clientWidth + 1) problems.push(`horizontal overflow ${graph.scrollWidth}/${graph.clientWidth}`);
    log(problems.length === 0, `${where}: no clipped or overlapping label, no white scrollbar corner`, problems.slice(0, 12).join(" | "));
  };
  const narrowShot = async (name: string) => {
    await narrow(true);
    log(window.innerWidth <= 700, `${name}: the narrow screenshot is taken in a narrow viewport`, `${window.innerWidth} CSS px wide`);
    layoutAudit(`${name} (narrow)`);
    await shot(`${name}-narrow`);
    await narrow(false);
  };
  const repo = () => invoke<Repository>("git_repository", { path });
  const selectedKey = async (keyName: string, opts: KeyboardEventInit = {}) => { await sleep(80); $(".git-graph")?.focus(); await sleep(40); key(keyName, opts); await sleep(100); };
  try {
    await h.updateSettings({ keyboardProfile: "vim", theme: "dark", shortcuts: {} });
    await setWindowSize(1320, 900);
    path = h.activePath(0); A.openOverlay({ type: "gitGraph", path });
    log(!!await until(() => history("add beta")), "workspace loads actual repository history");
    await showPanels();
    $(".git-graph")?.focus(); key("Enter");
    log(!!await until(() => $(".gg-working-details")), "Enter opens highlighted Working Changes even when other branches precede HEAD");
    await press("Collapse", $(".gg-working-title")!); await sleep(100);
    const initial = await repo(), beforeFiles = await h.api.listDir(path, { showHidden: false, hideTemp: false });
    click(history("add beta")!);
    log(!!await until(() => $(".git-commit-details")?.textContent?.includes("beta.txt") && $(".gcd-patch")?.textContent?.includes("+beta")), "commit click opens metadata, changed files and parent diff");
    await shot("workspace-commit-details");
    layoutAudit("English commit details");
    // Slovenian: the workspace follows the language and no label is cut off (English comes back for the steps below).
    await h.updateSettings({ language: "sl" });
    const workspaceText = () => $(".git-graph")?.textContent ?? "";
    log(!!await until(() => workspaceText().includes("Podrobnosti commita") || workspaceText().includes("Spremembe v delu")), "Slovenian: the Git workspace is translated", workspaceText().slice(0, 80));
    await sleep(250);
    await shot("workspace-sl-commit-details");
    h.note("SCREENSHOT workspace-sl-commit-details.png: expected the Git workspace in Slovenian (Spremembe v delu, Commiti, Podrobnosti commita) without clipped labels.");
    const clipped = $$<HTMLElement>(".git-graph button, .git-graph h3, .git-graph label, .git-graph strong, .git-graph kbd")
      .filter((el) => el.clientWidth > 0 && el.scrollWidth > el.clientWidth + 1 && getComputedStyle(el).textOverflow !== "ellipsis");
    log(clipped.length === 0, "Slovenian: no label of the Git workspace is clipped", clipped.slice(0, 5).map((el) => `${el.className || el.tagName}:${(el.textContent ?? "").trim().slice(0, 30)}`).join(" | "));
    const graph = $(".git-graph") as HTMLElement | null;
    log(!!graph && graph.scrollWidth <= graph.clientWidth + 1, "Slovenian: the Git workspace has no horizontal overflow", graph ? `${graph.scrollWidth}/${graph.clientWidth}` : "missing");
    layoutAudit("Slovenian commit details");
    await narrowShot("workspace-sl-commit-details");
    // German and Spanish have the longest labels of the shipped languages.
    for (const [language, label] of [["de", "German"], ["es", "Spanish"]] as const) {
      await h.updateSettings({ language }); await sleep(250);
      layoutAudit(`${label} commit details`);
      await shot(`workspace-${language}-commit-details`);
      await narrowShot(`workspace-${language}-commit-details`);
    }
    await h.updateSettings({ language: "en" });
    await sleep(250);
    // Esc and q with nothing focused (after a click on the backdrop or a closed dialog): they leave the open commit details
    // first, and only a second press closes the window.
    const unfocus = () => (document.activeElement as HTMLElement | null)?.blur();
    const reopenWorkspace = async () => { A.openOverlay(null); await sleep(150); A.openOverlay({ type: "gitGraph", path }); await until(() => history("add beta")); await showPanels(); };
    const reopenDetails = async () => { if ($(".git-commit-details")) return; click(history("add beta")!); await until(() => $(".git-commit-details")?.textContent?.includes("beta.txt") && $(".gcd-patch")); await sleep(120); };
    await shot("workspace-unfocused-details");
    unfocus(); key("Escape", {}, document.body); await sleep(250);
    log(!$(".git-commit-details") && !!$(".git-graph"), "Escape with nothing focused leaves the commit details first and keeps the window");
    unfocus(); key("Escape", {}, document.body); await sleep(350);
    log(!$(".git-graph"), "a second Escape with nothing focused closes the Git workspace");
    await reopenWorkspace(); await reopenDetails(); unfocus();
    key("q", {}, document.body); await sleep(250);
    log(!$(".git-commit-details") && !!$(".git-graph"), "q with nothing focused (Vim keys) leaves the commit details first");
    unfocus(); key("q", {}, document.body); await sleep(350);
    log(!$(".git-graph"), "a second q with nothing focused closes the Git workspace");
    await reopenWorkspace(); await reopenDetails(); unfocus();
    const overlayBackdrop = $(".backdrop") as HTMLElement | null;
    key("Escape", {}, overlayBackdrop ?? document.body); await sleep(250);
    log(!$(".git-commit-details") && !!$(".git-graph"), "Escape on the backdrop leaves the commit details like Escape on the page");
    // Without Vim keys q still works like Escape (Overlays turns it into one in every profile): details first, window second.
    // Changing the keyboard profile reloads the workspace: open the commit details after it.
    await h.updateSettings({ keyboardProfile: "explorer" }); await sleep(400);
    if (!$(".git-graph")) await reopenWorkspace();
    await reopenDetails(); unfocus();
    key("q", {}, document.body); await sleep(250);
    log(!$(".git-commit-details") && !!$(".git-graph"), "q with nothing focused works like Escape when Vim keys are off: details first, window second", `details=${!!$(".git-commit-details")} window=${!!$(".git-graph")} vimKeys=${h.getState().settings?.vimKeys}`);
    await h.updateSettings({ keyboardProfile: "vim" }); await sleep(400);
    if (!$(".git-graph")) await reopenWorkspace();
    await reopenDetails();
    await narrowShot("workspace-commit-details");
    await press("Commit actions…", $(".git-commit-details")!);
    log(!!await until(() => $(".menu")?.textContent?.includes("Rewrite history")), "commit details opens the grouped commit action menu");
    key("Escape", {}, $(".menu")); await until(() => !$(".menu"));
    click($(".gg-row.selected .gg-row-more")!); await sleep(80);
    const rowMenu = $(".menu")?.textContent ?? "";
    log(rowMenu.includes("Rewrite history") && rowMenu.includes("Integrate") && !rowMenu.includes("Fetch remote") && !rowMenu.includes("Terminal here"), "commit row ⋯ opens the commit menu without repository-wide actions");
    $$(".mi", $(".menu")!).find((row) => $(".lbl", row)?.textContent?.trim() === "Rewrite history")?.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    await sleep(120);
    log(!!$$(".menu").at(-1)?.textContent?.includes("Rebase current branch onto this commit…") && !!$$(".menu").at(-1)?.textContent?.includes("Interactive rebase from this commit…"), "Rewrite history offers plain and interactive rebase on a commit");
    await shot("workspace-commit-row-menu");
    key("Escape", {}, $$(".menu").at(-1)!); await until(() => !$(".menu"));
    history("add beta")?.focus(); key("F10", { shiftKey: true }, history("add beta")); await sleep(120);
    log(!!$(".menu")?.textContent?.includes("Rewrite history"), "Shift+F10 opens the same menu for the focused commit");
    key("Escape", {}, $(".menu")); await until(() => !$(".menu"));
    history("add beta")?.focus(); key("j"); await sleep(100);
    log($(".git-graph-lines .selected")?.textContent?.includes("add alpha") ?? false, "keyboard navigation works after focusing a clicked commit row");
    // Moving between the sidebar, the commit list and the details with Vim keys.
    const footer = () => $(".gg-workspace footer")?.textContent ?? "";
    key("h", {}, $(".git-graph-lines")); await sleep(150);
    log(!!document.activeElement?.closest(".gg-sidebar"), "h moves from the commit list to the sidebar");
    const workspace = () => $(".gg-workspace");
    log(!!workspace()?.classList.contains("gg-focus-sidebar") && footer().includes("Branches"), "the focused sidebar is marked and named in the footer");
    await shot("workspace-focus-sidebar");
    log(footer().includes("r rebase") && footer().includes("l commits"), "footer lists the focused sidebar keys and how to reach the commit list");
    key("]", {}, $(".gg-sidebar")); await sleep(250);
    log($(".gg-side-tabs [aria-pressed='true']")?.textContent === "Tags", "] switches to the next sidebar tab");
    log(!!document.activeElement?.closest(".gg-sidebar"), "focus stays in the sidebar after switching tabs, even when the tab has no rows");
    key("[", {}, $(".gg-sidebar")); await sleep(250);
    key("l", {}, $(".gg-sidebar")); await sleep(150);
    log(!!document.activeElement?.closest(".git-graph-lines") && footer().includes("r reword") && !!document.activeElement?.textContent?.includes("add gamma"), "l moves from the sidebar to the selected commit (not the top row) and the footer shows commit keys");
    log(!!workspace()?.classList.contains("gg-focus-history") && footer().includes("Commits"), "the focused commit list is marked and named in the footer");
    key("l", {}, document.activeElement); await sleep(400);
    log(!!document.activeElement?.closest(".git-commit-details"), "l from a commit focuses its details");
    log(!!workspace()?.classList.contains("gg-focus-details") && footer().includes("Commit details"), "the focused details panel is marked and named in the footer");
    await shot("workspace-focus-details");
    key("h", {}, document.activeElement); await sleep(150);
    log(!!document.activeElement?.closest(".git-graph-lines"), "h in the details returns to the commit list");
    // Regression: h, p (asks to confirm a pull), Esc used to leave the keyboard dead until Esc was pressed twice.
    key("h", {}, $(".git-graph-lines")); await sleep(200);
    key("p", {}, document.activeElement); await sleep(400);
    log(!!$(".modal"), "p in the sidebar asks to confirm a pull");
    key("Escape", {}, $(".modal")); await until(() => !$(".modal")); await sleep(400);
    log(!!document.activeElement?.closest(".gg-sidebar"), "after cancelling the confirmation keyboard focus is back in the sidebar");
    key("l", {}, document.activeElement); await sleep(200);
    log(!!document.activeElement?.closest(".git-graph-lines"), "keys still move between panels after cancelling a confirmation");
    log(footer().includes("l or Enter details"), "the commit list footer says how to open the details");
    await shot("workspace-key-hints");
    // y on a commit opens lazygit's copy-attribute menu.
    key("y", {}, document.activeElement); await sleep(600);
    const copyLabels = $$(".mi .lbl", $(".menu")!).map((label) => label.textContent?.trim());
    log(["Commit hash", "Abbreviated hash", "Commit subject", "Full commit message", "Author", "Date", "Commit diff"].every((label) => copyLabels.includes(label)), "y opens a menu of commit attributes to copy");
    await shot("workspace-copy-menu");
    await press("Commit subject", $(".menu")!); await sleep(500);
    const subject = await invoke<{ kind: string; text?: string }>("clip_inspect");
    log(subject.kind === "text" && subject.text === "add gamma", "copying the subject puts it on the clipboard", JSON.stringify(subject));
    key("y", {}, document.activeElement); await sleep(600);
    await press("Commit diff", $(".menu")!); await sleep(700);
    const diff = await invoke<{ kind: string; text?: string }>("clip_inspect");
    log(diff.kind === "text" && !!diff.text?.includes("diff --git a/gamma.txt b/gamma.txt") && !!diff.text?.includes("+gamma"), "copying the diff puts the whole commit diff on the clipboard");
    // The copy menu opens beside the commit (not over it) and lists its Vim accelerators; pressing one copies.
    const selectedRow = $(".git-graph-lines .gg-row.selected")!.getBoundingClientRect();
    key("y", {}, document.activeElement); await sleep(600);
    const copyMenu = $(".menu")!.getBoundingClientRect();
    log(copyMenu.top <= selectedRow.top + 4 && copyMenu.left > selectedRow.left + 150, "the copy menu opens level with the commit row, right of its start", `${JSON.stringify(copyMenu)} vs ${JSON.stringify(selectedRow)}`);
    const accels = Object.fromEntries($$(".menu .mi", document).map((item) => [item.querySelector(".lbl")?.textContent, item.querySelector(".sc")?.textContent]));
    log(accels["Full commit message"] === "m" && accels["Commit patch"] === "p" && accels["Commit hash"] === "y", "the copy menu shows the key of every entry", JSON.stringify(accels));
    await shot("workspace-copy-menu-keys");
    key("m", {}, document.body); await sleep(600);
    const fullMessage = await invoke<{ kind: string; text?: string }>("clip_inspect");
    log(!$(".menu") && fullMessage.kind === "text" && fullMessage.text === "add gamma", "m in the copy menu copies the full commit message", JSON.stringify(fullMessage));
    // WN-55: a menu opened with a key belongs to the selected commit. A mouse click selects a commit without moving keyboard
    // focus in WebKit, so the focus can still be on a sidebar row far to the left: the menu must follow the selection, not the focus.
    key("h", {}, document.activeElement); await sleep(250);
    log(!!document.activeElement?.closest(".gg-sidebar"), "focus is in the sidebar before the commit is clicked");
    click(history("add gamma")!); await sleep(400);
    const clicked = $(".git-graph-lines .gg-row.selected")!.getBoundingClientRect();
    // Right of the row's text and level with the row (a tall menu that does not fit from the row's top is moved up, so it only has
    // to span the row), or just below it. WN-67: the old check demanded `menu.top >= row.top - 6`, which no 441 px menu can meet in a
    // window of about 675 px.
    const beside = (menu: DOMRect) => (menu.left >= clicked.left + 100 && menu.top <= clicked.bottom && menu.bottom >= clicked.top) || (menu.top >= clicked.bottom - 2 && menu.top <= clicked.bottom + 12);
    log(!beside(new DOMRect(clicked.left + 20, clicked.top, 300, 200)) && !beside(new DOMRect(clicked.left + 300, clicked.bottom + 200, 300, 200)), "the placement check rejects a menu over the row start and a menu far below the row");
    const inside = (menu: DOMRect) => menu.left >= 0 && menu.top >= 0 && menu.right <= window.innerWidth + 1 && menu.bottom <= window.innerHeight + 1;
    key("y", {}, document.activeElement); await sleep(600);
    const copyAfterClick = $(".menu")!.getBoundingClientRect();
    log(beside(copyAfterClick) && inside(copyAfterClick), "y after a mouse click opens the copy menu at the clicked commit, not at the focused sidebar row", `${JSON.stringify(copyAfterClick)} vs ${JSON.stringify(clicked)}`);
    await shot("workspace-copy-menu-after-click");
    key("Escape", {}, document.body); await sleep(300);
    key("F10", { shiftKey: true }, document.activeElement); await sleep(600);
    const contextAfterClick = $(".menu")!.getBoundingClientRect();
    log(beside(contextAfterClick) && inside(contextAfterClick), "Shift+F10 after a mouse click opens the commit menu at the clicked commit", `${JSON.stringify(contextAfterClick)} vs ${JSON.stringify(clicked)}`);
    await shot("workspace-key-menu-after-click");
    key("Escape", {}, document.body); await sleep(300);
    key("l", {}, document.activeElement); await sleep(250);
    key("y", {}, document.activeElement); await sleep(600);
    key("p", {}, document.body); await sleep(900);
    const patch = await invoke<{ kind: string; text?: string }>("clip_inspect");
    log(patch.kind === "text" && !!patch.text?.includes("Subject: [PATCH] add gamma") && !!patch.text?.includes("+gamma"), "p in the copy menu copies the commit as an applicable patch");
    // a asks before applying the clipboard patch; this one is already in the history, so applying it is refused and nothing changes.
    key("a", {}, document.activeElement); await until(() => $(".modal"), 4000);
    log(!!$(".modal")?.textContent?.includes("Apply patch from clipboard?") && !!$(".modal")?.textContent?.includes("Subject: [PATCH] add gamma"), "a shows the patch from the clipboard and asks before applying it");
    await shot("workspace-apply-patch-confirm");
    await press("Apply", $(".modal")!); await until(() => $(".gg-error"), 6000);
    log(!!$(".gg-error") && !$(".modal"), "a patch that is already applied is refused with an error", $(".gg-error")?.textContent ?? "");
    await sleep(300);
    // Tools with lazygit keys: b bisect, w worktrees, x generic; Esc and h return to the commit list.
    const toolTab = () => $(".ga-tabs [aria-selected='true']")?.textContent ?? "";
    key("b", {}, document.activeElement); await sleep(600);
    log(toolTab().includes("Bisect"), "b opens the tools on bisect options");
    key("Escape", {}, $(".git-advanced")); await sleep(300);
    log(!$(".git-advanced") && !!$(".gg-workspace") && !!document.activeElement?.closest(".git-graph-lines"), "Esc closes the tools and returns to the commit list");
    key("w", {}, document.activeElement); await sleep(600);
    log(toolTab().includes("Worktrees"), "w opens worktree options");
    key("h", {}, $(".git-advanced")); await sleep(300);
    log(!!$(".git-advanced") && !!document.activeElement?.closest(".git-graph-lines"), "h returns from the tools to the commit list");
    key("x", {}, document.activeElement); await sleep(600);
    log(!!$(".git-advanced") && !!document.activeElement?.closest(".git-advanced"), "x opens the repository tools and focuses them");
    log(!!workspace()?.classList.contains("gg-focus-tools") && footer().includes("Repository tools") && footer().includes("b bisect"), "the focused tools panel is marked and named in the footer");
    await shot("workspace-tools-keys");
    key("Escape", {}, $(".git-advanced")); await sleep(300);
    // One registry: a remapped key works, and the button, the hint bar, the ? sheet and the menu show it.
    await h.updateSettings({ shortcuts: { "git.history.tools.bisect": "Z", "git.common.tools": "Ctrl+G" } }); await sleep(250);
    const toolsCap = () => $(".gg-advanced-button kbd")?.textContent ?? "";
    log(/^(Ctrl|Cmd)\+g$/.test(toolsCap()), "a remapped key is shown on the Tools button", toolsCap());
    log(/(Ctrl|Cmd)\+g tools/.test(footer()) && !footer().includes("x tools"), "a remapped key is shown in the hint bar", footer());
    key("b", {}, document.activeElement); await sleep(400);
    log(!$(".git-advanced"), "the old key of a remapped command does nothing");
    key("Z", { shiftKey: true }, document.activeElement); await sleep(600);
    log(toolTab().includes("Bisect"), "the new key runs the command");
    key("Escape", {}, $(".git-advanced")); await sleep(300);
    key("?", { shiftKey: true }, document.activeElement); await sleep(300);
    const sheetRow = (label: string) => $$(".gg-key-help button").find((row) => row.textContent?.includes(label));
    log(sheetRow("Bisect options")?.querySelector("kbd")?.textContent === "Z", "the ? sheet shows the remapped key", sheetRow("Bisect options")?.textContent ?? "");
    await shot("workspace-remapped-keys");
    key("Escape", {}, document.activeElement); await sleep(250);
    key("F10", { shiftKey: true }, document.activeElement); await sleep(500);
    const menuKeys = $$(".menu .mi").map((row) => `${$(".lbl", row)?.textContent}=${$(".sc", row)?.textContent ?? ""}`);
    log(menuKeys.some((row) => /^Checkout selected commit…=c$/.test(row)) && menuKeys.some((row) => /^Mark comparison base \/ compare=(Ctrl|Cmd)\+e$/.test(row)), "the context menu shows the Git keys, not chords of the file list", menuKeys.join(" | "));
    await shot("workspace-menu-keys");
    key("Escape", {}, document.body); await sleep(300);
    const hint = () => $$<HTMLButtonElement>(".gg-hint").find((row) => row.textContent?.includes("tools"));
    click(hint()!); await sleep(600);
    log(!!$(".git-advanced"), "a click on a hint runs the command");
    key("Escape", {}, $(".git-advanced")); await sleep(300);
    await h.updateSettings({ keyboardProfile: "explorer" }); await sleep(200);
    key("Z", { shiftKey: true }, document.activeElement); await sleep(400);
    log(!$(".git-advanced"), "without Vim keys a remapped letter stays quiet");
    key("g", { ctrlKey: true }, document.activeElement); await sleep(600);
    log(!!$(".git-advanced"), "without Vim keys a chosen chord runs the command");
    key("Escape", {}, $(".git-advanced")); await sleep(300);
    await h.updateSettings({ keyboardProfile: "vim", shortcuts: {} }); await sleep(200);
    // The palette: every command by name, its keys stay in it, and a target is asked for in a second step.
    const paletteRows = () => $$<HTMLElement>(".gp-row"), paletteInput = () => $<HTMLInputElement>(".gp-input input");
    key("?", { shiftKey: true }, document.activeElement); await sleep(150); key("?", { shiftKey: true }, document.activeElement); await sleep(150);
    key(":", { shiftKey: true }, document.activeElement);
    log(!!await until(paletteInput) && document.activeElement === paletteInput() && paletteRows().length > 60, "`:` opens the Git command palette", `${paletteRows().length} commands`);
    const foot = $(".gp-foot")!;
    log(foot.scrollHeight <= foot.clientHeight + 1 && foot.getBoundingClientRect().bottom <= $(".gp")!.getBoundingClientRect().bottom + 1, "the foot of a long palette is whole", `${foot.scrollHeight}/${foot.clientHeight}`);
    await shot("workspace-palette");
    for (const letter of ["d", "g", "q", "x", "?", "j", " "]) key(letter, {}, paletteInput());
    await sleep(250);
    log(!!paletteInput() && !$(".modal") && !$(".git-advanced") && !$(".gg-key-help"), "typing in the palette triggers no Git command");
    typeInto(paletteInput()!, "bisect opt"); await sleep(120);
    log(!!paletteRows()[0]?.textContent?.includes("Bisect options") && paletteRows()[0]?.querySelector("kbd")?.textContent === "b", "the palette finds a command and shows its key", paletteRows()[0]?.textContent ?? "");
    key("Enter", {}, paletteInput()); await sleep(600);
    log(!$(".gp") && toolTab().includes("Bisect"), "Enter runs the command of the palette");
    key("Escape", {}, $(".git-advanced")); await sleep(300);
    await h.updateSettings({ keyboardProfile: "explorer" }); await sleep(150);
    key("P", { ctrlKey: true, shiftKey: true }, document.activeElement); await sleep(250);
    log(!!paletteInput(), "Ctrl+Shift+P opens the palette without Vim keys");
    typeInto(paletteInput()!, "checkout"); await sleep(120); key("Enter", {}, paletteInput()); await sleep(200);
    log(!!$(".gp-step") && paletteRows().some((row) => row.textContent?.includes("conflict-left")) && paletteRows().some((row) => row.textContent?.includes("add alpha")), "checkout continues with branches and commits", $(".gp-step")?.textContent ?? "");
    typeInto(paletteInput()!, "cnfl left"); await sleep(120); await shot("workspace-palette-target");
    key("Enter", {}, paletteInput()); await until(() => $(".modal"), 6000);
    log(!!$(".modal")?.textContent && !$(".gp"), "choosing a target runs the command on it", $(".modal")?.textContent?.slice(0, 60) ?? "");
    key("Escape", {}, $(".modal")); await until(() => !$(".modal")); await settle();
    await h.updateSettings({ keyboardProfile: "vim" }); await sleep(150);
    $<HTMLElement>(".git-graph")?.focus(); key(":", { shiftKey: true }, document.activeElement); await until(paletteInput);
    typeInto(paletteInput()!, "zzzz"); await sleep(120); await shot("workspace-palette-empty"); await narrowShot("workspace-palette-empty");
    key("Escape", {}, paletteInput()); await sleep(250);
    log(!$(".gp") && !!document.activeElement?.closest(".git-graph"), "Esc closes the palette and gives the focus back");
    // Several commits: Space and v choose, Ctrl+click and Shift+click too, and the choice survives a refresh.
    const chosen = () => $$<HTMLElement>(".git-graph-lines .gg-row.picked").map((row) => row.querySelector(".gg-message > span:last-child")?.textContent?.trim() ?? ""), pill = () => $(".gg-picked")?.textContent ?? "";
    const rowOf = (text: string) => history(text) as HTMLElement;
    const headerHeight = $(".git-graph > header")!.getBoundingClientRect().height, plainBackground = getComputedStyle(rowOf("initial file")).backgroundColor;
    rowOf("add gamma").focus(); await sleep(150);
    key(" ", {}, document.activeElement); await sleep(150); key("j", {}, document.activeElement); await sleep(150); key(" ", {}, document.activeElement); await sleep(200);
    log(chosen().join() === "add gamma,add beta" && pill().includes("2 chosen"), "Space chooses several commits and the header counts them", `${chosen().join()} ${pill()}`);
    log(Math.abs($(".git-graph > header")!.getBoundingClientRect().height - headerHeight) < 0.6, "the header keeps its height when commits are chosen", `${headerHeight} -> ${$(".git-graph > header")!.getBoundingClientRect().height}`);
    log(getComputedStyle(rowOf("add gamma")).backgroundColor !== plainBackground, "a chosen row is tinted", `${getComputedStyle(rowOf("add gamma")).backgroundColor} vs ${plainBackground}`);
    key("R", { shiftKey: true }, document.activeElement); await settle(); await sleep(400);
    log(chosen().join() === "add gamma,add beta", "the choice survives a refresh", chosen().join());
    key("y", {}, document.activeElement); await sleep(500);
    log($$(".menu .mi").some((row) => row.textContent?.includes("Copy 2 hashes")), "y offers to copy all chosen commits", $(".menu")?.textContent ?? "");
    key("y", {}, document.body); await sleep(400);
    const hashes = (await invoke<{ text?: string }>("clip_inspect")).text ?? "";
    log(hashes.split("\n").length === 2 && hashes.split("\n")[1] === (await repo()).head, "the hashes are copied oldest first", hashes);
    click(rowOf("add alpha"), { ctrlKey: true }); await sleep(200);
    log(chosen().length === 3, "Ctrl+click adds a commit to the choice", chosen().join());
    rowOf("add beta").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 500, clientY: 300 })); await sleep(400);
    const batch = $$(".menu .mi").map((row) => row.textContent ?? "");
    log(["Cherry-pick 3 commits onto the current branch…", "Cherry-pick 3 commits to another branch…", "Copy 3 hashes", "Copy 3 commits as a patch", "Save 3 commits as a patch file…"].every((label) => batch.some((row) => row.includes(label))), "the menu of chosen commits offers the actions for all of them", batch.join(" | "));
    await shot("workspace-chosen-commits");
    key("Escape", {}, document.body); await sleep(250);
    key("Escape", {}, $(".git-graph-lines .gg-row.selected") ?? $(".git-graph")); await sleep(250);
    log(!chosen().length && !!$(".git-graph"), "Esc clears the choice before anything closes");
    rowOf("add gamma").focus(); await sleep(150); key("v", {}, document.activeElement); await sleep(150); key("j", {}, document.activeElement); await sleep(120); key("j", {}, document.activeElement); await sleep(200);
    log(chosen().length === 3 && pill().includes("range"), "v and j choose a range", `${chosen().join()} ${pill()}`);
    key("v", {}, document.activeElement); await sleep(120); key("j", {}, document.activeElement); await sleep(150);
    log(chosen().length === 3 && !pill().includes("range"), "a second v ends the range and keeps the choice", pill());
    await narrowShot("workspace-chosen-commits");
    click(rowOf("add gamma")); await sleep(200);
    log(!chosen().length, "a plain click chooses one commit again");
    click(rowOf("add alpha"), { shiftKey: true }); await sleep(200);
    log(chosen().length === 3, "Shift+click chooses a range", chosen().join());
    click(rowOf("add gamma")); await sleep(200);
    click(rowOf("add alpha"), { altKey: true });
    log(!!await until(() => $(".git-commit-details[aria-label='Commit comparison']"), 6000), "Alt+click compares with the commit that was shown");
    key("Escape", {}, $(".git-commit-details")); await sleep(250);
    const tip = rowOf("add gamma").title;
    log(tip.includes((await repo()).head) && tip.includes("Test ·") && tip.includes("main"), "the tooltip shows the full hash, author, date and references", tip.replace(/\n/g, " | "));
    rowOf("add gamma").dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true })); await sleep(700);
    log(!!document.activeElement?.closest(".git-commit-details"), "a double-click shows the changes of the commit");
    key("h", {}, document.activeElement); await sleep(250);
    await h.updateSettings({ gitDoubleClick: "checkout" }); await sleep(200);
    rowOf("add gamma").dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true })); await until(() => $(".modal"), 5000);
    log(!!$(".modal"), "a double-click asks to check out when that is chosen in Settings"); key("Escape", {}, $(".modal")); await until(() => !$(".modal")); await settle();
    await h.updateSettings({ gitDoubleClick: "details" }); await sleep(150);
    key("V", { shiftKey: true }, document.activeElement); await sleep(300);
    log(paletteRows().map((row) => row.textContent ?? "").some((row) => row.includes("conflict-left")) && !paletteRows().some((row) => /^main/.test(row.textContent ?? "")), "V asks for the branch to cherry-pick to, without the current one", paletteRows().map((row) => row.textContent).join(" | "));
    await shot("workspace-cherry-pick-to");
    key("Escape", {}, paletteInput()); await sleep(250);
    // Searching the whole history: 230 commits, of which 200 are loaded.
    A.openOverlay(null); await sleep(200); A.openOverlay({ type: "gitGraph", path: `${parentOf(path)}${sep()}many-commits` });
    await until(() => history("commit no. 230"), 8000); await showPanels(); await settle();
    const field = () => $(".gg-search") as HTMLInputElement, listed = () => $$(".git-graph-lines [data-commit]"), foundText = () => $(".gg-found")?.textContent ?? "";
    rowOf("commit no. 220").focus(); click(rowOf("commit no. 220")); await sleep(400);
    const shownBefore = $(".git-graph-lines .gg-row.selected")?.getAttribute("data-commit");
    key("/", {}, $(".git-graph-lines .gg-row.selected")); await sleep(150);
    log(document.activeElement === field(), "/ goes to the search field");
    typeInto(field(), "no. 22"); await sleep(200);
    log(listed().length === 10 && !$(".gg-found") && $$(".gg-row mark").length === 10, "loaded commits are filtered at once and the match is marked", `${listed().length} rows`);
    typeInto(field(), "no. 007");
    log(!!await until(() => listed().length === 1 && history("commit no. 007"), 8000) && foundText().includes("1 match in the whole history"), "text that no loaded commit has is searched in the whole history", foundText());
    await shot("workspace-search-history");
    typeInto(field(), "a:test g:no. 00");
    log(!!await until(() => listed().length === 9, 8000), "prefixes search by author and message together", `${listed().length} ${foundText()}`);
    key("Enter", {}, field()); await sleep(400);
    const selectedText = () => $(".git-graph-lines .gg-row.selected")?.textContent ?? "";
    log(selectedText().includes("commit no. 009") && !!document.activeElement?.closest(".git-graph-lines"), "Enter goes to the first match", selectedText());
    key("n", {}, document.activeElement); await sleep(300);
    log(selectedText().includes("commit no. 008") && !$(".modal"), "n goes to the next match instead of creating a branch", selectedText());
    key("N", { shiftKey: true }, document.activeElement); await sleep(300); key("N", { shiftKey: true }, document.activeElement); await sleep(300);
    log(selectedText().includes("commit no. 001") && !$(".modal"), "N goes back, around the end", selectedText());
    await narrowShot("workspace-search-matches");
    const seventh = (await invoke<{ graph: { history: string } }>("git_graph_view", { path: `${parentOf(path)}${sep()}many-commits`, comparison: false, reflog: false, limit: 20000, reference: null })).graph.history.split("\n").find((line) => line.includes("commit no. 007"))!.slice(0, 10);
    field().focus(); typeInto(field(), seventh);
    log(!!await until(() => listed().length === 1 && history("commit no. 007"), 8000), "an abbreviated hash finds its commit", `${listed().length}`);
    typeInto(field(), "a:nobody at all");
    log(!!await until(() => $(".git-graph-lines .gg-empty")?.textContent?.includes("No commit in the whole history"), 8000), "a search without matches says so"); await shot("workspace-search-none");
    // Typing on cancels: only the last search may fill the list.
    for (const text of ["g:c", "g:co", "g:commit no. 01", "g:commit no. 02"]) { typeInto(field(), text); await sleep(40); }
    log(!!await until(() => listed().length === 10 && listed().every((r) => r.textContent?.includes("no. 02")), 8000), "only the last of quickly typed searches is shown", `${listed().length}`);
    key("Escape", {}, field()); await sleep(400);
    log(field().value === "" && listed().length === 200 && $(".git-graph-lines .gg-row.selected")?.getAttribute("data-commit") === shownBefore && !!$(".git-commit-details"), "Esc brings back the list, the selection and the details", `${listed().length}`);
    A.openOverlay(null); await sleep(200); A.openOverlay({ type: "gitGraph", path }); await until(() => history("add gamma"), 8000); await showPanels(); await settle(); click(rowOf("add gamma")); await sleep(400);
    // Esc goes back one level: the text of the search, then the field, then the window stays with the commit list.
    const searchField = $(".gg-search") as HTMLInputElement;
    searchField.focus(); typeInto(searchField, "beta"); await sleep(150);
    key("Escape", {}, searchField); await sleep(200);
    log(searchField.value === "" && !!$(".git-graph"), "Esc in the search field clears it first");
    key("Escape", {}, searchField); await sleep(250);
    log(!!$(".git-graph") && document.activeElement !== searchField, "a second Esc leaves the search field for the commits");
    if (!$(".git-commit-details")) { click(history("add gamma")!); await until(() => $(".git-commit-details")); }
    $(".git-graph-lines .gg-row.selected") && ($(".git-graph-lines .gg-row.selected") as HTMLElement).focus(); await sleep(150);
    // Working Changes: a typed commit message survives leaving the panel, and Esc returns to the commit list.
    const workingRow = $(".gg-working") as HTMLButtonElement;
    log(!workingRow.disabled && !$(".git-changes"), `Working Changes row is enabled and closed before opening it (disabled=${workingRow.disabled}, open=${!!$(".git-changes")})`);
    click(workingRow); await until(() => $("textarea[aria-label='Commit message']"), 6000); await sleep(100);
    typeInto($("textarea[aria-label='Commit message']") as HTMLTextAreaElement, "draft kept"); await sleep(100);
    key("h", {}, $(".git-changes .gc-file") ?? $(".git-changes")); await sleep(150);
    key("h", {}, $(".git-graph-lines")); await sleep(250);
    log(!$(".git-changes") && !!document.activeElement?.closest(".gg-sidebar"), "h from the commit list leaves Working Changes for the sidebar");
    key("l", {}, $(".gg-sidebar")); await sleep(150);
    click($(".gg-working")!); await sleep(500);
    log(($("textarea[aria-label='Commit message']") as HTMLTextAreaElement | null)?.value === "draft kept", "an unsent commit message survives leaving and reopening Working Changes");
    typeInto($("textarea[aria-label='Commit message']") as HTMLTextAreaElement, ""); await sleep(80);
    key("Escape", {}, $(".git-changes")); await sleep(250);
    log(!$(".git-changes") && !!$(".gg-workspace") && !!document.activeElement?.closest(".git-graph-lines"), "Esc in Working Changes returns to the commit list without closing the workspace");
    click(history("add beta")!); await sleep(100);
    await press("File", $(".git-commit-details")!);
    log(!!await until(() => $(".gcd-patch")?.textContent?.includes("beta")), "historical file content loads without checkout");
    await press("History", $(".git-commit-details")!);
    log(!!await until(() => $(".gcd-history")?.textContent?.includes("add beta")), "rename-aware file history loads from selected commit");
    await press("Blame", $(".git-commit-details")!);
    log(!!await until(() => $(".gcd-blame-line")?.textContent?.includes("Test")), "blame shows actual author and line");
    await shot("workspace-blame");
    const afterFiles = await h.api.listDir(path, { showHidden: false, hideTemp: false });
    log((await repo()).head === initial.head && beforeFiles.every((f) => afterFiles.some((n) => n.path === f.path && n.modified === f.modified && n.size === f.size)), "inspection preserves HEAD and filesystem timestamps");
    click(history("add alpha")!); await selectedKey("e", { ctrlKey: true }); click(history("add gamma")!); await selectedKey("e", { ctrlKey: true });
    log(!!await until(() => $(".git-commit-details[aria-label='Commit comparison']")?.textContent?.includes("beta.txt") && $(".git-commit-details")?.textContent?.includes("gamma.txt")), "two selected commits compare as a range");
    await shot("workspace-commit-range");
    const filter = $(".gg-search") as HTMLInputElement;
    typeInto(filter, "no-matching-commit");
    log(!!await until(() => $(".git-graph-lines")?.textContent?.includes("No commit in the whole history"), 8000), "empty history search has explicit feedback");
    await shot("workspace-search-empty"); typeInto(filter, "");
    click($(".gg-working")!);
    log(!!await until(() => $(".gc-patch")?.textContent?.includes("second changed")), "working changes show selectable actual diff");
    const hunks = $$(".gc-hunk"); log(hunks.length === 2, "separate changes appear as two selectable hunks");
    click(hunks[0]); await sleep(80); await press("Stage selection", $(".git-changes")!);
    log(!!await until(() => $$(".gc-file").some((r) => r.textContent?.includes("Staged") && r.textContent?.includes("file.txt"))), "selected hunk stages independently");
    const staged = await invoke<string>("git_file_patch", { path, file: "file.txt", staged: true });
    log(staged.includes("second changed") && !staged.includes("twenty-five changed"), "partial staging leaves excluded hunk unstaged");
    await shot("workspace-partial-staging");
    await narrowShot("workspace-partial-staging");
    // WN-35 keys in the diff: ] jumps to the next hunk, v starts a line range, j extends it, Space stages what is chosen.
    click($$(".gc-file").find((r) => r.textContent?.includes("Unstaged") && r.textContent?.includes("file.txt"))!); await sleep(200);
    ($(".gc-patch") as HTMLElement).focus(); await sleep(100);
    key("]", {}, $(".gc-patch")); await sleep(120);
    log(!!$(".gc-diff-line.current")?.textContent?.includes("@@"), "] moves the cursor to the next hunk header");
    key("v", {}, $(".gc-patch")); await sleep(80);
    const startChosen = $$(".gc-diff-line.chosen").length;
    for (let i = 0; i < 12 && $$(".gc-diff-line.chosen").length < 2; i++) { key("j", {}, $(".gc-patch")); await sleep(60); }
    log($$(".gc-diff-line.chosen").length >= 2 && startChosen === 0, "v starts a line range at the hunk header and moving down extends it over the changed lines", String($$(".gc-diff-line.chosen").length));
    await shot("workspace-hunk-keys");
    key(" ", {}, $(".gc-patch")); await sleep(300);
    const afterKey = await invoke<string>("git_file_patch", { path, file: "file.txt", staged: true });
    log(afterKey.includes("twenty-five changed"), "Space in the diff stages the chosen lines by key", afterKey);
    // Dragging a file row to the other list stages it.
    const dragged = $$(".gc-file").find((r) => r.textContent?.includes("Unstaged"));
    if (dragged) {
      const transfer = new DataTransfer(); const label = dragged.querySelector("span:nth-child(2)")?.textContent ?? "";
      dragged.dispatchEvent(new DragEvent("dragstart", { bubbles: true, dataTransfer: transfer }));
      const stagedGroup = $$(".gc-group").find((g) => g.textContent?.includes("Staged"));
      log(!!stagedGroup, "the Staged list is a drop target");
      if (stagedGroup) { stagedGroup.dispatchEvent(new DragEvent("drop", { bubbles: true, cancelable: true, dataTransfer: transfer })); await sleep(500); }
      log(!$$(".gc-file").some((r) => r.textContent?.includes("Unstaged") && r.textContent?.includes(label)), "dropping an unstaged file on the Staged list stages it", label);
    }
    await press("Stage all", $(".git-changes")!); await until(() => $$(".gc-file .staged").length === 2);
    const message = $(".gc-commit textarea") as HTMLTextAreaElement; typeInto(message, "working changes"); await sleep(80);
    await press("Commit staged", $(".git-changes")!);
    log(!!await until(() => $(".gc-files")?.textContent?.includes("Working tree clean"), 6000), "staged commit completes and refreshes integrated graph");
    await settle(); await press("Collapse", $(".gg-working-title")!);
    click(history("working changes")!); await selectedKey("r");
    await until(() => $(".modal textarea"));
    log(!!$(".modal textarea") && !$(".git-rebase"), "direct reword opens a message field without rebase planner");
    const rewordField = $(".modal textarea") as HTMLTextAreaElement;
    typeInto(rewordField, "working changes renamed\n\nMessage body preserved."); key("Enter", {}, rewordField);
    log(!!$(".modal textarea"), "Enter in reword field preserves normal multiline editing");
    await shot("workspace-direct-reword"); await dialog("Save message");
    log(!!await until(() => history("working changes renamed"), 8000), "direct HEAD reword updates graph");
    const renamedHead = (await repo()).head;
    const reworded = await invoke<{message:string}>("git_commit_details", { path, commit: renamedHead, base: null });
    log(reworded.message.includes("\n\nMessage body preserved."), "direct reword preserves multiline commit body");
    click(history("add beta")!); await selectedKey("i");
    log(!!await until(() => $$(".grw-entry").length === 3), "interactive planner includes selected commit through HEAD");
    log(!!await until(() => $(".grw-summary .grw-chip.clean"), 8000), "actual rebase planner finishes conflict prediction for multiple commits");
    log((await repo()).head === renamedHead && !(await repo()).operation, "conflict preview leaves HEAD and operation state unchanged");
    await shot("workspace-rebase-prediction-clean");
    const oldest = () => $$(".grw-entry").at(-1);
    log($$(".grw-entry")[0]?.textContent?.includes("add beta") === false && !!oldest()?.textContent?.includes("add beta"), "interactive planner lists newest commit first, like the graph");
    click($<HTMLButtonElement>("button[aria-label='Move add beta up']", oldest())!);
    log(!!await until(() => oldest()?.textContent?.includes("add gamma")), "row arrow moves commit up (newer) in the list");
    $(".git-rebase")?.focus(); key("J"); await sleep(80);
    log(!!oldest()?.textContent?.includes("add beta"), "Vim J moves selected commit down (older)");
    const rows = $$(".grw-entry");
    const move = await h.drag(rows[rows.length - 1], rows[rows.length - 2]); move.drop(); await sleep(100);
    log(!!oldest()?.textContent?.includes("add gamma"), "dragging reorders commits");
    click($("button[aria-label='Move add beta down']", $$(".grw-entry").find((r) => r.textContent?.includes("add beta")))!); await sleep(100);
    const gamma = $$(".grw-entry").find((r) => r.textContent?.includes("add gamma"))!;
    const action = $("select", gamma) as HTMLSelectElement; action.value = "fixup"; action.dispatchEvent(new Event("change", { bubbles: true }));
    const beta = $$(".grw-entry").find((r) => r.textContent?.includes("add beta"))!;
    const betaAction = $("select", beta) as HTMLSelectElement; betaAction.value = "reword"; betaAction.dispatchEvent(new Event("change", { bubbles: true }));
    await sleep(80); typeInto($("textarea", beta) as HTMLTextAreaElement, "combine beta and gamma"); await sleep(80);
    log(!!await until(() => $(".grw-summary .grw-chip.clean"), 8000), "conflict prediction finishes after reordering and changing commit actions");
    await shot("workspace-interactive-rebase");
    await narrowShot("workspace-interactive-rebase");
    await press("Start rebase…", $(".git-rebase")!); await dialog("Start interactive rebase");
    const rewritten = await until(() => history("combine beta and gamma"), 12000);
    log(!!rewritten, "interactive reword and fixup execute through actual native helper", rewritten ? "" : $(".grw-error,.gg-error")?.textContent ?? "");
    const combined = await repo(); log(combined.head !== renamedHead && !combined.operation, "rebase ends on branch with changed commit IDs and no pending operation");
    const headFiles = await invoke<{files:{path:string}[]}>("git_commit_details", { path, commit: combined.head, base: initial.head });
    log(headFiles.files.some((f) => f.path === "new.txt"), "rebase retains committed working changes");
    if (!rewritten) throw new Error("Cannot continue workspace checks without the rewritten commit");
    await press("✕", $(".grw-heading")!); await settle();
    click(history("combine beta and gamma")!); await selectedKey("r"); await dialog("Save message", "direct historical reword");
    log(!!await until(() => history("direct historical reword"), 12000), "non-HEAD reword executes directly without opening planner");
    await shot("workspace-after-rebase");
    click(history("direct historical reword")!); await selectedKey("n"); await dialog("Create branch", "review-branch");
    log(!!await until(() => $$(".gg-side-row").some((r) => r.textContent?.includes("review-branch"))), "create branch from selected historical commit");
    await selectedKey("T"); await dialog("Next", "review-tag"); await dialog("Create tag", "Annotated test tag");
    await press("Tags", $(".gg-side-tabs")!); await sleep(100);
    log(!!await until(() => $$(".gg-side-row").some((r) => r.textContent?.includes("review-tag"))), "annotated tag appears in its own Tags panel");
    await press("Refs", $(".gg-side-tabs")!); await sleep(100);
    log(!$$(".gg-side-row").some((r) => r.textContent?.includes("review-tag")), "tags are not listed among branches");
    await press("Remotes", $(".gg-side-tabs")!); await sleep(100);
    click($$(".gg-side-row").find((r) => r.textContent?.includes("origin"))!);
    await sleep(80); key("f", {}, $(".gg-sidebar")); await settle(); log(!!$(".gg-output") && !$(".gg-error") && !!(await repo()).refs.some((ref) => ref.fullName === "refs/remotes/origin/main"), "fetch completes through cancellable remote job");
    const remoteBeforePush = (await repo()).refs.find((ref) => ref.fullName === "refs/remotes/origin/main")?.id;
    const rewrittenHead = (await repo()).head;
    await selectedKey("P"); await dialog("Push");
    log(!!await until(() => $(".modal")?.textContent?.includes("Your branch has diverged"), 12000), "rejected normal push offers force-with-lease confirmation");
    log(!!$(".gg-output")?.textContent?.match(/rejected|non-fast-forward|failed to push/i), "rejected push keeps Git output visible behind confirmation");
    await shot("workspace-push-lease-confirm");
    key("Escape", {}, $(".modal")); await until(() => !$(".modal")); await settle();
    await selectedKey("f"); await settle();
    const cancelledPush = await repo();
    log(!!remoteBeforePush && remoteBeforePush !== rewrittenHead && cancelledPush.head === rewrittenHead && cancelledPush.refs.find((ref) => ref.fullName === "refs/remotes/origin/main")?.id === remoteBeforePush, "cancelling force-with-lease preserves local and freshly fetched remote history");
    click($(".gg-actions")!); await sleep(80);
    const repositoryLabels = $$(".mi .lbl", $(".menu")!).map((label) => label.textContent?.trim());
    log(["Fetch remote", "Pull (fast-forward only)", "Push branch", "Force push with explicit lease…", "Stash working changes…", "Terminal here"].every((label) => repositoryLabels.includes(label)) && !repositoryLabels.includes("Rewrite history"), "Actions menu holds repository-wide actions only");
    await shot("workspace-actions-menu");
    key("Escape", {}, $(".menu")); await until(() => !$(".menu"));
    await selectedKey("P"); await dialog("Push");
    await until(() => $(".modal")?.textContent?.includes("Your branch has diverged"), 12000);
    await dialog("Force push with lease"); await settle();
    await selectedKey("f"); await settle();
    const forcedPush = await repo();
    log(!$(".gg-error") && forcedPush.refs.find((ref) => ref.fullName === "refs/remotes/origin/main")?.id === rewrittenHead, "confirmed force-with-lease publishes the rewritten disposable branch");
    await shot("workspace-refs-remotes");
    await press("Refs", $(".gg-side-tabs")!); await sleep(100);
    {
      // Drag and drop: a commit dropped on a branch asks the cherry-pick confirmation, a branch dropped on a branch offers merge or rebase; Esc leaves everything as it was.
      const transfer = new DataTransfer(), fire = (type: string, el: Element) => el.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: transfer }));
      const branchRow = (name: string) => $$<HTMLElement>(".gg-side-row").find((r) => r.dataset.dropRef === `refs/heads/${name}`)!;
      const before = await repo(), commitRow = history("direct historical reword")!;
      log(commitRow.draggable, "commit rows are draggable");
      fire("dragstart", commitRow); fire("dragover", branchRow("review-branch"));
      log(!!await until(() => branchRow("review-branch").classList.contains("gg-drop-target")), "a commit dragged over a branch highlights the branch");
      fire("dragend", commitRow);
      log(!!await until(() => !branchRow("review-branch").classList.contains("gg-drop-target")), "ending the drag (Esc) removes the highlight");
      fire("dragstart", commitRow); fire("dragover", branchRow("review-branch")); fire("drop", branchRow("review-branch"));
      log(!!await until(() => { const text = $(".modal")?.textContent ?? ""; return text.includes("Cherry-pick") && text.includes("review-branch") && text.includes("Keyboard equivalent"); }), "dropping a commit on a branch asks the cherry-pick confirmation naming source, target and key");
      await shot("workspace-drop-cherry-pick");
      key("Escape", {}, $(".modal")); await until(() => !$(".modal")); await settle();
      fire("dragstart", branchRow("conflict-left")); fire("dragover", branchRow("review-branch")); fire("drop", branchRow("review-branch"));
      const dropMenu = () => $$(".menu .mi .lbl").map((label) => label.textContent?.trim());
      log(!!await until(() => dropMenu().includes("Merge conflict-left into review-branch…") && dropMenu().includes("Rebase conflict-left onto review-branch…")), "dropping a branch on a branch offers merge and rebase at the drop point");
      await shot("workspace-drop-branch-menu");
      key("Escape", {}, $(".menu")); await until(() => !$(".menu"));
      const after = await repo();
      log(after.head === before.head && after.branch === before.branch && !after.operation, "cancelled drops change nothing in the repository");
    }
    {
      // Squash of a chosen run: two adjacent commits chosen with Shift+click, `s` asks for the one message (Esc leaves the history as it was).
      const rows = $$<HTMLElement>(".gg-row[data-commit]"), before = await repo();
      if (rows.length >= 2) {
        click(rows[0]); await sleep(80);
        rows[1].dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, shiftKey: true })); await sleep(120);
        key("s", {}, rows[1]);
        log(!!await until(() => $(".modal textarea")), "s on two chosen commits asks for the message of the squashed commit");
        log(($(".modal textarea") as HTMLTextAreaElement | null)?.value.includes(rows[0].title.split("\n")[0]) === true, "the squash message starts from the messages of the chosen commits");
        await shot("workspace-squash-range-message");
        key("Escape", {}, $(".modal")); await until(() => !$(".modal")); await settle();
        log((await repo()).head === before.head, "cancelling the squash changes nothing");
      }
    }
    {
      // Hint mode: `;` labels what is on screen, a typed label clicks it, Esc leaves, and a field keeps its own typing.
      const tab = $$<HTMLElement>(".gg-side-tabs button").find((b) => b.textContent?.trim() === "Tags");
      const labelOf = (el: Element) => { const r = el.getBoundingClientRect(); return $$<HTMLElement>(".hint-label").find((l) => Math.abs(parseFloat(l.style.left) - r.left) < 3 && Math.abs(parseFloat(l.style.top) - r.top) < 3); };
      await selectedKey(";");
      log(!!await until(() => $$(".hint-label").length > 5 && !!$(".hint-bar")), "; shows hint labels on the controls that are on screen");
      const mark = tab ? labelOf(tab) : undefined;
      log(!!mark, "the Tags tab of the sidebar has a hint label");
      key("Escape", {}, document.body); await sleep(100);
      log(!$(".hint-layer"), "Esc leaves hint mode and nothing was clicked");
      await selectedKey(";");
      await until(() => $$(".hint-label").length > 5);
      const typed = (tab ? labelOf(tab)?.textContent ?? "" : "");
      for (const letter of typed) { key(letter, {}, document.body); await sleep(40); }
      log(!!await until(() => !$(".hint-layer") && $(".gg-side-tabs [aria-pressed='true']")?.textContent === "Tags", 4000), "typing a label clicks its control");
      await press("Refs", $(".gg-side-tabs")!); await sleep(100);
      const field = $(".gg-search") as HTMLInputElement;
      field.focus(); key(";", {}, field); await sleep(150);
      log(!$(".hint-layer"), "a plain ; typed in a field does not start hint mode");
      key(";", { ctrlKey: true }, field); await sleep(150);
      log(!!$(".hint-layer"), "Ctrl+; starts hint mode from a field");
      await shot("workspace-hint-labels");
      key("Escape", {}, document.body); await sleep(100);
    }
    const mergeBranch = async (name: string) => {
      const branch = await until(() => $<HTMLButtonElement>(`.gg-side-row[data-drag-ref="refs/heads/${name}"]:not(:disabled)`));
      if (!branch) throw new Error(`Missing local branch ${name}`);
      branch.focus();
      if (!await until(() => document.activeElement === branch && branch.classList.contains("selected"))) {
        throw new Error(`The branch ${name} did not receive selection and focus`);
      }
      key("M", { shiftKey: true }, branch);
      if (!await until(() => $(".modal .buttons"), 12000)) throw new Error(`The merge command did not ask for confirmation on ${name}`);
      await dialog("Merge");
    };
    await mergeBranch("conflict-left"); await settle();
    await mergeBranch("conflict-right");
    log(!!await until(() => $(".gg-operation")?.textContent?.includes("merge"), 8000), "conflicting merge remains visible and resumable");
    log(!!await until(() => $(".gg-conflicts-tab")?.getAttribute("aria-pressed") === "true", 4000), "Conflicts tab appears first and is selected when conflicts exist"); await sleep(80);
    click($$(".gg-side-row").find((r) => r.textContent?.includes("file.txt"))!);
    log(!!await until(() => $(".gm-card .gm-left")?.textContent?.includes("left side") && $(".gm-card .gm-right")?.textContent?.includes("right side") && $(".gm-card .gm-base")), "merge editor shows ours, theirs, base and live result");
    log(!!$(".gm-card .gm-left")?.textContent?.includes("ours") && !!$(".gm-card .gm-right")?.textContent?.includes("theirs"), "Git conflict sides name ours and theirs");
    log(!!$(".gg-conflict-commit")?.textContent?.includes("Merging") && !!$(".gg-operation .gg-applying"), "conflict names the commit being merged");
    await shot("workspace-conflict");
    // The editor has less width than the window because of the sidebar. Its cards reflow and retain space.
    await narrow(true);
    const resultBox = $(".gm-result")?.getBoundingClientRect(), cardsBox = $(".gm-cards")?.getBoundingClientRect();
    log(!!resultBox && resultBox.height >= 109 && !!cardsBox && cardsBox.height >= 199, "narrow merge editor keeps result and conflicts readable");
    const columns = $$(".gm-card .gm-row > .gm-col").map((el) => el.getBoundingClientRect());
    const preview = $(".gm-result")?.getBoundingClientRect(), leftSide = $(".gm-card .gm-left")?.getBoundingClientRect(), rightSide = $(".gm-card .gm-right")?.getBoundingClientRect(), baseSide = $(".gm-card .gm-base")?.getBoundingClientRect();
    log(columns.length === 3 && !!preview && !!leftSide && !!rightSide && !!baseSide && preview.bottom <= leftSide.top + 1 && baseSide.top >= Math.max(leftSide.bottom, rightSide.bottom) - 1, "preview is above the sides and base is below");
    // An error and the operation bar leave the sidebar its tabs and the conflict list some height.
    const aside = $(".gg-sidebar")?.getBoundingClientRect(), inAside = (el: Element) => { const r = el.getBoundingClientRect(); return !!aside && r.top >= aside.top - 1 && r.bottom <= aside.bottom + 1; };
    log(!!$(".gg-error") && $$(".gg-side-tabs button").every(inAside), "the sidebar shows all its tabs beside an error and a merge in a low window", `${Math.round(aside?.height ?? 0)} px, ${$$(".gg-side-tabs button").filter((b) => !inAside(b)).map((b) => b.textContent).join(", ")}`);
    const conflictList = $<HTMLElement>(".gg-side-list");
    const side = $<HTMLElement>(".gg-sidebar");
    if (side) side.scrollTop = side.scrollHeight;
    const listBox = conflictList?.getBoundingClientRect(), listHeight = listBox && aside ? Math.min(listBox.bottom, aside.bottom) - Math.max(listBox.top, aside.top) : 0;
    log(listHeight >= 119, "the conflict list keeps at least 120 px", `${Math.round(listHeight)} px`);
    if (side) side.scrollTop = 0;
    const bar = $(".gg-operation")?.getBoundingClientRect().height ?? 999, errorRow = $(".gg-error")?.getBoundingClientRect().height ?? 999;
    log(bar <= 44 && errorRow <= 40, "the error and the operation bar take one row each", `${Math.round(errorRow)} / ${Math.round(bar)} px`);
    click($(".gg-error button")!); await sleep(120);
    log(($(".gg-error")?.getBoundingClientRect().height ?? 0) > errorRow, "the whole error opens"); click($(".gg-error button")!); await sleep(120);
    $(".gm-cards")?.scrollIntoView({ block: "center" }); await sleep(200);
    await shot("workspace-conflict-narrow");
    await narrow(false);
    click($$<HTMLButtonElement>(".gm-actions button", $(".gm")!).find((b) => b.textContent?.startsWith("Edit"))!); await sleep(80);
    typeInto($("textarea[aria-label='Resolved file']") as HTMLTextAreaElement, "hand edited\n"); await sleep(80);
    await press("Done editing", $(".gm")!); await sleep(80);
    log(!!$(".gm-result")?.textContent?.includes("hand edited"), "manual edits survive leaving edit mode");
    click($(".gm-card .gm-left")!); await dialog("Discard edits"); await sleep(120);
    log(!$(".gm-result")?.textContent?.includes("hand edited") && !!$(".gm-result")?.textContent?.includes("left side"), "clicking Local replaces manual edits after confirmation");
    log($(".gm-card .gm-left")?.getAttribute("aria-pressed") === "true" && !!$(".gm-card .gm-left .gm-selected") && $(".gm-confirm") instanceof HTMLButtonElement && !($(".gm-confirm") as HTMLButtonElement).disabled, "clicking a side marks it and enables Confirm");
    click($(".gm-card .gm-base")!); await sleep(80);
    log($(".gm-card .gm-base")?.getAttribute("aria-pressed") === "true" && $(".gm-card .gm-left")?.getAttribute("aria-pressed") === "false", "clicking Base changes the selected side");
    click($(".gm-card .gm-right")!); await sleep(80);
    log($(".gm-card .gm-right")?.getAttribute("aria-pressed") === "true" && !!$(".gm-result")?.textContent?.includes("right side"), "clicking Remote updates the selection and result");
    click($(".gm-card .gm-left")!); await sleep(80);
    log(!!$(".gm-status")?.textContent?.includes("0 left"), "the status counts the conflicts that are left");
    await press("Confirm", $(".gm")!); await settle();
    log(!!await until(() => document.activeElement?.textContent?.trim().startsWith("Continue"), 4000), "after the last conflicted file the Continue button has focus, so Enter continues");
    await press("Continue", $(".gg-operation")!);
    log(!!await until(() => !$(".gg-operation"), 8000), "resolution and continue complete actual merge");
    const noteCommit = (await repo()).head;
    click($(`[data-commit="${noteCommit}"]`)!); await sleep(120);
    await press("Tools");
    log(!!await until(() => $(".git-advanced")?.textContent?.includes("Create worktree")), "advanced repository tools open in workspace");
    await shot("workspace-tools");
    await press("Notes", $(".ga-tabs")!);
    await until(() => $("textarea[aria-label='Git note']"));
    const note = $("textarea[aria-label='Git note']") as HTMLTextAreaElement;
    if (!note || note.disabled) throw new Error("Selected commit note editor unavailable");
    typeInto(note, "Review note"); await sleep(80); await press("Save note…", $(".git-advanced")!); await dialog("Save note"); await settle();
    const saved = await invoke<{notes:string}>("git_advanced", { path, commit: noteCommit });
    log(saved.notes.trimEnd() === "Review note", "commit note saves through backend");

    await press("Worktrees", $(".ga-tabs")!);
    const treePath = path + "-extra-worktree";
    typeInto($("[aria-label='Worktree destination']") as HTMLInputElement, treePath);
    typeInto($("[aria-label='Worktree commit']") as HTMLInputElement, noteCommit);
    typeInto($("[aria-label='Worktree branch']") as HTMLInputElement, "extra-worktree"); await sleep(100);
    await press("Create worktree…", $(".git-advanced")!); await dialog("Create worktree"); await settle();
    log(!!await until(() => $$(".ga-item").some((row) => row.textContent?.includes(treePath))), "create worktree completes through actual form");
    let treeRow = $$(".ga-item").find((row) => row.textContent?.includes(treePath))!;
    await press("Lock", treeRow); await dialog("Lock worktree"); await settle();
    treeRow = $$(".ga-item").find((row) => row.textContent?.includes(treePath))!;
    log(!!button("Unlock", treeRow) && !!(button("Remove…", treeRow) as HTMLButtonElement)?.disabled, "locked worktree cannot be removed");
    await shot("workspace-worktree-locked");
    await press("Unlock", treeRow); await dialog("Unlock worktree"); await settle();
    await press("Tools"); await until(() => !$(".git-advanced"));
    await press("Worktrees", $(".gg-side-tabs")!);
    const linkedRow = () => $$<HTMLButtonElement>(".gg-worktree-main").find((row) => row.title === treePath);
    log(!!await until(linkedRow), "worktree sidebar lists the created working folder");
    await h.updateSettings({ keyboardProfile: "explorer" });
    linkedRow()?.focus(); key("d", {}, linkedRow()); key("n", {}, linkedRow()); await sleep(250);
    log(!$(".modal") && !$(".git-advanced") && (await h.api.pathResolve(treePath)).exists, "worktree d and n do nothing when Vim keys are disabled");
    await shot("workspace-worktrees-vim-disabled");
    await h.updateSettings({ keyboardProfile: "vim" });
    linkedRow()?.focus(); key("d", {}, linkedRow());
    log(!!await until(() => $(".modal")?.textContent?.includes(treePath)), "Vim d asks to remove the focused worktree");
    await shot("workspace-worktree-remove-key-confirm");
    key("Escape", {}, $(".modal")); await until(() => !$(".modal")); await settle();
    linkedRow()?.focus(); key("n", {}, linkedRow());
    log(!!await until(() => $(".git-advanced [aria-label='Worktree destination']")), "Vim n opens the actual worktree creation form");
    treeRow = $$(".ga-item").find((row) => row.textContent?.includes(treePath))!;
    await press("Remove…", treeRow); await dialog("Remove worktree"); await settle();
    log(!(await h.api.pathResolve(treePath)).exists, "worktree removal removes only clean test worktree");
    // One key, no form: W creates in the preset folder, the toast opens or takes it back, the status stays clean.
    const toast = (text: string) => $$(".toast").find((row) => row.textContent?.includes(text));
    const clearToasts = async () => { $$<HTMLElement>(".toast").forEach((row) => click(row)); await sleep(200); };
    const quickPath = (name: string) => [path, ".worktrees", name].join(sep());
    const exists = async (name: string) => (await h.api.pathResolve(quickPath(name))).exists;
    const changesText = async () => (await invoke<{ changes: string }>("git_graph", { path })).changes;
    key("Escape", {}, $(".git-advanced")); await until(() => !$(".git-advanced")); await settle(); await clearToasts();
    const changesBefore = await changesText(), short = noteCommit.slice(0, 8);
    const selectNote = async () => { const row = $(`[data-commit="${noteCommit}"]`)!; row.focus(); click(row); await sleep(150); row.focus(); };
    await h.updateSettings({ keyboardProfile: "explorer" }); await selectNote();
    key("W", { shiftKey: true }, document.activeElement); await sleep(500);
    log(!toast("Worktree created") && !await exists(short), "W does nothing when Vim keys are disabled");
    await h.updateSettings({ keyboardProfile: "vim" }); await selectNote();
    key("W", { shiftKey: true }, document.activeElement);
    log(!!await until(() => toast("Worktree created"), 12000) && await exists(short), "W creates a worktree of the selected commit without a form", quickPath(short));
    await settle(); await shot("workspace-worktree-created");
    const created = toast("Worktree created")?.querySelector<HTMLElement>(".t");
    log(!!created && created.clientWidth > created.parentElement!.clientWidth * 0.6, "the text of a notification with several buttons keeps its width", `${created?.clientWidth}/${created?.parentElement?.clientWidth}`);
    log(await changesText() === changesBefore, "a nested worktree does not show up in the status of the main worktree");
    await press("Undo", toast("Worktree created")!);
    log(!!await until(() => toast("Worktree removed"), 12000) && !await exists(short), "Undo in the toast removes the unchanged worktree");
    await settle(); await clearToasts(); await selectNote();
    key("N", { shiftKey: true }, document.activeElement); await until(() => $(".modal input"));
    await shot("workspace-worktree-new-branch");
    await dialog("Create worktree", "quick/new");
    log(!!await until(() => toast("Worktree created"), 12000) && await exists("quick-new"), "N asks only for a branch name and creates its worktree");
    await settle(); await clearToasts();
    await press("Refs", $(".gg-side-tabs")!); await sleep(200);
    const quickRow = () => $$<HTMLButtonElement>(".gg-side-row").find((row) => row.textContent?.includes("quick/new"));
    // A click selects the row also while the window is not the active one, where the browser sends no focus events.
    await until(quickRow); click(quickRow()!); quickRow()!.focus();
    log(!!await until(() => quickRow()?.classList.contains("selected")), "the branch with the worktree is selected"); await settle();
    key("W", { shiftKey: true }, quickRow()!);
    log(!!await until(() => toast("already has a worktree"), 12000), "W on a branch that has a worktree offers to open it");
    await settle(); await shot("workspace-worktree-existing"); await clearToasts();
    // Dragging a branch onto the Worktrees tab does the same as W.
    const data = new DataTransfer(), tab = button("Worktrees", $(".gg-side-tabs")!)!;
    quickRow()!.dispatchEvent(new DragEvent("dragstart", { bubbles: true, cancelable: true, dataTransfer: data }));
    tab.dispatchEvent(new DragEvent("dragover", { bubbles: true, cancelable: true, dataTransfer: data })); await sleep(120);
    log(tab.classList.contains("gg-drop"), "the Worktrees tab accepts a dragged branch"); await shot("workspace-worktree-drop");
    tab.dispatchEvent(new DragEvent("drop", { bubbles: true, cancelable: true, dataTransfer: data }));
    log(!!await until(() => toast("already has a worktree"), 12000), "dropping a branch on the Worktrees tab creates or finds its worktree");
    await settle(); await clearToasts();
    // The form proposes the preset folder; a location for this repository is previewed before it is saved.
    await selectNote(); key("w", {}, document.activeElement);
    const destinationField = () => $("[aria-label='Worktree destination']") as HTMLInputElement | null;
    log(!!await until(() => destinationField()?.value === quickPath(short), 6000), "the form is filled with the preset folder", destinationField()?.value);
    const locationField = $("[aria-label='Worktree location of this repository']") as HTMLInputElement;
    typeInto(locationField, "{repo}/.git/{branch}");
    log(!!await until(() => $(".ga-preview-error"), 4000), "a location inside Git metadata is refused in the preview");
    locationField.scrollIntoView({ block: "center" }); await sleep(150); await shot("workspace-worktree-location-error");
    typeInto(locationField, "../{repo}-{branch}");
    log(!!await until(() => $(".ga-preview")?.textContent?.includes(`${path}-${short}`), 4000), "a sibling location is previewed live", $(".ga-preview")?.textContent ?? "");
    await shot("workspace-worktree-location"); await narrowShot("workspace-worktree-location");
    typeInto(locationField, ""); key("Enter", {}, locationField); await sleep(300);
    const linked = $$(".ga-item").find((row) => row.textContent?.includes(quickPath("quick-new")))!;
    await press("Remove…", linked); await dialog("Remove worktree"); await settle();
    log(!await exists("quick-new"), "a worktree created with a key is removed like any other");
    A.openOverlay({ type: "settings", section: "shortcuts" });
    const rewordKey = () => $<HTMLInputElement>("[data-shortcut='git.history.reword']");
    log(!!await until(rewordKey), "Settings lists the Git commands under Shortcuts");
    typeInto(rewordKey()!, "j"); key("Enter", {}, rewordKey()); await sleep(250);
    log(!!rewordKey()?.closest(".kb-row")?.querySelector(".kb-clash"), "Settings says that a navigation key cannot run a command");
    rewordKey()!.scrollIntoView({ block: "center" }); await sleep(150); await shot("workspace-git-shortcuts");
    typeInto(rewordKey()!, "Z, Ctrl+Shift+R"); key("Enter", {}, rewordKey()); await sleep(250);
    log(h.getState().settings?.shortcuts["git.history.reword"] === "Z, Ctrl+Shift+R" && !rewordKey()?.closest(".kb-row")?.querySelector(".kb-clash"), "Settings saves the keys of a Git command");
    typeInto(rewordKey()!, ""); key("Enter", {}, rewordKey()); await sleep(250);
    log(h.getState().settings?.shortcuts["git.history.reword"] === undefined, "an empty field restores the default keys");
    A.openOverlay(null); await sleep(200);
    A.openOverlay({ type: "settings", section: "behavior" });
    log(!!await until(() => $("[id='opt-worktreeLocation']")), "Settings has the location of new worktrees");
    $("[id='opt-worktreeLocation']")?.scrollIntoView({ block: "center" });
    log(!!await until(() => $(".opt [role='status']")?.textContent?.includes("feature-example"), 4000), "Settings previews the resulting folder", $(".opt [role='status']")?.textContent ?? "");
    await sleep(200); await shot("workspace-worktree-settings");
    A.openOverlay({ type: "gitGraph", path }); await until(() => $(`[data-commit="${noteCommit}"]`)); await showPanels();
    click($(`[data-commit="${noteCommit}"]`)!); await sleep(120); await press("Tools"); await until(() => $(".ga-tabs"));
    await press("Submodules", $(".ga-tabs")!); await shot("workspace-submodules-empty");
    await press("Bisect", $(".ga-tabs")!);
    const good = history("initial file")?.getAttribute("data-commit"); if (!good) throw new Error("Initial commit unavailable");
    typeInto($("[aria-label='Bisect good commit']") as HTMLInputElement, good);
    typeInto($("[aria-label='Bisect bad commit']") as HTMLInputElement, noteCommit); await sleep(80);
    await press("Start bisect…", $(".git-advanced")!); await dialog("Start bisect"); await settle();
    log((await repo()).bisectActive, "bisect starts and exposes actual test revision"); await shot("workspace-bisect");
    await press("Good", $(".git-advanced")!); await dialog("Mark good"); await settle();
    await press("Finish / reset…", $(".git-advanced")!); await dialog("Finish bisect"); await settle();
    log(!(await repo()).bisectActive && (await repo()).head === noteCommit, "bisect reset restores original branch and commit");
    await press("Repository", $(".ga-tabs")!);
    const clonedPath = path + "-clone";
    typeInto($("[aria-label='Clone URL']") as HTMLInputElement, parentOf(path) + sep() + "remote.git");
    typeInto($("[aria-label='Clone destination']") as HTMLInputElement, clonedPath); await sleep(80);
    await press("Clone…", $(".git-advanced")!); await dialog("Clone repository"); await settle();
    const cloneError = $(".ga-error")?.textContent;
    if (cloneError) throw new Error(`Clone failed: ${cloneError}`);
    const cloned = await invoke<Repository>("git_repository", { path: clonedPath });
    log(!!cloned.head && (await h.api.pathResolve(clonedPath + "/file.txt")).exists, "clone creates actual new working repository");
    await shot("workspace-clone");
    click(history("add alpha")!); await sleep(100); await press("Tools"); await until(() => $(".ga-tabs"));
    await press("Patches", $(".ga-tabs")!);
    await until(() => $("[aria-label='Patch export destination']"));
    const patchPath = path + "-alpha.patch";
    typeInto($("[aria-label='Patch export destination']") as HTMLInputElement, patchPath); await sleep(80);
    await press("Export commit patch…", $(".git-advanced")!); await dialog("Export patch"); await settle();
    log((await h.api.pathResolve(patchPath)).exists, "patch exports through actual destination form");
    typeInto($("[aria-label='Patch file']") as HTMLInputElement, patchPath); await sleep(80);
    await press("Check & apply…", $(".git-advanced")!); await dialog("Apply patch"); await settle();
    log(!!$(".ga-error") && (await repo()).head === noteCommit, "conflicting patch application reports error and preserves HEAD");
    await shot("workspace-patch-error");
    const moduleHost = parentOf(path) + sep() + "submodule-host";
    A.openOverlay(null); await sleep(100); A.openOverlay({ type: "gitGraph", path: moduleHost }); await until(() => $(".git-graph-lines .gg-row")); await showPanels();
    await press("Tools"); await until(() => $(".ga-tabs")); await press("Submodules", $(".ga-tabs")!);
    await press("Initialize…", $(".git-advanced")!); await dialog("Initialize submodule"); await settle();
    log((await h.api.pathResolve(moduleHost + sep() + "module" + sep() + "file.txt")).exists && !$(".ga-error"), "submodule initialization checks out recorded commit through UI");
    await shot("workspace-submodule-initialized");
    await press("Tools"); await until(() => !$(".git-advanced"));
    await press("Worktrees", $(".gg-side-tabs")!);
    log(!!await until(() => $<HTMLButtonElement>(".gg-worktree-main")?.title === moduleHost), "host worktree sidebar shows the host repository");
    click($("button[aria-label='Repository']")!); await until(() => $(".rp-menu"));
    click($$(".rp-item").find((item) => $(".rp-name", item)?.textContent === "module")!);
    await dialog("Switch");
    log(!!await until(() => $<HTMLButtonElement>(".gg-worktree-main")?.title === moduleHost + sep() + "module"), "switching repositories replaces host worktree rows with submodule rows");
    log(!$$<HTMLButtonElement>(".gg-worktree-main").some((row) => row.title === moduleHost), "submodule worktree list does not retain the previous host row");
    await shot("workspace-worktrees-submodule");
    click($("button[aria-label='Repository']")!); await until(() => $(".rp-menu")); click($$(".rp-item")[0]); await dialog("Switch");
    log(!!await until(() => $<HTMLButtonElement>(".gg-worktree-main")?.title === moduleHost), "switching back restores only the host repository worktrees");
    await shot("workspace-worktrees-host");
    const parent = parentOf(path);
    const emptyPath = await h.api.createDir(parent, "new-repository");
    A.openOverlay(null); await sleep(100); A.openOverlay({ type: "gitGraph", path: emptyPath });
    await until(() => button("Repository setup…")); await press("Repository setup…");
    await until(() => $(".ga-tabs")); await press("Repository", $(".ga-tabs")!);
    await press("Initialize repository…", $(".git-advanced")!); await dialog("Initialize repository"); await settle();
    const initialized = await invoke<Repository>("git_repository", { path: emptyPath });
    log(!initialized.head && initialized.branch === "main", "repository setup initializes an actual empty main branch");
    await shot("workspace-initialized-empty");
    await h.api.saveText(emptyPath + "/base.txt", "base content\n");
    click($("[aria-label='Refresh Git workspace']")!); await settle();
    click($(".gg-working")!); await until(() => $(".gc-file"));
    await press("Stage all", $(".git-changes")!); await until(() => $(".gc-file .staged"));
    typeInto($(".gc-commit textarea") as HTMLTextAreaElement, "first commit in new repository"); await sleep(80);
    await press("Commit staged", $(".git-changes")!); await settle();
    const firstRepo = await invoke<Repository>("git_repository", { path: emptyPath });
    log(!!firstRepo.head && !!$(".gc-files")?.textContent?.includes("Working tree clean"), "Working Changes creates first commit in initialized repository");
    await press("Collapse", $(".gg-working-title")!); click($(`[data-commit="${firstRepo.head}"]`)!); await sleep(100);
    await press("Tools"); await until(() => $(".ga-tabs")); await press("Patches", $(".ga-tabs")!);
    await until(() => $("[aria-label='Patch file']"));
    typeInto($("[aria-label='Patch file']") as HTMLInputElement, patchPath); await sleep(80);
    await press("Check & apply…", $(".git-advanced")!); await dialog("Apply patch"); await settle();
    const patchWorkspace = await invoke<{changes:{path:string;index:string}[]}>("git_workspace", { path: emptyPath });
    log(!$(".ga-error") && patchWorkspace.changes.some((change) => change.path === "alpha.txt" && change.index === "A") && (await h.api.pathResolve(emptyPath + "/alpha.txt")).exists, "checked patch application updates actual working file and index");
    await shot("workspace-patch-applied");
    // The "Load more commits" button must stay readable in both themes (it once fell back to
    // the platform's grey button face with accent-coloured text).
    // A canvas turns any CSS colour (rgb(), color(srgb …), color-mix results) into sRGB bytes.
    const canvas = document.createElement("canvas"); canvas.width = canvas.height = 1;
    const context = canvas.getContext("2d", { willReadFrequently: true })!;
    const rgba = (colour: string): number[] => { context.clearRect(0, 0, 1, 1); context.fillStyle = "#000"; context.fillStyle = colour; context.fillRect(0, 0, 1, 1); const [r, g, b, a] = context.getImageData(0, 0, 1, 1).data; return [r, g, b, a / 255]; };
    // The colour the user sees behind an element: its own background blended over its ancestors'.
    const backdrop = (element: Element): number[] => { let result = [0, 0, 0]; const layers: number[][] = []; for (let node: Element | null = element; node; node = node.parentElement) layers.push(rgba(getComputedStyle(node).backgroundColor)); for (const [r, g, b, a] of layers.reverse()) result = [r * a + result[0] * (1 - a), g * a + result[1] * (1 - a), b * a + result[2] * (1 - a)]; return result; };
    const luminance = ([r, g, b]: number[]) => { const [lr, lg, lb] = [r, g, b].map((v) => { const c = v / 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; }); return 0.2126 * lr + 0.7152 * lg + 0.0722 * lb; };
    const contrast = (a: number[], b: number[]) => { const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x); return (hi + 0.05) / (lo + 0.05); };
    for (const theme of ["dark", "light"] as const) {
      await h.updateSettings({ theme });
      A.openOverlay(null); await sleep(100); A.openOverlay({ type: "gitGraph", path: parent + sep() + "many-commits" });
      const more = await until(() => $<HTMLButtonElement>(".gg-load-more"), 12000);
      const style = more ? getComputedStyle(more) : null;
      const ratio = more ? contrast(rgba(style!.color), backdrop(more)) : 0;
      log(!!more && style!.appearance === "none" && ratio >= 4.5, `Load more commits is readable in the ${theme} theme`, more ? `contrast ${ratio.toFixed(2)}:1, appearance ${style!.appearance}` : "button missing");
      more?.scrollIntoView({ block: "center" }); await sleep(120);
      await shot(`workspace-load-more-${theme}`);
    }
    await h.updateSettings({ keyboardProfile: "explorer", theme: "light" });
    A.openOverlay(null); await sleep(100); A.openOverlay({ type: "gitGraph", path: emptyPath });
    await setWindowSize(760, 680);
    await shot("workspace-tools-narrow-light");
    const beforeDisabled = await invoke<Repository>("git_repository", { path: emptyPath }); key("P", {}, $(".git-graph")); await sleep(100);
    log(h.getState().overlay?.type !== "dialog" && (await invoke<Repository>("git_repository", { path: emptyPath })).head === beforeDisabled.head, "Vim-disabled letters do not trigger repository actions");
  } catch (e) { log(false, "Git workspace scenario", String(e)); await shot("workspace-failure"); }
  if (h.getState().info?.platform === "linux") {
    const { runMergeWorkspaceSelfTest } = await import("./selftestStandaloneMerge");
    await runMergeWorkspaceSelfTest(h, path || h.activePath(0));
    const { runMergeAiSelfTest } = await import("./selftestMergeAi");
    await runMergeAiSelfTest(h, path || h.activePath(0));
  }
  await h.finish();
}

/** Focused regression: staged nested repositories remain visible and can be excluded before rebase. */
export async function runGitRebaseSubmodulesSelfTest(h: typeof import("./selftest").gitSelftest) {
  const { $, $$, key, click, until, sleep, shot, setWindowSize, log } = h;
  const path = h.activePath(0);
  const repo = (root = path) => invoke<Repository>("git_repository", { path: root });
  const button = (label: string, scope: ParentNode = document) => $$<HTMLButtonElement>("button", scope).find((element) => { const text = element.cloneNode(true) as HTMLElement; text.querySelectorAll("kbd,svg").forEach((node) => node.remove()); return text.textContent?.trim() === label; });
  const press = async (label: string, scope: ParentNode = document) => { const element = await until(() => button(label, scope) ?? $$(".mi", scope).find((row) => $(".lbl", row)?.textContent?.trim() === label)); if (!element) throw new Error(`Missing action ${label}`); click(element); };
  const settle = async () => { await sleep(160); await until(() => !($(".gg-actions") as HTMLButtonElement)?.disabled, 12000); };
  const nestedFile = () => $$(".gc-file").find((row) => !!row.querySelector('[title=".worktrees"]') || !!row.querySelector('[title=".worktrees/"]'));
  try {
    await h.updateSettings({ keyboardProfile: "vim", theme: "dark", shortcuts: {} });
    await setWindowSize(1320, 900);
    const before = await repo(), nestedBefore = await repo(path + "/.worktrees");
    const actualCopy = before.branch === "MaybeSometime", destination = actualCopy ? "Dev" : "main";
    const moduleBefore = actualCopy ? null : await repo(path + "/module");
    const base = before.refs.find((ref) => ref.kind === "branch" && ref.name === destination);
    const beforeDetails = await invoke<{ message: string }>("git_commit_details", { path, commit: before.head, base: null });
    if (!base) throw new Error(`Missing ${destination} branch in rebase fixture`);
    const baseRow = () => $$(".gg-sidebar .gg-side-row").find((row) => row.querySelector(".gg-name")?.textContent === destination);
    const startRebase = async () => {
      const row = await until(baseRow, 12000); if (!row) throw new Error("Missing destination branch in sidebar");
      click(row); row.focus(); await sleep(120); key("r", {}, row);
      log(!!await until(() => $(".modal")?.textContent?.includes("Rebase commit?")), "Vim r on destination asks to rebase the current branch");
      log(!!$(".modal")?.textContent?.includes(base.id.slice(0, 12)) && !!$(".modal")?.textContent?.includes(`as the new base of ${before.branch}`), "confirmation identifies the selected destination and current branch");
    };
    log(before.head !== base.id && !!nestedBefore.head, "fixture has divergent branches and a staged nested repository");
    A.openOverlay({ type: "gitGraph", path });
    await until(() => $(".gg-working"), 12000);
    const changesBadge = $(".gg-working .gg-badge");
    log(!!changesBadge && /^\d+ changed$/.test(changesBadge.textContent?.trim() ?? ""), "Working Changes badge exposes the staged gitlink instead of showing Clean");
    click($(".gg-working")!);
    log(!!await until(() => nestedFile()?.querySelector(".staged")), "Working Changes lists the staged .worktrees gitlink");
    await shot("rebase-submodules-staged-working-changes");
    await press("Collapse", $(".gg-working-title")!);
    await startRebase(); await shot("rebase-submodules-blocked-confirm"); await press("Rebase", $(".modal")!);
    await until(() => !$(".modal")); await settle();
    log(!!$(".gg-error")?.textContent?.includes(".worktrees"), "rebase names the staged gitlink that must be unstaged");
    const blocked = await repo();
    log(blocked.head === before.head && !blocked.operation, "blocked rebase leaves HEAD unchanged and starts no operation");
    await shot("rebase-submodules-blocked");
    click($(".gg-working")!);
    const staged = await until(nestedFile); if (!staged) throw new Error("Missing staged nested repository");
    click(staged); await press("Unstage file", $(".git-changes")!); await settle();
    const unstaged = await until(() => { const row = nestedFile(); return row && !row.querySelector(".staged") ? row : null; });
    if (!unstaged) throw new Error("Nested repository was not unstaged");
    click(unstaged); await press("File actions…", $(".git-changes")!); await press("Exclude file locally", $(".menu")!); await settle();
    log(!nestedFile() && !$(".gc-error"), "local exclusion removes the untracked nested repository from Working Changes");
    await press("Collapse", $(".gg-working-title")!);
    await startRebase(); await shot("rebase-submodules-confirm"); await press("Rebase", $(".modal")!);
    await until(() => !$(".modal")); await settle();
    let after = await repo();
    if (actualCopy && after.operation === "rebase" && after.conflicts.length > 0) {
      log(!$(".gg-error")?.textContent?.includes("Unstage") && !!$(".gg-operation"), "rebase starts on the disposable project copy and exposes its genuine content conflicts");
      const conflict = await until(() => $$(".gg-sidebar .gg-side-row").find((row) => row.querySelector("span")?.firstChild?.textContent === after.conflicts[0]));
      if (!conflict) throw new Error("Running rebase did not expose its first conflict");
      click(conflict);
      log(!!await until(() => $(".gg-conflict")?.textContent?.includes(after.conflicts[0])), "actual project conflict can be inspected without resolving its content automatically");
      log(!!await until(() => $(".gm-card .gm-left")?.textContent?.includes("Ours / rebased so far") && $(".gm-card .gm-right")?.textContent?.includes("Theirs / replayed commit")), "rebase identifies the Git sides and their roles");
      await shot("rebase-submodules-actual-conflict");
      await press("Abort…", $(".gg-operation")!); await until(() => $(".modal"));
      await shot("rebase-submodules-actual-abort-confirm"); await press("Abort operation", $(".modal")!);
      await until(() => !$(".modal")); await settle(); after = await repo();
      log(after.head === before.head && after.branch === before.branch && !after.operation && after.conflicts.length === 0, "aborting the real project's conflicting rebase restores its original HEAD and branch");
    } else {
      log(!$(".gg-error"), "rebase finishes through the actual UI without the terminal-only error");
      log(after.branch === before.branch && after.head !== before.head && !after.operation && after.conflicts.length === 0, "rebase rewrites the current branch and leaves no pending operation or conflicts");
      const details = await invoke<{ parents: string[]; message: string }>("git_commit_details", { path, commit: after.head, base: null });
      log(details.parents.length === 1 && details.parents[0] === base.id && details.message === beforeDetails.message, "rebased commit has the selected destination as its actual parent and retains its message");
    }
    const nestedAfter = await repo(path + "/.worktrees");
    const sentinel = await h.api.grepContext(path + "/.worktrees/keep.txt", 1, 0, 1);
    log(nestedAfter.head === nestedBefore.head && sentinel.length === 1 && sentinel[0][1] === "nested repository sentinel", "unstage, local exclusion and rebase preserve the nested repository HEAD and sentinel");
    if (moduleBefore) {
      const moduleAfter = await repo(path + "/module");
      const topic = await h.api.grepContext(path + "/topic.txt", 1, 0, 0), upstream = await h.api.grepContext(path + "/upstream.txt", 1, 0, 0);
      log(topic.some(([, line]) => line === "topic content") && upstream.some(([, line]) => line === "upstream content"), "rebased working tree retains both topic and upstream contents");
      log(moduleAfter.head === moduleBefore.head, "rebase preserves the initialized submodule checkout");
      const moduleEdit = await h.api.grepContext(path + "/module/module.txt", 1, 0, 1), moduleUntracked = await h.api.grepContext(path + "/module/keep.txt", 1, 0, 1);
      log(moduleEdit.length === 1 && moduleEdit[0][1] === "module local edit" && moduleUntracked.length === 1 && moduleUntracked[0][1] === "keep untracked content", "rebase preserves tracked edits and untracked submodule contents with recursive configuration enabled");
    }
    await sleep(200); await shot("rebase-submodules-after");
    await h.updateSettings({ keyboardProfile: "explorer" });
    const row = await until(baseRow); if (!row) throw new Error("Destination branch disappeared after rebase");
    click(row); row.focus(); await sleep(100); key("r", {}, row); await sleep(150);
    log(!$(".modal") && (await repo()).head === after.head, "r does not trigger rebase when Vim keys are disabled");
  } catch (e) { log(false, "Rebase with submodules scenario", String(e)); await shot("rebase-submodules-failure"); }
  await h.finish();
}
