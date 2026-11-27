// Every Git command in one place: what it is called, which panel it belongs to and which keys run it. The key handlers, the
// context menus, the `?` sheet, the hint bar, the key caps on buttons and Settings › Shortcuts all read this registry, so a
// remapped key (settings.shortcuts, id `git.<scope>.<action>`) changes everywhere. Moving between panels and rows (h j k l,
// arrows, Tab, Esc, q, ?) is fixed and cannot be taken by a command.
import { altGrChar } from "./keymap";

/** A panel with keys of its own. */
import { CUSTOM_PREFIX, type GitCommandDef, type GitCustomContext } from "./gitCustom";
export type GitContext = "history" | "files" | "branches" | "tags" | "stashes" | "remotes" | "worktrees" | "conflicts" | "details" | "tools";
/** Parts of a panel that handle their keys themselves. */
export type GitScope = GitContext | "diff" | "rebase" | "merge" | "search";
export type GitShortcuts = Readonly<Record<string, string>>;
export type GitCommand = {
  /** The name in settings.shortcuts. */
  id: string;
  /** The key to show; empty when nothing runs the command. */
  key: string;
  /** All keys that run it now. */
  keys: string[];
  defaults: string[];
  label: string;
  action: string;
  danger?: boolean;
  /** Available in every panel. */
  shared: boolean;
  /** The user chose the keys. */
  custom: boolean;
  /** Its chord works without Vim keys, whoever chose it. */
  always: boolean;
};
type Definition = { keys: string[]; label: string; action: string; danger?: boolean; always?: boolean };
const def = (keys: string | string[], label: string, action: string, danger = false): Definition => ({ keys: typeof keys === "string" ? [keys] : keys, label, action, ...(danger ? { danger } : {}) });

const common: Definition[] = [
  def("f", "Fetch remote", "fetch"),
  def("p", "Pull (fast-forward only)", "pull"),
  def("P", "Push branch", "push"),
  def("R", "Refresh workspace", "refresh"),
  def("m", "Continue / skip / abort", "operation"),
  def("x", "Repository tools…", "tools"),
  def("u", "Undo the last operation…", "undo"),
  def("Ctrl+r", "Redo the undone operation…", "redo"),
  def("U", "Operation history (undo timeline)", "journal"),
  def("z", "Go to the stashes", "showStashes"),
  def("H", "Reflog: find lost commits", "reflog"),
  { ...def([":", "Ctrl+Shift+p"], "Find a Git command…", "palette"), always: true },
  // `f` is Fetch here, so hint mode takes `;`; its chord works without Vim keys and from inside a dialog.
  { ...def([";", "Ctrl+;"], "Show hint labels (click by typing)", "hints"), always: true },
];
const worktreeHere = [def("W", "Create worktree here", "worktreeHere"), def("N", "New branch in a new worktree…", "worktreeNewBranch"), def("w", "Worktree options…", "tools:worktrees"), def("X", "Try in a temporary worktree", "worktreeTry")];
/** Actions starting with `file:`, `detail:`, `diff:` and `panel:` are carried out by the panel itself, on the default key after the colon. */
const definitions: Record<GitScope, Definition[]> = {
  history: [
    def("f", "Fixup into preceding commit…", "quick:fixup"),
    def("s", "Squash into preceding commit…", "quick:squash"),
    def("d", "Drop selected commit…", "quick:drop", true),
    def("A", "Amend selected commit…", "amendSelected", true),
    def(["Ctrl+j", "Alt+ArrowDown"], "Move commit earlier…", "quick:moveEarlier"),
    def(["Ctrl+k", "Alt+ArrowUp"], "Move commit later…", "quick:moveLater"),
    def("r", "Reword commit…", "reword"),
    def("i", "Interactive rebase from this commit…", "interactiveRebase"),
    def(" ", "Choose or leave out this commit", "pick"),
    def("v", "Choose a range of commits", "pickRange"),
    def("c", "Checkout selected commit…", "checkout"),
    def("n", "Create branch here…", "createBranch"),
    def("T", "Tag commit…", "createTag"),
    def("C", "Cherry-pick onto the current branch…", "cherryPick"),
    def("V", "Cherry-pick to another branch…", "cherryPickTo"),
    def("t", "Revert commit…", "revert"),
    def("g", "Reset to commit…", "reset"),
    worktreeHere[0], worktreeHere[1], worktreeHere[3],
    def("Ctrl+e", "Mark comparison base / compare", "compareBase"),
    def("y", "Copy commit attribute (hash, message, patch…)…", "copyMenu"),
    def("Y", "Save as patch file…", "savePatch"),
    def("a", "Apply patch from clipboard…", "applyPatch"),
    def("b", "Bisect options…", "tools:bisect"),
    worktreeHere[2],
    def("/", "Search commits", "search"),
  ],
  files: [
    def(" ", "Stage / unstage selected file", "file: "),
    def("a", "Stage / unstage all files", "file:a"),
    def("u", "Unstage selected file", "file:u"),
    def("c", "Commit message", "file:c"),
    def("Ctrl+Enter", "Commit staged changes", "file:Ctrl+Enter"),
    def("A", "Amend last commit…", "file:A", true),
    def("d", "Discard selected changes…", "file:d", true),
    def("i", "Ignore / exclude selected file…", "file:i"),
    def("e", "Edit selected file", "file:e"),
    def("w", "Bring the file or the chosen lines to another worktree…", "file:w"),
    def("s", "Stash working changes…", "stashSave"),
    def("S", "Stash selected file…", "file:S"),
    def("b", "Absorb staged changes into their commits…", "file:b"),
    def("F", "Put staged file into a commit…", "file:F"),
    def("r", "Refresh files", "file:r"),
  ],
  diff: [
    def([" ", "Enter"], "Stage or unstage the chosen lines", "diff: "),
    def("]", "Next hunk", "diff:]"),
    def("[", "Previous hunk", "diff:["),
    def("v", "Start or end a line range", "diff:v"),
    def("x", "Choose one line", "diff:x"),
    def("d", "Discard the chosen lines…", "diff:d", true),
    def("u", "Unstage the chosen lines", "diff:u"),
    def("S", "Stash the chosen lines…", "diff:S"),
    def("F", "Put the chosen staged lines into a commit…", "diff:F"),
  ],
  branches: [
    def("Z", "Discard the changes this branch remembers…", "discardWip", true),
    def([], "Save the changes this branch remembers as a stash", "wipStash"),
    def(" ", "Checkout selected reference…", "checkout"),
    def("n", "Create branch here…", "createBranch"),
    def("c", "Checkout remote branch as local…", "checkoutRemote"),
    def("R", "Rename branch…", "renameBranch"),
    def("d", "Delete branch…", "deleteBranch", true),
    def("r", "Rebase current branch onto selection…", "rebase"),
    def("M", "Merge selection into current branch…", "merge"),
    def("u", "Set upstream…", "setUpstream"),
    def("T", "Create tag here…", "createTag"),
    def("g", "Reset to selected reference…", "reset"),
    ...worktreeHere,
    def("y", "Copy reference name", "copyRef"),
  ],
  tags: [
    def(" ", "Checkout tagged commit…", "checkout"),
    def("h", "Show history of this tag", "showHistory"),
    def("n", "Create tag at HEAD…", "createTag"),
    def("d", "Delete local tag…", "deleteTag", true),
    def("D", "Delete remote tag…", "deleteRemoteTag", true),
    def("P", "Push tag…", "pushTag"),
    def("g", "Reset to tagged commit…", "reset"),
    ...worktreeHere,
    def("y", "Copy tag name", "copyRef"),
  ],
  stashes: [
    def(" ", "Apply stash…", "stashApply"),
    def("g", "Pop stash…", "stashPop"),
    def("d", "Drop stash…", "stashDrop", true),
    def("r", "Rename stash…", "stashRename"),
    def("s", "Save working changes…", "stashSave"),
    def("n", "Create branch from stash…", "stashBranch"),
    def("/", "Search stashes", "stashSearch"),
  ],
  remotes: [
    def("n", "Add remote…", "addRemote"),
    def("e", "Edit fetch URL…", "setRemoteUrl"),
    def("r", "Rename remote…", "renameRemote"),
    def("d", "Remove remote…", "removeRemote", true),
  ],
  conflicts: [
    def("e", "Edit conflicted file", "editConflict"),
    def("o", "Use current Git side (ours)…", "resolveOurs"),
    def("t", "Use incoming Git side (theirs)…", "resolveTheirs"),
    def(" ", "Mark file resolved…", "resolveMark"),
  ],
  worktrees: [
    def([" ", "Enter"], "Open worktree", "panel: "),
    def("n", "New worktree…", "panel:n"),
    def("d", "Remove worktree…", "panel:d", true),
    def("c", "Compare worktrees (first, then second)", "panel:c"),
    def("g", "Clean up worktrees…", "panel:g"),
    def("r", "Move worktree…", "panel:r"),
    def("t", "Open worktree in terminal", "panel:t"),
    def("e", "Open worktree in editor", "panel:e"),
  ],
  tools: [
    def("w", "Worktrees", "panel:w"),
    def("s", "Submodules", "panel:s"),
    def("b", "Bisect", "panel:b"),
    def("p", "Patches", "panel:p"),
    def("n", "Notes", "panel:n"),
    def("r", "Repository", "panel:r"),
  ],
  details: [
    def("Enter", "Focus file diff", "detail:Enter"),
    def("d", "Show diff", "detail:d"),
    def("v", "Show file", "detail:v"),
    def("f", "File history", "detail:f"),
    def("b", "Blame selected file", "detail:b"),
    def("B", "Blame beside every line / only the current line", "detail:B"),
    def("P", "Blame before the commit of this line", "detail:P"),
    def("L", "History of the selected lines…", "detail:L"),
    def("y", "Copy selected file path", "detail:y"),
    def("e", "Edit working file", "detail:e"),
    def("c", "Check out file", "detail:c"),
    def("[", "Older revision of the file", "detail:["),
    def("]", "Newer revision of the file", "detail:]"),
    def("D", "Show the change of each revision / the file", "detail:D"),
  ],
  rebase: [
    def("p", "Pick", "panel:p"),
    def("r", "Reword", "panel:r"),
    def("s", "Squash", "panel:s"),
    def("f", "Fixup", "panel:f"),
    def("d", "Drop", "panel:d", true),
    def("e", "Edit stop", "panel:e"),
    def(["J", "Ctrl+j", "Alt+ArrowDown"], "Move earlier", "panel:J"),
    def(["K", "Ctrl+k", "Alt+ArrowUp"], "Move later", "panel:K"),
    def("m", "Write the message", "panel:m"),
    def("x", "Add an exec line…", "panel:x"),
    def("a", "Apply the autosquash suggestion", "panel:a"),
    def("Ctrl+Enter", "Start rebase…", "panel:Ctrl+Enter", true),
  ],
  // Only while a search shows its matches; there they come before the keys of the commit list.
  search: [
    def("n", "Next match", "search:next"),
    def("N", "Previous match", "search:previous"),
  ],
  merge: [
    def("o", "Take ours", "panel:o"),
    def("t", "Take theirs", "panel:t"),
    def("b", "Combine changes", "panel:b"),
    def("B", "Take base", "panel:B"),
    def("]", "Next conflict", "panel:]"),
    def("[", "Previous conflict", "panel:["),
    def("e", "Edit the result", "panel:e"),
    def("p", "Propose a resolution (local model)…", "panel:p"),
    def("Ctrl+Enter", "Confirm the resolution", "panel:Ctrl+Enter"),
  ],
};
export const GIT_SCOPES = Object.keys(definitions) as GitScope[];
export const GIT_SCOPE_LABELS: Record<GitScope | "common", string> = {
  common: "All Git panels", history: "Commits", files: "Working Changes", diff: "Working Changes diff", branches: "Branches", tags: "Tags", stashes: "Stashes", remotes: "Remotes",
  conflicts: "Conflicts", worktrees: "Worktrees", tools: "Repository tools", details: "Commit details", rebase: "Interactive rebase", merge: "Merge editor", search: "Search matches",
};
/** Panels that do not pass keys on, so the shared commands do not apply there. */
const closed: GitScope[] = ["diff", "rebase", "merge", "search"];

/** Keys of the navigation between rows and panels; a command cannot be put on them. */
export const GIT_RESERVED = ["h", "j", "k", "l", "q", "?", "Escape", "Tab", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"];

type KeyEvent = { key: string; code?: string; ctrlKey: boolean; metaKey: boolean; altKey: boolean; shiftKey?: boolean; getModifierState?: (k: "AltGraph") => boolean };
/** The name of a pressed key: `w`, `W`, ` `, `Ctrl+j`, `Alt+ArrowDown`, `F5`. Cmd counts as Ctrl. */
export function gitKeyOf(e: KeyEvent): string {
  const typed = altGrChar(e);
  const ctrl = !typed && (e.ctrlKey || e.metaKey), alt = !typed && e.altKey;
  const key = (ctrl || alt) && e.key.length === 1 ? e.key.toLowerCase() : e.key;
  // Shift is part of a typed character, but not of Ctrl+j or F5.
  const shift = !!e.shiftKey && (key.length > 1 || ((ctrl || alt) && /^[a-z]$/.test(key)));
  return `${ctrl ? "Ctrl+" : ""}${alt ? "Alt+" : ""}${shift ? "Shift+" : ""}${key}`;
}
/** A key that types nothing, so it works without Vim keys too. */
export function isGitChord(key: string): boolean {
  return /^(Ctrl|Alt)\+./.test(key) || /^(Shift\+)?F\d{1,2}$/.test(key);
}
/** A key written in Settings (`Space`, `ctrl + J`) as the name a pressed key has. */
export function parseGitKey(text: string): string {
  const parts = text.trim().split("+").map((part) => part.trim());
  // "+" itself, alone or after a modifier, leaves an empty last part.
  const last = parts.pop() || "+";
  const mods = parts.map((part) => part.toLowerCase());
  let key = /^space$/i.test(last) ? " " : last;
  const ctrl = mods.some((m) => ["ctrl", "cmd"].includes(m)), alt = mods.some((m) => ["alt", "option"].includes(m)), shift = mods.includes("shift");
  if (key.length === 1) key = ctrl || alt ? key.toLowerCase() : shift ? key.toUpperCase() : key;
  else key = key[0].toUpperCase() + key.slice(1);
  return `${ctrl ? "Ctrl+" : ""}${alt ? "Alt+" : ""}${shift && (key.length > 1 || ctrl || alt) ? "Shift+" : ""}${key}`;
}
export function gitKeyName(key: string): string {
  return key.replace(/ $/, "Space");
}
const commandId = (scope: GitScope | "common", action: string) => `git.${scope}.${action.replace(/^(file|detail|panel|diff):/, "").replace(/^ $/, "space").replace(/:/g, ".")}`;

function resolve(list: { scope: GitScope | "common"; item: Definition }[], shortcuts: GitShortcuts): GitCommand[] {
  const chosen = list.map(({ scope, item }) => {
    const text = shortcuts[commandId(scope, item.action)];
    return text === undefined ? null : text.split(",").map((part) => part.trim()).filter(Boolean).map(parseGitKey).filter((key) => !GIT_RESERVED.includes(key));
  });
  // A key the user chose beats a default, a command of the panel beats a shared one, and the first of two equals wins.
  const taken = new Set<string>();
  const keys: string[][] = list.map(() => []);
  for (const custom of [true, false]) for (const shared of [false, true]) list.forEach(({ scope, item }, i) => {
    if ((scope === "common") !== shared || (chosen[i] !== null) !== custom) return;
    keys[i] = (chosen[i] ?? item.keys).filter((key) => !taken.has(key) && !!taken.add(key));
  });
  return list.map(({ scope, item }, i) => ({ id: commandId(scope, item.action), key: keys[i][0] ?? "", keys: keys[i], defaults: item.keys, label: item.label, action: item.action, ...(item.danger ? { danger: true } : {}), shared: scope === "common", custom: chosen[i] !== null, always: !!item.always }));
}
/** The commands of a panel and the shared ones, with the keys that run them now. Shared commands whose keys the panel uses itself are left out. */
/** The panel a custom command of a context belongs to. */
export const CUSTOM_SCOPE: Record<GitCustomContext, GitScope> = { commit: "history", branch: "branches", tag: "tags", file: "files", stash: "stashes" };
let customDefinitions: Partial<Record<GitScope, Definition[]>> = {};
let customSource: readonly GitCommandDef[] | undefined;
/**
 * The commands the user defined join the registry of their panel (as `custom:<id>`), so the palette, the menus, the key
 * sheet and the key bindings of Settings treat them like the built-in ones. Called with the saved list whenever it changes.
 */
export function setCustomGitCommands(defs: readonly GitCommandDef[] | undefined) {
  if (defs === customSource) return;
  customSource = defs;
  customDefinitions = {};
  for (const item of defs ?? []) {
    const scope = CUSTOM_SCOPE[item.context];
    if (!scope || !item.id || !item.label.trim()) continue;
    (customDefinitions[scope] ??= []).push({ keys: item.key ? [item.key] : [], label: item.label, action: `${CUSTOM_PREFIX}${item.id}` });
  }
}
export function gitCommands(scope: GitScope, shortcuts: GitShortcuts = {}): GitCommand[] {
  const own = [...definitions[scope], ...(customDefinitions[scope] ?? [])].map((item) => ({ scope, item }));
  const all = resolve(closed.includes(scope) ? own : [...own, ...common.map((item) => ({ scope: "common" as const, item }))], shortcuts);
  return all.filter((command) => !command.shared || command.keys.length > 0);
}
export function gitSharedCommands(shortcuts: GitShortcuts = {}): GitCommand[] {
  return resolve(common.map((item) => ({ scope: "common" as const, item })), shortcuts);
}
/** The default keys of the built-in commands of a panel, for checking a key a custom command asks for. */
export function builtinKeys(scope: GitScope): string[] {
  return [...definitions[scope], ...common].flatMap((item) => item.keys);
}
export function gitCommandForKey(scope: GitScope, key: string, shortcuts: GitShortcuts = {}): GitCommand | undefined {
  return gitCommands(scope, shortcuts).find((item) => item.keys.includes(key));
}
/** What a pressed key means to a panel that knows only the default keys: the default key of the command it runs, an empty
 *  string for a default key that now runs nothing, and any other key (navigation, typing) unchanged. */
export function gitDefaultKey(scope: GitScope, pressed: string, shortcuts: GitShortcuts = {}): string {
  const command = gitCommandForKey(scope, pressed, shortcuts);
  if (command) return command.defaults.includes(pressed) ? pressed : command.defaults[0];
  return definitions[scope].some((item) => item.keys.includes(pressed)) ? "" : pressed;
}
/** The key for a panel (the first of `scopes` that knows it) and whether it may act: letters need Vim keys, a chord the user chose does not. */
export function gitPanelKey(scopes: GitScope[], event: KeyEvent, shortcuts: GitShortcuts, vim: boolean): { key: string; on: boolean } {
  const pressed = gitKeyOf(event);
  for (const scope of scopes) {
    const command = gitCommandForKey(scope, pressed, shortcuts);
    if (command) return { key: command.shared ? pressed : gitDefaultKey(scope, pressed, shortcuts), on: vim || ((command.custom || command.always) && isGitChord(pressed)) };
  }
  return { key: scopes.some((scope) => gitDefaultKey(scope, pressed, shortcuts) === "") ? "" : pressed, on: vim };
}
/** The key cap for a command given by its default key, empty when it would not work (no key left, or a letter without Vim keys). */
export function gitKeyCap(scope: GitScope, defaultKey: string, shortcuts: GitShortcuts, vim: boolean): string {
  const command = gitCommands(scope, shortcuts).find((item) => item.defaults.includes(defaultKey));
  return command ? gitShownKey(command, vim) : "";
}
export function gitShownKey(command: GitCommand, vim: boolean): string {
  const key = vim ? command.key : command.custom || command.always ? command.keys.find(isGitChord) ?? "" : "";
  return gitKeyName(key);
}
/** Two commands of one panel on the same default key. */
export function gitKeyConflicts(): string[] {
  return GIT_SCOPES.flatMap((scope) => {
    const seen = new Map<string, string>();
    return definitions[scope].flatMap((item) => item.keys.flatMap((key) => { const other = seen.get(key); seen.set(key, item.action); return other ? [`${scope} ${gitKeyName(key)}: ${other} and ${item.action}`] : []; }));
  });
}
/** Default keys with more than one meaning across the panels, as `key: action, action`. */
export function gitKeyMeanings(): Map<string, string[]> {
  const found = new Map<string, Set<string>>();
  for (const item of [...GIT_SCOPES.filter((scope) => !closed.includes(scope)).flatMap((scope) => definitions[scope]), ...common]) for (const key of item.keys) found.set(key, (found.get(key) ?? new Set()).add(item.action));
  return new Map([...found].filter(([, actions]) => actions.size > 1).map(([key, actions]) => [key, [...actions]]));
}
/** Every command once, for Settings. */
export function gitCommandList(shortcuts: GitShortcuts = {}): { scope: GitScope | "common"; commands: GitCommand[] }[] {
  return [{ scope: "common" as const, commands: gitSharedCommands(shortcuts) }, ...GIT_SCOPES.map((scope) => ({ scope, commands: gitCommands(scope, shortcuts).filter((command) => !command.shared) }))];
}

/** The hint bar, like lazygit's options bar: default keys (every one a command of the panel, or navigation) with short labels. */
export const contextHints: Record<GitContext, [key: string, label: string][]> = {
  history: [["r", "reword"], ["f", "fixup"], ["s", "squash"], ["d", "drop"], ["i", "interactive"], ["y", "copy"], ["a", "apply patch"], ["W", "worktree"], ["x", "tools"], [":", "find"], ["u", "undo"], [" ", "choose"], ["C", "cherry-pick"], ["c", "checkout"]],
  files: [[" ", "stage"], ["a", "stage all"], ["c", "commit"], ["d", "discard"], ["e", "edit"], ["s", "stash"]],
  branches: [[" ", "checkout"], ["r", "rebase"], ["M", "merge"], ["n", "new"], ["R", "rename"], ["W", "worktree"], ["d", "delete"]],
  tags: [[" ", "checkout"], ["n", "new"], ["W", "worktree"], ["d", "delete"], ["g", "reset"]],
  stashes: [["/", "search"], [" ", "apply"], ["g", "pop"], ["d", "drop"], ["n", "branch"], ["s", "save"]],
  remotes: [["n", "add"], ["e", "edit"], ["r", "rename"], ["d", "remove"]],
  worktrees: [[" ", "open"], ["n", "new"], ["c", "compare"], ["g", "clean up"], ["r", "move"], ["t", "terminal"], ["d", "remove"]],
  conflicts: [["e", "edit"], ["o", "ours"], ["t", "theirs"], [" ", "mark resolved"]],
  details: [["j", "next file"], ["d", "diff"], ["v", "file"], ["f", "history"], ["b", "blame"], ["y", "copy path"]],
  tools: [["w", "worktrees"], ["s", "submodules"], ["b", "bisect"], ["p", "patches"], ["n", "notes"], ["r", "repository"]],
};
/** The hints with the keys that work now; a command without a key is left out. */
export function gitHints(context: GitContext, shortcuts: GitShortcuts = {}): { key: string; label: string }[] {
  const commands = gitCommands(context, shortcuts);
  return contextHints[context].map(([key, label]) => { const command = commands.find((item) => item.defaults.includes(key)); return { key: command ? command.key : key, label }; }).filter((hint) => !!hint.key);
}
