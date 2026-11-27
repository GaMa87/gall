// The badges of a history row: which references name its commit, each once, by full reference name. Pure, so it is
// unit-tested; the graph only draws them.

export type KnownRef = { name: string; fullName: string; kind: "branch" | "remote" | "tag" };
export type Badge = {
  /** What the badge says. */
  label: string;
  /** The full reference name for a branch or remote branch the repository knows; undefined for the rest. */
  fullName?: string;
  kind: "branch" | "remote" | "tag" | "saved" | "other";
  /** The checked-out branch (`HEAD -> name`). */
  current: boolean;
};

const PREFIXES = ["refs/heads/", "refs/remotes/", "heads/", "remotes/"];
const shortName = (name: string) => PREFIXES.reduce((rest, prefix) => (rest.startsWith(prefix) ? rest.slice(prefix.length) : rest), name);

/**
 * Turns the decorations Git printed for a commit (`HEAD -> main`, `origin/main`, `tag: v1`, a full name where a short
 * one is ambiguous) into badges: `HEAD -> name` becomes the branch plus a separate HEAD marker, a branch or remote
 * branch is looked up by its name and shown once under its full reference name. Tag names retain their Git spelling,
 * and a saved commit keeps its own kind.
 */
export function badgeRefs(decorations: string[], known: KnownRef[]): { head: boolean; badges: Badge[] } {
  let head = false;
  const badges: Badge[] = [];
  const seen = new Set<string>();
  const add = (key: string, badge: Badge) => { if (!seen.has(key)) { seen.add(key); badges.push(badge); } };
  for (const raw of decorations) {
    if (!raw) continue;
    if (raw === "HEAD") { head = true; continue; }
    let name = raw;
    let current = false;
    if (name.startsWith("HEAD -> ")) { head = true; current = true; name = name.slice(8); }
    if (name.startsWith("tag: ")) {
      const tag = name.slice(5);
      add(`tag:${tag}`, { label: tag, kind: "tag", current: false });
      continue;
    }
    if (name.startsWith("refs/gall/recovery/")) { add(name, { label: name, kind: "saved", current: false }); continue; }
    const short = shortName(name);
    const local = known.find((ref) => ref.kind === "branch" && ref.name === short);
    const remote = known.find((ref) => ref.kind === "remote" && ref.name === short);
    // A full name (`refs/heads/x`, `refs/remotes/o/x`) says which; a short one that both a branch and a remote
    // branch could be is the local branch (the remote one has `<remote>/` in front).
    const wantsRemote = name.startsWith("refs/remotes/") || name.startsWith("remotes/");
    const found = wantsRemote ? remote ?? local : local ?? remote;
    if (found) add(found.fullName, { label: found.name, fullName: found.fullName, kind: found.kind === "remote" ? "remote" : "branch", current: current && found.kind === "branch" });
    else add(`other:${short}`, { label: short, kind: "other", current });
  }
  return { head, badges };
}

/** The tag a remote reference stands for, or null. A name `<remote>/tags/<name>` is a fetched tag only when a tag
 *  `<name>` of the same commit exists: Git keeps fetched tags in `refs/tags`, so a name that merely looks like one is a
 *  real remote branch (`origin/tags/v1`) and stays a branch. */
export function remoteTagName(ref: { kind: string; name: string; id: string }, refs: { kind: string; name: string; id: string }[]): string | null {
  if (ref.kind !== "remote") return null;
  const name = /^[^/]+\/tags\/(.+)$/.exec(ref.name)?.[1];
  return name && refs.some((r) => r.kind === "tag" && r.name === name && r.id === ref.id) ? name : null;
}
