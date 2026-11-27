// Commands the user defines for the Git workspace (WN-47). A command is an argument list, never a shell string:
// the first entry is the program, `{name}` variables are replaced inside one argument at a time, so a value cannot
// add arguments or run anything else. The backend (`fs::gitcustom`) checks and expands again; these rules mirror it.

export type GitCustomContext = "commit" | "branch" | "tag" | "file" | "stash";
export type GitCommandDef = { id: string; label: string; context: GitCustomContext; command: string[]; key: string; confirm: boolean };
export const CUSTOM_CONTEXTS: GitCustomContext[] = ["commit", "branch", "tag", "file", "stash"];
export const CUSTOM_PREFIX = "custom:";
export const MAX_ARGS = 64;
export const MAX_ARG = 8192;
/** The variables a command may use, for the editor's hint. */
export const CUSTOM_VARIABLES = ["{hash}", "{branch}", "{tag}", "{stash}", "{file}", "{selection}", "{repo}", "{head}"] as const;

type Selected =
  | { kind: "commit"; id: string }
  | { kind: "reference"; ref: { name: string; id: string; kind: string } }
  | { kind: "stash"; stash: { selector: string; id: string } }
  | { kind: string };
type Where = { root: string; head: string; branch: string | null };

/**
 * The context of the selection and the variables a command gets there. `{selection}` is what is selected as text: the
 * hash, the branch or tag name, the stash name or the file. `{branch}` is the selected branch, else the current one.
 */
export function customTarget(target: Selected | null, where: Where | null, file?: string): { context: GitCustomContext; variables: Record<string, string> } | null {
  if (!where) return null;
  const common = { repo: where.root, head: where.head, branch: where.branch ?? "", hash: where.head, tag: "", stash: "", file: "", selection: "" };
  if (file !== undefined) return { context: "file", variables: { ...common, file, selection: file } };
  if (target?.kind === "commit") { const id = (target as { id: string }).id; return { context: "commit", variables: { ...common, hash: id, selection: id } }; }
  if (target?.kind === "reference") {
    const ref = (target as { ref: { name: string; id: string; kind: string } }).ref;
    if (ref.kind === "tag") return { context: "tag", variables: { ...common, tag: ref.name, hash: ref.id, selection: ref.name } };
    return { context: "branch", variables: { ...common, branch: ref.name, hash: ref.id, selection: ref.name } };
  }
  if (target?.kind === "stash") {
    const stash = (target as { stash: { selector: string; id: string } }).stash;
    return { context: "stash", variables: { ...common, stash: stash.selector, hash: stash.id, selection: stash.selector } };
  }
  return null;
}

/**
 * `{name}` in every argument replaced by its value; `{{` and `}}` are the braces themselves. An unknown name is an
 * error (a typo must not run with the text `{brnch}`), and the program (the first entry) takes none.
 */
export function expandCommand(argv: string[], variables: Record<string, string>): { argv: string[] } | { error: string } {
  const problem = commandProblem(argv);
  if (problem) return { error: problem };
  const out: string[] = [];
  for (let index = 0; index < argv.length; index++) {
    const part = argv[index];
    let text = "";
    for (let at = 0; at < part.length; at++) {
      const c = part[at];
      if (c === "{" && part[at + 1] === "{") { text += "{"; at++; }
      else if (c === "}" && part[at + 1] === "}") { text += "}"; at++; }
      else if (c === "{") {
        const end = part.indexOf("}", at);
        if (end < 0) return { error: `A variable is not closed in “${part}”` };
        const name = part.slice(at + 1, end);
        if (index === 0) return { error: "The program of a command cannot use a variable" };
        if (!Object.prototype.hasOwnProperty.call(variables, name)) return { error: `Unknown variable {${name}}` };
        text += variables[name];
        at = end;
      } else if (c === "}") return { error: `A stray } in “${part}”` };
      else text += c;
    }
    out.push(text);
  }
  return { argv: out };
}

/** What cannot be a command, in words; null when it can. */
export function commandProblem(argv: string[]): string | null {
  if (!argv.length) return "The command is empty";
  if (!argv[0].trim()) return "The command has no program";
  if (argv.length > MAX_ARGS) return "The command has too many arguments";
  if (argv.some((part) => part.includes("\0") || part.length > MAX_ARG)) return "The command has an invalid argument";
  return null;
}

/**
 * The argument list a typed command line stands for: spaces separate, a pair of single or double quotes keeps its text
 * as one argument, and inside double quotes `\"` is a quote (no other backslash means anything, so Windows paths stay
 * as typed). This only splits text into arguments; nothing is ever run through a shell. Null for an unclosed quote.
 */
export function splitCommandLine(text: string): string[] | null {
  const args: string[] = [];
  let current = "", started = false, quote: string | null = null;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quote) {
      if (c === quote) quote = null;
      else if (quote === '"' && c === "\\" && text[i + 1] === '"') { current += '"'; i++; }
      else current += c;
    } else if (c === '"' || c === "'") { quote = c; started = true; }
    else if (/\s/.test(c)) { if (started) { args.push(current); current = ""; started = false; } }
    else { current += c; started = true; }
  }
  if (quote) return null;
  if (started) args.push(current);
  return args;
}
/** The typed form of an argument list, which splits back into the same list. */
export function joinCommandLine(argv: string[]): string {
  return argv.map((part) => (part !== "" && !/[\s"']/.test(part) ? part : `"${part.replaceAll('"', '\\"')}"`)).join(" ");
}

/** Keys of the built-in commands of a context, so a custom key cannot hide one. */
export type KeyCheck = { problem: "label" | "command" | "key" | "taken" | "variables" | null; detail?: string };
export function checkDefinition(def: GitCommandDef, builtinKeys: readonly string[], others: readonly GitCommandDef[]): KeyCheck {
  if (!def.label.trim()) return { problem: "label" };
  const problem = commandProblem(def.command);
  if (problem) return { problem: "command", detail: problem };
  if (def.key && (def.key.length !== 1 || /\s/.test(def.key))) return { problem: "key" };
  if (def.key && builtinKeys.includes(def.key)) return { problem: "taken", detail: def.key };
  if (def.key && others.some((other) => other.id !== def.id && other.context === def.context && other.key === def.key)) return { problem: "taken", detail: def.key };
  // A variable that does not exist is found now, with the names every context can give.
  const probe = Object.fromEntries(CUSTOM_VARIABLES.map((name) => [name.slice(1, -1), ""]));
  const expanded = expandCommand(def.command, probe);
  if ("error" in expanded) return { problem: "variables", detail: expanded.error };
  return { problem: null };
}
