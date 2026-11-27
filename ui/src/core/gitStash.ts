import { fuzzyMatch } from "./fuzzy";
import type { GitStash } from "./gitRepository";

/** See gitstash.rs. */
export type StashInfo = { id: string; time: number; files: string[] };
export type StashFile = { path: string; added: number; deleted: number; line: number };

/** "On main: name" and "WIP on main: 1a2b3c4 subject" as branch and name. */
export function stashName(message: string): { branch: string | null; name: string; named: boolean } {
  const named = /^On ([^:]+): ([\s\S]*)$/.exec(message);
  if (named) return { branch: named[1] === "(no branch)" ? null : named[1], name: named[2], named: true };
  const wip = /^WIP on ([^:]+): (?:[0-9a-f]{4,64} )?([\s\S]*)$/.exec(message);
  if (wip) return { branch: wip[1] === "(no branch)" ? null : wip[1], name: wip[2], named: false };
  return { branch: null, name: message, named: true };
}

/** Stashes whose name, branch or a file matches, best first; without text in their order. */
export function searchStashes(stashes: GitStash[], infos: ReadonlyMap<string, StashInfo>, query: string): { stash: GitStash; file: string | null }[] {
  const text = query.trim();
  if (!text) return stashes.map((stash) => ({ stash, file: null }));
  return stashes.flatMap((stash, i) => {
    const own = fuzzyMatch(text, stash.message)?.score ?? -Infinity;
    let file: string | null = null, best = -Infinity;
    for (const path of infos.get(stash.id)?.files ?? []) {
      const score = fuzzyMatch(text, path)?.score ?? -Infinity;
      if (score > best) { best = score; file = path; }
    }
    // A file is named only when it is why the stash is listed.
    return own === -Infinity && best === -Infinity ? [] : [{ stash, file: best > own ? file : null, score: Math.max(own, best), i }];
  }).sort((a, b) => b.score - a.score || a.i - b.i).map(({ stash, file }) => ({ stash, file }));
}

/** The files of a patch with their changed lines and where they begin. */
export function patchFiles(lines: string[]): StashFile[] {
  const files: StashFile[] = [];
  let inHunk = false;
  lines.forEach((line, i) => {
    if (line.startsWith("diff --git ")) { files.push({ path: "", added: 0, deleted: 0, line: i }); inHunk = false; return; }
    const file = files[files.length - 1];
    if (!file) return;
    if (line.startsWith("@@ ")) inHunk = true;
    else if (inHunk && line.startsWith("+")) file.added++;
    else if (inHunk && line.startsWith("-")) file.deleted++;
    else if (!inHunk) {
      // The new name; a deleted file has only the old one. Binary files and pure renames have neither line.
      const name = /^(?:\+\+\+ b\/|rename to )(.+?)\t?$/.exec(line) ?? (file.path ? null : /^(?:--- a\/|rename from )(.+?)\t?$/.exec(line));
      if (name) file.path = name[1];
    }
  });
  for (const file of files) if (!file.path) file.path = / b\/(.+)$/.exec(lines[file.line])?.[1] ?? lines[file.line].slice(11);
  return files;
}
