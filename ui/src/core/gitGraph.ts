/**
 * Assign lanes to commits and route parent edges through each row.
 *
 * Lines never shift sideways between commits: a lane keeps its column from where it starts to where it
 * ends. A branch keeps its own lane and color down to the row of its first parent, where it turns into
 * that commit. Columns follow one rule: the main line is column 0; branches merged into it (second
 * parents of its merge commits) come next, always to its right and nearest to it; then unmerged
 * branches, shortest first; the longest go furthest right. A long branch that itself has branches
 * merged into it acts as a secondary main line: its merged branches sit directly to its right.
 */
export function layoutHistory(history: string) {
  const commits = history.trimEnd().split("\n").filter(Boolean).map((line) => {
    const [id, parents, refs, subject, author, date, stash] = line.split("\0");
    // A stash entry has a seventh field, its selector (`stash@{0}`), and one parent: the commit it was made on. Its
    // lane is its own, so a commit of the same object (a branch that points at the stash commit) is a row of its own.
    return { id, key: stash ? `stash:${stash}` : id, stash: stash || undefined, parents: parents ? parents.split(" ") : [], refs: refs ? refs.split(", ") : [], subject, author, date };
  });
  // `owner`: the row (by key) that last sent this lane on its way; the edges of the lane belong to it.
  type Track = { id: string; color: number; start: number; end: number; col: number; host?: Track; owner: string };
  type Edge = { from: number; to: number; color: number; owner: string };
  const tracks: Track[] = [];
  let nextColor = 0;
  const newTrack = (id: string, start: number, host?: Track, owner = ""): Track => { const track = { id, color: nextColor++, start, end: Infinity, col: -1, host, owner }; tracks.push(track); return track; };

  // Pass 1: follow the lanes without columns and record where each one starts and ends.
  let waiting: Track[] = [];
  const steps = commits.map((commit, row) => {
    // Who owns each lane as it arrives at this row (before this row sends them on).
    const before = new Map(waiting.map((track) => [track, track.owner] as const));
    const mine = waiting.filter((track) => track.id === commit.key);
    const tip = mine.length === 0;
    const own = tip ? newTrack(commit.key, row) : mine[0];
    if (tip) waiting.push(own);
    const joining = mine.slice(1);
    joining.forEach((track) => { track.end = row; });
    waiting = waiting.filter((track) => !joining.includes(track));
    const targets = commit.parents.map((parent, order) => {
      if (order === 0) { own.id = parent; own.owner = commit.key; return own; }
      const existing = waiting.find((track) => track.id === parent);
      if (existing) return existing;
      const created = newTrack(parent, row, own, commit.key);
      waiting.push(created);
      return created;
    });
    if (!commit.parents.length) { own.end = row; waiting = waiting.filter((track) => track !== own); }
    return { own, tip, joining, targets, before };
  });

  // Pass 2: columns. Two lanes may share a column only if their rows do not overlap (a lane that ends
  // at a commit and one that starts at the same commit only touch).
  const last = commits.length - 1;
  const stop = (track: Track) => Math.min(track.end, last + 1);
  const taken: Track[][] = [];
  const place = (track: Track, from: number) => {
    let col = from;
    while ((taken[col] ?? []).some((other) => other.start < stop(track) && track.start < stop(other))) col++;
    (taken[col] ??= []).push(track);
    track.col = col;
  };
  const children = new Map<Track, Track[]>();
  tracks.forEach((track) => { if (track.host) children.set(track.host, [...(children.get(track.host) ?? []), track]); });
  const placeMerged = (host: Track) => (children.get(host) ?? []).forEach((child) => { if (child.col < 0) { place(child, host.col + 1); placeMerged(child); } });
  if (steps.length) { place(steps[0].own, 0); placeMerged(steps[0].own); }
  tracks.filter((track) => track.col < 0 && !track.host)
    .sort((a, b) => (stop(a) - a.start) - (stop(b) - b.start) || a.start - b.start)
    .forEach((track) => { if (track.col < 0) { place(track, 0); placeMerged(track); } });
  tracks.filter((track) => track.col < 0).forEach((track) => place(track, (track.host?.col ?? -1) + 1));

  // Pass 3: rows.
  const byStart = new Map<number, Track[]>();
  tracks.forEach((track) => byStart.set(track.start, [...(byStart.get(track.start) ?? []), track]));
  const live = new Set<Track>();
  return commits.map((commit, row) => {
    const { own, tip, joining, targets, before } = steps[row];
    (byStart.get(row) ?? []).forEach((track) => live.add(track));
    const passing = [...live].filter((track) => track !== own && track.start < row && track.end > row);
    // A lane that passes or joins belongs to the row that sent it on; one that leaves belongs to this row.
    const sender = (track: Track) => before.get(track) ?? track.owner;
    const edges: Edge[] = [
      ...passing.map((track) => ({ from: track.col, to: track.col, color: track.color, owner: sender(track) })),
      ...joining.map((track) => ({ from: track.col, to: own.col, color: track.color, owner: sender(track) })),
      ...targets.map((track) => ({ from: own.col, to: track.col, color: track.color, owner: commit.key })),
    ].sort((a, b) => a.from - b.from);
    const width = Math.max(1, ...[...live].map((track) => track.col + 1));
    live.forEach((track) => { if (track.end <= row) live.delete(track); });
    // The row that sent the lane which arrives at this one (the line above the node); none for a tip.
    const incoming = tip ? undefined : sender(own);
    return { ...commit, lane: own.col, color: own.color, edges, tip, width, incoming };
  });
}

/**
 * The commits of the loaded history that a branch tip reaches through parents, the tip and both sides of every merge
 * included. A stash row is never part of it (it has no children and is nobody's parent), and a parent that is not
 * loaded ends the walk there: only what is known from the loaded rows is returned. Keyed by commit id.
 */
export function reachableFrom(rows: { id: string; stash?: string; parents: string[] }[], tip: string): Set<string> {
  const byId = new Map<string, string[]>();
  for (const row of rows) if (!row.stash && !byId.has(row.id)) byId.set(row.id, row.parents);
  const seen = new Set<string>();
  const pending = [tip];
  while (pending.length) {
    const id = pending.pop()!;
    const parents = byId.get(id);
    if (seen.has(id) || !parents) continue;
    seen.add(id);
    pending.push(...parents);
  }
  return seen;
}

/** Whether a row or an edge keeps full emphasis under a branch focus: its row, or the row the edge belongs to, is reached. */
export const inFocus = (reached: ReadonlySet<string>, key: string): boolean => reached.has(key);

/** Several commits chosen in the list. `rows` is the list as shown, newest first. */
export function rangeIds(rows: { id: string }[], from: string, to: string): string[] {
  const a = rows.findIndex((row) => row.id === from), b = rows.findIndex((row) => row.id === to);
  return a < 0 || b < 0 ? [] : rows.slice(Math.min(a, b), Math.max(a, b) + 1).map((row) => row.id);
}
export function toggleId(picked: ReadonlySet<string>, id: string): Set<string> {
  const next = new Set(picked);
  if (!next.delete(id)) next.add(id);
  return next;
}
/** The choice after a refresh: commits that are no longer listed (dropped, rewritten) leave it. The same set when nothing left. */
export function keepListed(picked: ReadonlySet<string>, rows: { id: string }[]): ReadonlySet<string> {
  if (!picked.size) return picked;
  const listed = new Set(rows.map((row) => row.id));
  const kept = [...picked].filter((id) => listed.has(id));
  return kept.length === picked.size ? picked : new Set(kept);
}
/** Chosen commits in the order they are applied: a parent before its child. */
export function oldestFirst(rows: { id: string }[], picked: ReadonlySet<string>): string[] {
  return rows.filter((row) => picked.has(row.id)).map((row) => row.id).reverse();
}

const LANE_X = 18;
const LANE_GAP = 18;
export const ROW_HEIGHT = 28;
export const ROW_MID = ROW_HEIGHT / 2;
const ELBOW_RADIUS = 12;

/**
 * SVG path of one edge inside a single row. Lanes stay vertical; a lane change is a large rounded
 * corner at a commit. An edge leaving the commit goes out horizontally and then down; an edge joining
 * the commit (a branch reaching its parent) is the mirror image: down along its lane, then in.
 */
export function edgePath(edge: { from: number; to: number }, rowLane: number) {
  const fx = LANE_X + edge.from * LANE_GAP;
  const tx = LANE_X + edge.to * LANE_GAP;
  const mid = ROW_MID;
  if (edge.from === edge.to) return `M ${fx} ${edge.from === rowLane ? mid : 0} V ${ROW_HEIGHT}`;
  const dir = Math.sign(tx - fx);
  const r = Math.min(Math.abs(tx - fx), ELBOW_RADIUS);
  if (edge.from === rowLane) return `M ${fx} ${mid} H ${tx - dir * r} Q ${tx} ${mid} ${tx} ${mid + r} V ${ROW_HEIGHT}`;
  return `M ${fx} 0 V ${mid - r} Q ${fx} ${mid} ${fx + dir * r} ${mid} H ${tx}`;
}
