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
    const [id, parents, refs, subject, author, date] = line.split("\0");
    return { id, parents: parents ? parents.split(" ") : [], refs: refs ? refs.split(", ") : [], subject, author, date };
  });
  type Track = { id: string; color: number; start: number; end: number; col: number; host?: Track };
  type Edge = { from: number; to: number; color: number };
  const tracks: Track[] = [];
  let nextColor = 0;
  const newTrack = (id: string, start: number, host?: Track): Track => { const track = { id, color: nextColor++, start, end: Infinity, col: -1, host }; tracks.push(track); return track; };

  // Pass 1: follow the lanes without columns and record where each one starts and ends.
  let waiting: Track[] = [];
  const steps = commits.map((commit, row) => {
    const mine = waiting.filter((track) => track.id === commit.id);
    const tip = mine.length === 0;
    const own = tip ? newTrack(commit.id, row) : mine[0];
    if (tip) waiting.push(own);
    const joining = mine.slice(1);
    joining.forEach((track) => { track.end = row; });
    waiting = waiting.filter((track) => !joining.includes(track));
    const targets = commit.parents.map((parent, order) => {
      if (order === 0) { own.id = parent; return own; }
      const existing = waiting.find((track) => track.id === parent);
      if (existing) return existing;
      const created = newTrack(parent, row, own);
      waiting.push(created);
      return created;
    });
    if (!commit.parents.length) { own.end = row; waiting = waiting.filter((track) => track !== own); }
    return { own, tip, joining, targets };
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
    const { own, tip, joining, targets } = steps[row];
    (byStart.get(row) ?? []).forEach((track) => live.add(track));
    const passing = [...live].filter((track) => track !== own && track.start < row && track.end > row);
    const edges: Edge[] = [
      ...passing.map((track) => ({ from: track.col, to: track.col, color: track.color })),
      ...joining.map((track) => ({ from: track.col, to: own.col, color: track.color })),
      ...targets.map((track) => ({ from: own.col, to: track.col, color: track.color })),
    ].sort((a, b) => a.from - b.from);
    const width = Math.max(1, ...[...live].map((track) => track.col + 1));
    live.forEach((track) => { if (track.end <= row) live.delete(track); });
    return { ...commit, lane: own.col, color: own.color, edges, tip, width };
  });
}

const LANE_X = 18;
const LANE_GAP = 18;
const ROW_HEIGHT = 36;
const ELBOW_RADIUS = 16;

/**
 * SVG path of one edge inside a single row. Lanes stay vertical; a lane change is a large rounded
 * corner at a commit. An edge leaving the commit goes out horizontally and then down; an edge joining
 * the commit (a branch reaching its parent) is the mirror image: down along its lane, then in.
 */
export function edgePath(edge: { from: number; to: number }, rowLane: number) {
  const fx = LANE_X + edge.from * LANE_GAP;
  const tx = LANE_X + edge.to * LANE_GAP;
  const mid = ROW_HEIGHT / 2;
  if (edge.from === edge.to) return `M ${fx} ${edge.from === rowLane ? mid : 0} V ${ROW_HEIGHT}`;
  const dir = Math.sign(tx - fx);
  const r = Math.min(Math.abs(tx - fx), ELBOW_RADIUS);
  if (edge.from === rowLane) return `M ${fx} ${mid} H ${tx - dir * r} Q ${tx} ${mid} ${tx} ${mid + r} V ${ROW_HEIGHT}`;
  return `M ${fx} 0 V ${mid - r} Q ${fx} ${mid} ${fx + dir * r} ${mid} H ${tx}`;
}
