import { describe, expect, it } from "vitest";
import { buildKeymap, DEFAULT_KEYMAP, type Action, type KeyboardProfile } from "./keymap";
import { buildLeaderTree, buildVimTree, CTRL_TAP, keysForAction, leaderToken, listBindings, physicalLetter, PREVIOUS_MARK, resolve, shadowedByVim, shortcutLabel, splitKeys, treeForSettings, vimToken, type KeyNode } from "./vim";

describe("vim keys", () => {
  const tree = buildVimTree("Space");

  it("splits key specs into tokens", () => {
    expect(splitKeys("gT")).toEqual(["g", "T"]);
    expect(splitKeys("Ctrl+d")).toEqual(["Ctrl+d"]);
    expect(splitKeys("Space")).toEqual(["Space"]);
    expect(splitKeys("Ctrl+w w")).toEqual(["Ctrl+w", "w"]);
  });

  it("resolves single keys, sequences and prefixes", () => {
    expect(resolve(tree, ["j"])).toEqual({ kind: "action", action: "cursorDown" });
    expect(resolve(tree, ["g"]).kind).toBe("pending");
    expect(resolve(tree, ["g", "g"])).toEqual({ kind: "action", action: "cursorTop" });
    expect(resolve(tree, ["G"])).toEqual({ kind: "action", action: "cursorBottom" });
    expect(resolve(tree, ["y", "y"])).toEqual({ kind: "action", action: "copy" });
    expect(resolve(tree, ["Ctrl+d"])).toEqual({ kind: "action", action: "halfPageDown" });
    expect(resolve(tree, ["Z"]).kind).toBe("none");
    expect(resolve(tree, ["z"]).kind).toBe("pending"); // +zip group
    expect(resolve(tree, ["z", "x"])).toEqual({ kind: "action", action: "extractHere" });
    expect(resolve(tree, ["d", "D"])).toEqual({ kind: "action", action: "deletePermanent" });
    expect(resolve(tree, ["g", "z"]).kind).toBe("none");
    expect(resolve(tree, ["="]).kind).toBe("pending");
    expect(resolve(tree, ["=", "="])).toEqual({ kind: "action", action: "compare" });
    expect(resolve(tree, ["=", "h"])).toEqual({ kind: "action", action: "compareGitHead" });
    expect(resolve(tree, ["=", "r"]).kind).toBe("none");
    expect(resolve(tree, ["g", "r"])).toEqual({ kind: "action", action: "gitGraph" });
    expect(resolve(tree, ["=", "g"])).toEqual({ kind: "action", action: "compareGitCommit" });
    expect(resolve(tree, ["+", "+"])).toEqual({ kind: "action", action: "focusOther" });
  });

  it("leader menu with groups", () => {
    const p = resolve(tree, ["Space"]);
    expect(p.kind).toBe("pending");
    if (p.kind === "pending") expect(p.node.children!.get("f")!.label).toBe("+file/find");
    expect(resolve(tree, ["Space", "Space"])).toEqual({ kind: "action", action: "quickOpen" });
    expect(resolve(tree, ["Space", "f", "f"])).toEqual({ kind: "action", action: "quickOpen" });
    expect(resolve(tree, ["Space", "s", "G"])).toEqual({ kind: "action", action: "grepRoot" });
  });

  it("pins and unpins through the normal key or discoverable buffer leader group", () => {
    expect(resolve(tree, ["b"])).toEqual({ kind: "action", action: "togglePin" });
    expect(resolve(tree, ["Space", "b", "p"])).toEqual({ kind: "action", action: "togglePin" });
    expect(resolve(buildLeaderTree(CTRL_TAP), [CTRL_TAP, "b", "p"])).toEqual({ kind: "action", action: "togglePin" });
    expect(resolve(buildVimTree(";"), [";", "b", "p"])).toEqual({ kind: "action", action: "togglePin" });
  });

  it("offers a vim or leader sequence for every keyboard action", () => {
    const actions = new Set(listBindings(tree).map((binding) => binding.action));
    const missing = (Object.keys(DEFAULT_KEYMAP) as Action[]).filter((action) => !actions.has(action));
    expect(missing).toEqual([]);
    expect(resolve(tree, ["v"])).toEqual({ kind: "action", action: "visualMode" });
    expect(resolve(tree, ["Space", "v"])).toEqual({ kind: "action", action: "toggleSelect" });
    expect(resolve(tree, ["Space", "o", "m"])).toEqual({ kind: "action", action: "contextMenu" });
  });

  it("custom leader", () => {
    const t = buildVimTree(";");
    expect(resolve(t, [";", "f", "f"])).toEqual({ kind: "action", action: "quickOpen" });
    expect(resolve(t, ["Space"]).kind).toBe("none");
  });

  it("conventions from vim / yazi / lazygit / LazyVim", () => {
    const act = (...k: string[]) => resolve(tree, k);
    expect(act("x")).toEqual({ kind: "action", action: "cut" });
    expect(act("q")).toEqual({ kind: "action", action: "closeTab" });
    expect(act("i")).toEqual({ kind: "action", action: "preview" });
    expect(act("I")).toEqual({ kind: "action", action: "properties" });
    expect(act("-")).toEqual({ kind: "action", action: "parent" });
    expect(act("[")).toEqual({ kind: "action", action: "prevTab" });
    expect(act("]")).toEqual({ kind: "action", action: "nextTab" });
    expect(act("3")).toEqual({ kind: "action", action: "tab3" });
    expect(act("Ctrl+w", "w")).toEqual({ kind: "action", action: "focusOther" });
    expect(act("Ctrl+w", "h")).toEqual({ kind: "action", action: "focusLeft" });
    expect(act("Ctrl+h")).toEqual({ kind: "action", action: "focusLeft" });
    expect(shadowedByVim("Ctrl+H")).toBe(true);
    expect(shadowedByVim("Ctrl+N")).toBe(false);
    expect(shadowedByVim("Ctrl+Shift+H")).toBe(false);
    expect(act("Ctrl+l")).toEqual({ kind: "action", action: "focusRight" });
    expect(act(",", "S")).toEqual({ kind: "action", action: "sortSizeDesc" });
    expect(act("Space", "s", "g")).toEqual({ kind: "action", action: "grepFolder" });
    expect(act("Space", "b", "d")).toEqual({ kind: "action", action: "closeTab" });
    expect(act("Space", "w", "w")).toEqual({ kind: "action", action: "focusOther" });
    expect(act("Space", "f", "n")).toEqual({ kind: "action", action: "newFile" });
  });

  it("labels shortcuts with the keys of the current mode", () => {
    const ctx = (vim: boolean, over: Record<string, string> = {}) => ({ vim, keymap: buildKeymap(over), leader: vim ? "Space" : "Ctrl Ctrl" });
    expect(shortcutLabel("Ctrl+H", ctx(false))).toBe("Ctrl+H");
    expect(shortcutLabel("Ctrl+H", ctx(true))).toBe(".");
    expect(shortcutLabel("Ctrl+C", ctx(true))).toBe("y y");
    expect(shortcutLabel("Ctrl+H", ctx(false, { toggleHidden: "Alt+H" }))).toBe("Alt+H");
    // A chord vim has no key of its own for stays as is.
    expect(shortcutLabel("Alt+S", ctx(true))).toBe("Alt+S");
    // Pin has a letter with vim keys; swap is vim's Ctrl+w x (Ctrl+U is half page up).
    expect(shortcutLabel("Ctrl+B", ctx(true))).toBe("b");
    expect(shortcutLabel("Ctrl+U", ctx(true))).toBe("Ctrl+w x");
    expect(shortcutLabel("Ctrl+U", ctx(false))).toBe("Ctrl+U");
    expect(keysForAction("toggleHidden", ctx(false))).toBe("Ctrl+H");
    // A leader key typed on the file list opens the leader menu: its chord does nothing else.
    const bs = { vim: false, keymap: buildKeymap({}), leader: "\\" };
    expect(shortcutLabel("\\", bs)).toBe("Alt+D");
    expect(keysForAction("editPath", bs)).toBe("Alt+D");
  });
  it("shows each profile's effective keys for legacy menu hints", () => {
    for (const profile of ["explorer", "totalCommander", "vim"] as KeyboardProfile[]) {
      const ctx = { vim: profile === "vim", keymap: buildKeymap({}, profile), leader: profile === "vim" ? "Space" : "Ctrl Ctrl" };
      expect(shortcutLabel("F4", ctx)).toBe(profile === "vim" ? "e" : profile === "explorer" ? "Ctrl+Alt+E" : "F4");
      expect(shortcutLabel("F5", ctx)).toBe(profile === "vim" ? "R" : profile === "totalCommander" ? "F2" : "F5");
      expect(keysForAction("copyToOther", ctx)).toBe(profile === "vim" ? "C" : profile === "totalCommander" ? "F5" : "Alt+C");
      expect(keysForAction("longNames", ctx)).not.toBeNull();
    }
  });
  it("keeps every non-motion Gall command accessible in all three profiles", () => {
    const motions = new Set<Action>(["cursorDown", "cursorUp", "cursorTop", "cursorBottom", "halfPageDown", "halfPageUp", "enter", "filter", "tab1", "tab2", "tab3", "tab4", "tab5", "tab6", "tab7", "tab8", "tab9", "visualMode"]);
    for (const profile of ["explorer", "totalCommander", "vim"] as KeyboardProfile[]) {
      const ctx = { vim: profile === "vim", keymap: buildKeymap({}, profile), leader: profile === "vim" ? "Space" : "Ctrl Ctrl" };
      const missing = (Object.keys(DEFAULT_KEYMAP) as Action[]).filter((a) => !motions.has(a) && !keysForAction(a, ctx));
      expect(missing).toEqual([]);
    }
  });

  it("Ctrl tap / double tap as leader", () => {
    expect(leaderToken("Ctrl")).toBe(CTRL_TAP);
    expect(leaderToken("Ctrl Ctrl")).toBe(CTRL_TAP);
    expect(leaderToken("Space")).toBe("Space");
    const t = buildVimTree(CTRL_TAP);
    expect(resolve(t, [CTRL_TAP, "f", "f"])).toEqual({ kind: "action", action: "quickOpen" });
    expect(resolve(t, ["j"])).toEqual({ kind: "action", action: "cursorDown" });
  });

  it("vim keys off: only the leader menu, letters stay free", () => {
    const t = buildLeaderTree(CTRL_TAP);
    expect(resolve(t, ["j"]).kind).toBe("none");
    expect(resolve(t, ["g", "g"]).kind).toBe("none");
    expect(resolve(t, [CTRL_TAP, "Space"])).toEqual({ kind: "action", action: "quickOpen" });
    expect(resolve(buildLeaderTree(""), [CTRL_TAP]).kind).toBe("none");
    const basic = treeForSettings({ vimKeys: false, leaderKey: "Space", leaderKeyBasic: "Ctrl Ctrl" });
    expect(resolve(basic, [CTRL_TAP, "s", "G"])).toEqual({ kind: "action", action: "grepRoot" });
  });

  it("no binding hides another (a key is either an action or a prefix)", () => {
    const walk = (n: KeyNode, path: string[]): string[] =>
      [...(n.children ?? [])].flatMap(([k, c]) => [
        ...(c.action && c.children?.size ? [[...path, k].join(" ")] : []),
        ...walk(c, [...path, k]),
      ]);
    for (const l of ["Space", ";", "\\", CTRL_TAP]) expect(walk(buildVimTree(l), [])).toEqual([]);
  });

  it("tokens use Shift for case and ignore Alt", () => {
    const ev = (key: string, m: Partial<{ ctrlKey: boolean; altKey: boolean; shiftKey: boolean }> = {}) =>
      vimToken({ key, ctrlKey: false, metaKey: false, altKey: false, ...m });
    expect(ev("g", { shiftKey: true })).toBe("G");
    expect(ev(" ")).toBe("Space");
    expect(ev("D", { ctrlKey: true })).toBe("Ctrl+d");
    // Ctrl+Shift+D is "compare", not vim's half page down; same for Ctrl+Shift+L.
    expect(ev("D", { ctrlKey: true, shiftKey: true })).toBeNull();
    expect(ev("L", { ctrlKey: true, shiftKey: true })).toBeNull();
    expect(ev("x", { altKey: true })).toBeNull();
    expect(ev("Shift")).toBeNull();
    expect(ev("ArrowDown")).toBeNull();
  });

  it("uses Shift rather than Caps Lock letter casing for Vim keys", () => {
    const ev = (key: string, shiftKey = false) =>
      vimToken({ key, ctrlKey: false, metaKey: false, altKey: false, shiftKey });
    expect(ev("j")).toBe("j");
    expect(ev("J")).toBe("j");
    expect(ev("J", true)).toBe("J");
    expect(ev("j", true)).toBe("J");
  });

  it("folder marks: m sets, ' and ` jump, dm deletes; c is the colour tag", () => {
    const act = (...k: string[]) => resolve(tree, k);
    expect(act("m").kind).toBe("pending");
    expect(act("m", "a")).toEqual({ kind: "mark", op: "set", key: "a" });
    expect(act("m", "A")).toEqual({ kind: "mark", op: "set", key: "A" });
    expect(act("'", "a")).toEqual({ kind: "mark", op: "jump", key: "a" });
    expect(act("`", "Z")).toEqual({ kind: "mark", op: "jumpItem", key: "Z" });
    // '' and `` (also ' then `) go back to the folder before the last jump.
    expect(act("'", "'")).toEqual({ kind: "mark", op: "jump", key: PREVIOUS_MARK });
    expect(act("`", "`")).toEqual({ kind: "mark", op: "jumpItem", key: PREVIOUS_MARK });
    expect(act("d", "m", "q")).toEqual({ kind: "mark", op: "delete", key: "q" });
    expect(act("d", "d")).toEqual({ kind: "action", action: "cut" });
    // Only letters name a mark; the previous-folder mark is not set or deleted by hand.
    for (const bad of ["1", "'", "Space", "Ctrl+d"]) expect(act("m", bad).kind).toBe("none");
    expect(act("d", "m", "'").kind).toBe("none");
    expect(act("m", "a", "b").kind).toBe("none");
    expect(act("c")).toEqual({ kind: "action", action: "tag" });
    // Vim keys off: m, ' and ` stay free for typing to filter.
    for (const k of ["m", "'", "`"]) expect(resolve(buildLeaderTree(CTRL_TAP), [k]).kind).toBe("none");
    // No mark key hides an action or a group.
    const walk = (n: KeyNode): boolean => [...(n.children ?? [])].every(([, c]) => (!c.mark || (!c.action && !c.children)) && walk(c));
    expect(walk(tree)).toBe(true);
  });

  it("a plain key vim takes over is not offered as the shortcut (` toggled columns)", () => {
    expect(shadowedByVim("`")).toBe(true);
    expect(shadowedByVim(".")).toBe(true);
    expect(shadowedByVim("Shift+G")).toBe(true);
    expect(shadowedByVim("Shift+Y")).toBe(false);
    expect(shadowedByVim("F4")).toBe(false);
    const vim = { vim: true, keymap: buildKeymap({}, "vim"), leader: "Space" };
    expect(keysForAction("toggleColumns", vim)).toBe("␣ w c");
    expect(keysForAction("toggleColumns", { vim: false, keymap: buildKeymap({}), leader: "Ctrl Ctrl" })).toBe("`");
  });

  it("dead keys still reach the mark keys", () => {
    const ev = (key: string, code: string, shiftKey = false) => ({ key, code, shiftKey, ctrlKey: false, metaKey: false, altKey: false });
    expect(vimToken(ev("Dead", "Quote"))).toBe("'");
    expect(vimToken(ev("Dead", "Backquote"))).toBe("`");
    expect(vimToken(ev("Dead", "Quote", true))).toBeNull();
    expect(vimToken(ev("Dead", "Equal"))).toBeNull();
    // ' then a composes á: the mark name is the physical letter.
    expect(physicalLetter(ev("á", "KeyA"))).toBe("a");
    expect(physicalLetter(ev("Á", "KeyA", true))).toBe("A");
    expect(physicalLetter(ev("č", "Semicolon"))).toBeNull();
  });

  it("every binding is reachable and unique", () => {
    const rows = listBindings(tree);
    const seen = new Set(rows.map((r) => r.keys.join(" ")));
    expect(seen.size).toBe(rows.length);
    for (const r of rows) expect(resolve(tree, r.keys)).toEqual({ kind: "action", action: r.action });
  });
});

import { fuzzyMatch } from "./fuzzy";

describe("fuzzyMatch", () => {
  it("matches in order with positions and prefers word starts", () => {
    expect(fuzzyMatch("nt", "New tab")!.positions).toEqual([0, 4]);
    expect(fuzzyMatch("xyz", "New tab")).toBeNull();
    expect(fuzzyMatch("nt", "New tab")!.score).toBeGreaterThan(fuzzyMatch("nt", "Content")!.score);
    expect(fuzzyMatch("Tab", "new tab")).toBeNull();
  });
});
