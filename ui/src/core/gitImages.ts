/** One side of an image comparison (see gitimages.rs). */
export type ImageSide = { kind: "commit"; id: string; file: string } | { kind: "index" | "head" | "work"; file: string } | { kind: "none" };
export type GitImage = { path: string | null; size: number; reason: string | null };

/** The two versions of a file changed in a commit; `base` is null for the first commit. */
export function commitSides(file: { path: string; original: string | null; status: string }, commit: string, base: string | null): [ImageSide, ImageSide] {
  const added = file.status.startsWith("A") || !base, deleted = file.status.startsWith("D");
  return [added ? { kind: "none" } : { kind: "commit", id: base!, file: file.original ?? file.path }, deleted ? { kind: "none" } : { kind: "commit", id: commit, file: file.path }];
}

/** The two versions of a file in Working Changes: HEAD and the index, or the index and the file on disk. */
export function changeSides(change: { path: string; original?: string | null; index: string; worktree: string; staged: boolean }): [ImageSide, ImageSide] {
  if (change.staged) return [change.index === "A" ? { kind: "none" } : { kind: "head", file: change.original ?? change.path }, change.index === "D" ? { kind: "none" } : { kind: "index", file: change.path }];
  return [change.index === "?" ? { kind: "none" } : { kind: "index", file: change.path }, change.worktree === "D" ? { kind: "none" } : { kind: "work", file: change.path }];
}
