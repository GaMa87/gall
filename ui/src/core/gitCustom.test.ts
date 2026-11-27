import { afterEach, expect, it } from "vitest";
import { builtinKeys, gitCommandForKey, gitCommands, setCustomGitCommands } from "./gitCommands";
import { paletteCommands } from "./gitPalette";
import { checkDefinition, customTarget, expandCommand, joinCommandLine, splitCommandLine, type GitCommandDef } from "./gitCustom";

const where = { root: "/work/app", head: "f".repeat(40), branch: "main" };
const def = (patch: Partial<GitCommandDef> = {}): GitCommandDef => ({ id: "log", label: "Log of this", context: "commit", command: ["git", "log", "{hash}"], key: "", confirm: true, ...patch });
afterEach(() => setCustomGitCommands(undefined));

it("puts a value into one argument and never lets it add arguments or reach a shell", () => {
  const hostile = "x; rm -rf / && $(evil) \"q\" 'p' `t` --upload-pack=bad\nnext";
  const expanded = expandCommand(["git", "log", "--grep={branch}", "{hash}"], { branch: hostile, hash: "abc" });
  expect(expanded).toEqual({ argv: ["git", "log", `--grep=${hostile}`, "abc"] });
  expect("argv" in expanded && expanded.argv).toHaveLength(4);
});

it("writes braces with doubled braces and refuses typos, stray braces, empty commands and a variable program", () => {
  expect(expandCommand(["tool", "{{x}}"], {})).toEqual({ argv: ["tool", "{x}"] });
  expect(expandCommand(["tool", "{brnch}"], { branch: "b" })).toHaveProperty("error");
  expect(expandCommand(["tool", "{open"], {})).toHaveProperty("error");
  expect(expandCommand(["tool", "stray}"], {})).toHaveProperty("error");
  expect(expandCommand(["{hash}"], { hash: "a" })).toHaveProperty("error");
  expect(expandCommand([], {})).toHaveProperty("error");
  expect(expandCommand(["  "], {})).toHaveProperty("error");
  expect(expandCommand(["tool", "a\0b"], {})).toHaveProperty("error");
});

it("gives each selection its context and variables", () => {
  const commit = customTarget({ kind: "commit", id: "abc" }, where)!;
  expect(commit.context).toBe("commit");
  expect(commit.variables).toMatchObject({ hash: "abc", selection: "abc", branch: "main", repo: "/work/app" });
  const branch = customTarget({ kind: "reference", ref: { name: "feature/x", id: "def", kind: "branch" } }, where)!;
  expect(branch).toMatchObject({ context: "branch", variables: { branch: "feature/x", hash: "def", selection: "feature/x" } });
  expect(customTarget({ kind: "reference", ref: { name: "v1", id: "d", kind: "tag" } }, where)).toMatchObject({ context: "tag", variables: { tag: "v1" } });
  expect(customTarget({ kind: "stash", stash: { selector: "stash@{1}", id: "s1" } }, where)).toMatchObject({ context: "stash", variables: { stash: "stash@{1}" } });
  expect(customTarget(null, null)).toBeNull();
  expect(customTarget({ kind: "working" }, where)).toBeNull();
  // A file of Working Changes comes with its own context, whatever else is selected.
  expect(customTarget(null, where, "src/a b.ts")).toMatchObject({ context: "file", variables: { file: "src/a b.ts", selection: "src/a b.ts" } });
});

it("splits a typed line into arguments without a shell and joins it back unchanged", () => {
  expect(splitCommandLine("git log --oneline {hash}")).toEqual(["git", "log", "--oneline", "{hash}"]);
  expect(splitCommandLine(`tool "a b" 'c d' e"f g"h`)).toEqual(["tool", "a b", "c d", "ef gh"]);
  expect(splitCommandLine(`C:\\Tools\\run.exe "C:\\My Files\\x"`)).toEqual(["C:\\Tools\\run.exe", "C:\\My Files\\x"]);
  expect(splitCommandLine(`say "he said \\"hi\\""`)).toEqual(["say", 'he said "hi"']);
  expect(splitCommandLine(`tool ""`)).toEqual(["tool", ""]);
  expect(splitCommandLine("  ")).toEqual([]);
  expect(splitCommandLine(`tool "open`)).toBeNull();
  // `;`, `&&`, `$()` and pipes are ordinary text of one argument, not syntax.
  expect(splitCommandLine("echo a;b && c | d $(x)")).toEqual(["echo", "a;b", "&&", "c", "|", "d", "$(x)"]);
  for (const argv of [["git", "log"], ["a b", "", 'c"d', "e'f"], ["C:\\p q\\x.exe", "{hash}"]]) expect(splitCommandLine(joinCommandLine(argv))).toEqual(argv);
});

it("checks a definition: name, command, variables, a one-character key that hides no built-in key", () => {
  const history = builtinKeys("history");
  expect(checkDefinition(def(), history, []).problem).toBeNull();
  expect(checkDefinition(def({ label: " " }), history, []).problem).toBe("label");
  expect(checkDefinition(def({ command: [] }), history, []).problem).toBe("command");
  expect(checkDefinition(def({ command: ["git", "log", "{nope}"] }), history, []).problem).toBe("variables");
  expect(checkDefinition(def({ key: "ab" }), history, []).problem).toBe("key");
  expect(checkDefinition(def({ key: " " }), history, []).problem).toBe("key");
  // `s` squashes in the commit panel; a free key does not clash, and a key already given to another command does.
  expect(checkDefinition(def({ key: "s" }), history, [])).toMatchObject({ problem: "taken", detail: "s" });
  expect(checkDefinition(def({ key: "9" }), history, []).problem).toBeNull();
  expect(checkDefinition(def({ key: "9" }), history, [def({ id: "other", key: "9" })]).problem).toBe("taken");
  expect(checkDefinition(def({ key: "9", context: "branch" }), history, [def({ id: "other", key: "9" })]).problem).toBeNull();
});

it("adds the commands of the user to the registry of their panel, so key, palette and menus find them", () => {
  setCustomGitCommands([def({ key: "9" }), def({ id: "br", label: "Show branch", context: "branch", key: "" }), def({ id: "f", label: "Blame file", context: "file", key: "8" })]);
  const own = gitCommands("history").find((command) => command.action === "custom:log")!;
  expect(own).toMatchObject({ label: "Log of this", key: "9", keys: ["9"] });
  expect(gitCommandForKey("history", "9")?.action).toBe("custom:log");
  // No key: still listed (menu, palette), reachable by nothing else.
  expect(gitCommands("branches").find((command) => command.action === "custom:br")).toMatchObject({ key: "", keys: [] });
  expect(gitCommands("files").some((command) => command.action === "custom:f")).toBe(true);
  expect(gitCommands("stashes").some((command) => command.action.startsWith("custom:"))).toBe(false);
  expect(paletteCommands("history").some((item) => item.command.action === "custom:log")).toBe(true);
  // Settings › Shortcuts can rebind it like any command.
  expect(gitCommandForKey("history", "7", { [own.id]: "7" })?.action).toBe("custom:log");
  // A built-in key keeps its meaning even when a custom command asks for it.
  setCustomGitCommands([def({ key: "s" })]);
  expect(gitCommandForKey("history", "s")?.action).toBe("quick:squash");
  // Removing them removes the entries.
  setCustomGitCommands([]);
  expect(gitCommands("history").some((command) => command.action.startsWith("custom:"))).toBe(false);
});
