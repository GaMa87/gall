// What the Git command palette lists and how it ranks it. Everything comes from the command registry and from data the
// workspace has already loaded, so opening and typing never wait for Git.
import { fuzzyMatch } from "./fuzzy";
import { GIT_SCOPES, GIT_SCOPE_LABELS, gitCommands, gitSharedCommands, type GitCommand, type GitScope, type GitShortcuts } from "./gitCommands";
import type { GitReference, GitRemote, GitStash } from "./gitRepository";

/** What an action works on; the palette asks for it in a second step when the selection is not one. */
export type PickKind = "commit" | "branch" | "remote" | "tag" | "stash" | "remoteRepository" | "conflict";
const revision: PickKind[] = ["branch", "remote", "tag", "commit"];
/** Actions with a parameter. Every action that needs a selection is listed (a test checks it). */
export const gitPickers: Record<string, PickKind[]> = {
  checkout: revision, createBranch: revision, createTag: revision, merge: revision, rebase: revision, reset: revision, revert: revision, cherryPick: revision, cherryPickTo: ["branch"], savePatch: ["commit"], pick: ["commit"],
  copyMenu: revision, compareBase: revision, worktreeHere: revision, worktreeNewBranch: revision, worktreeTry: revision, interactiveRebase: revision, interactiveRebaseOnto: revision,
  reword: ["commit"], amendSelected: ["commit"], "quick:fixup": ["commit"], "quick:squash": ["commit"], "quick:drop": ["commit"], "quick:moveEarlier": ["commit"], "quick:moveLater": ["commit"],
  checkoutRemote: ["remote"], renameBranch: ["branch"], deleteBranch: ["branch", "remote"], setUpstream: ["branch"], deleteTag: ["tag"], deleteRemoteTag: ["tag"], pushTag: ["tag"], copyRef: ["branch", "remote", "tag"], showHistory: ["branch", "remote", "tag"],
  stashApply: ["stash"], stashPop: ["stash"], stashDrop: ["stash"], stashBranch: ["stash"], stashRename: ["stash"],
  setRemoteUrl: ["remoteRepository"], renameRemote: ["remoteRepository"], removeRemote: ["remoteRepository"],
  editConflict: ["conflict"], resolveOurs: ["conflict"], resolveTheirs: ["conflict"], resolveMark: ["conflict"],
};
/** Asked even when something is selected: the point of finding them by name is to name the target as well. */
export const alwaysPicks = (action: string) => action === "checkout" || action === "cherryPickTo";
/** The target is an addition to the selection (a branch to bring it to) and does not replace it. */
export const keepsSelection = (action: string) => action === "cherryPickTo";
/** Targets that make no sense for an action: the current branch for a cherry-pick, and the same commit under a second name. */
export function sensibleCandidates(action: string, candidates: Candidate[], current: { branch: string | null; head: string }): Candidate[] {
  if (action !== "cherryPick" && action !== "cherryPickTo") return candidates;
  const local = new Set(candidates.filter((item) => item.kind === "branch").map((item) => item.ref!.id));
  return candidates.filter((item) => !(item.kind === "branch" && item.ref!.name === current.branch)
    && (action === "cherryPickTo" || ((item.ref?.id ?? item.id) !== current.head && !(item.kind === "remote" && local.has(item.ref!.id)))));
}
/** Fast existence check equivalent to `sensibleCandidates(action, pickCandidates(gitPickers[action] ?? [], source), current).length > 0`, without allocating candidate lists for thousands of commits. */
export function hasCandidate(action: string, source: CandidateSource, current: { branch: string | null; head: string }): boolean {
  const kinds = gitPickers[action];
  if (!kinds?.length) return false;
  if (action === "cherryPickTo") return source.refs.some((ref) => ref.kind === "branch" && !/\/HEAD$/.test(ref.name) && ref.name !== current.branch);
  if (action === "cherryPick") {
    let local: Set<string> | null = null;
    for (const kind of kinds) {
      if (kind === "branch" && source.refs.some((ref) => ref.kind === "branch" && !/\/HEAD$/.test(ref.name) && ref.name !== current.branch && ref.id !== current.head)) return true;
      if (kind === "tag" && source.refs.some((ref) => ref.kind === "tag" && !/\/HEAD$/.test(ref.name) && ref.id !== current.head)) return true;
      if (kind === "remote") {
        local ??= new Set(source.refs.filter((ref) => ref.kind === "branch" && !/\/HEAD$/.test(ref.name)).map((ref) => ref.id));
        if (source.refs.some((ref) => ref.kind === "remote" && !/\/HEAD$/.test(ref.name) && ref.id !== current.head && !local!.has(ref.id))) return true;
      }
      if (kind === "commit" && source.commits.some((commit) => commit.id !== current.head)) return true;
    }
    return false;
  }
  for (const kind of kinds) {
    if (kind === "commit") { if (source.commits.length > 0) return true; }
    else if (kind === "branch" || kind === "remote" || kind === "tag") { if (source.refs.some((ref) => ref.kind === kind && !/\/HEAD$/.test(ref.name))) return true; }
    else if (kind === "stash") { if (source.stashes.length > 0) return true; }
    else if (kind === "remoteRepository") { if (source.remotes.length > 0) return true; }
    else if (source.conflicts.length > 0) return true;
  }
  return false;
}

export type PaletteCommand = { command: GitCommand; scope: GitScope | "common"; scopeLabel: string };
/** Every Git command once. An action of several panels (checkout, worktree…) is taken from the panel in use. */
export function paletteCommands(current: GitScope, shortcuts: GitShortcuts = {}): PaletteCommand[] {
  const found = new Map<string, PaletteCommand>();
  for (const scope of [current, ...GIT_SCOPES.filter((other) => other !== current)]) for (const command of gitCommands(scope, shortcuts)) {
    if (command.shared || command.action === "palette" || scope === "search") continue;
    // A panel's own keys mean something only there (`panel:n` of two panels are two commands).
    const own = /^(file|detail|diff|panel):/.test(command.action) ? `${scope} ${command.action}` : command.action;
    // It asks for the target, so "Checkout selected commit" would say the wrong thing.
    if (!found.has(own)) found.set(own, { command: command.action === "checkout" ? { ...command, label: "Checkout" } : command, scope, scopeLabel: GIT_SCOPE_LABELS[scope] });
  }
  for (const command of gitSharedCommands(shortcuts)) if (command.action !== "palette") found.set(command.action, { command, scope: "common", scopeLabel: GIT_SCOPE_LABELS.common });
  return [...found.values()];
}

export type PaletteState = "run" | "pick" | "off";
export type RankedCommand = PaletteCommand & { state: PaletteState; label: string; positions: number[] };
/** Ranked by match, then recent use, then the panel in use; what cannot run now comes last. */
export function rankCommands(commands: PaletteCommand[], query: string, options: { stateOf: (item: PaletteCommand) => PaletteState; recent: string[]; current: GitScope; translate: (text: string) => string }): RankedCommand[] {
  const rows: (RankedCommand & { score: number; recency: number })[] = [];
  for (const item of commands) {
    const label = options.translate(item.command.label);
    let match = fuzzyMatch(query, label), positions = match?.positions ?? [];
    // "stash drop" finds a command of the Stashes panel, and the English name works in every language.
    if (!match) { match = fuzzyMatch(query, `${label} ${options.translate(item.scopeLabel)}`) ?? fuzzyMatch(query, `${options.translate(item.scopeLabel)} ${label}`) ?? fuzzyMatch(query, item.command.label); if (match) match = { ...match, score: match.score - 12 }; positions = []; }
    if (!match) continue;
    const used = options.recent.indexOf(item.command.id);
    rows.push({ ...item, state: options.stateOf(item), label, positions, score: query.trim() ? match.score : 0, recency: used < 0 ? 0 : options.recent.length - used });
  }
  const here = (row: RankedCommand) => (row.scope === options.current ? 1 : 0);
  return rows.map((row, order) => ({ row, order })).sort((a, b) =>
    Number(a.row.state === "off") - Number(b.row.state === "off") || Math.round(b.row.score) - Math.round(a.row.score) || b.row.recency - a.row.recency || here(b.row) - here(a.row) || a.order - b.order).map(({ row }) => row);
}

export type Candidate = { key: string; label: string; detail: string; kind: PickKind; ref?: GitReference; id?: string; stash?: GitStash; remote?: GitRemote; file?: string };
export type CandidateSource = { refs: GitReference[]; stashes: GitStash[]; remotes: GitRemote[]; conflicts: string[]; commits: { id: string; subject: string; author: string }[] };
/** The targets of an action, from what is loaded: branches, remote branches and tags first, then commits. */
export function pickCandidates(kinds: PickKind[], source: CandidateSource): Candidate[] {
  const result: Candidate[] = [];
  for (const kind of kinds) {
    if (kind === "branch" || kind === "remote" || kind === "tag") for (const ref of source.refs) { if (ref.kind === kind && !/\/HEAD$/.test(ref.name)) result.push({ key: ref.fullName, label: ref.name, detail: ref.id.slice(0, 8), kind, ref }); }
    else if (kind === "commit") for (const commit of source.commits) result.push({ key: commit.id, label: commit.subject, detail: `${commit.id.slice(0, 8)} · ${commit.author}`, kind, id: commit.id });
    else if (kind === "stash") for (const stash of source.stashes) result.push({ key: stash.selector + stash.id, label: stash.message, detail: stash.selector, kind, stash });
    else if (kind === "remoteRepository") for (const remote of source.remotes) result.push({ key: remote.name, label: remote.name, detail: remote.fetchUrl, kind, remote });
    else for (const file of source.conflicts) result.push({ key: file, label: file, detail: "", kind, file });
  }
  return result;
}
/** Matches names, subjects, hashes and stash selectors; an empty query keeps the given order. */
export function rankCandidates(candidates: Candidate[], query: string, limit = 200): (Candidate & { positions: number[] })[] {
  const q = query.trim();
  if (!q) return candidates.slice(0, limit).map((item) => ({ ...item, positions: [] }));
  const rows: { item: Candidate; score: number; positions: number[]; order: number }[] = [];
  candidates.forEach((item, order) => {
    const match = fuzzyMatch(q, item.label);
    // A typed hash is an exact wish.
    const hash = /^[0-9a-f]{4,40}$/i.test(q) && (item.id ?? item.ref?.id ?? "").startsWith(q.toLowerCase());
    if (hash) rows.push({ item, score: 10_000, positions: [], order });
    else if (match) rows.push({ item, score: match.score, positions: match.positions, order });
    else { const detail = fuzzyMatch(q, item.detail); if (detail) rows.push({ item, score: detail.score - 12, positions: [], order }); }
  });
  return rows.sort((a, b) => b.score - a.score || a.order - b.order).slice(0, limit).map(({ item, positions }) => ({ ...item, positions }));
}

const MAX_RECENT = 12;
const recent: string[] = [];
/** Commands run from the palette in this session, the latest first. */
export function recentCommands(): string[] { return recent; }
export function rememberCommand(id: string) {
  const at = recent.indexOf(id);
  if (at >= 0) recent.splice(at, 1);
  recent.unshift(id);
  recent.length = Math.min(recent.length, MAX_RECENT);
}
