// What is typed into the search field of the Git workspace: a filter of the loaded commits, or a search of the whole history.

/** Filled fields must all match (see `CommitQuery` in gitsearch.rs). */
export type CommitQuery = { author: string; message: string; path: string; content: string; hash: string };
export type ParsedSearch =
  /** Nothing typed. */
  | { kind: "none" }
  /** Plain text: filters the loaded commits first; `query` is what the whole history is asked when none of them matches. */
  | { kind: "text"; text: string; query: CommitQuery }
  /** A prefix: the whole history, at once. */
  | { kind: "history"; query: CommitQuery };
const prefixes: Record<string, keyof CommitQuery> = { a: "author", g: "message", f: "path", s: "content" };
export const emptyQuery = (): CommitQuery => ({ author: "", message: "", path: "", content: "", hash: "" });
const isHash = (text: string) => /^[0-9a-f]{4,64}$/i.test(text);

/** `a:` author, `f:` file, `s:` content, `g:` message; a prefix starts a field that ends at the next prefix. */
export function parseSearch(input: string): ParsedSearch {
  const text = input.trim();
  if (!text) return { kind: "none" };
  const query = emptyQuery();
  const parts = text.split(/(?:^|\s+)([agfs]):/);
  // split keeps the letters: ["before", "a", "value", "f", "value", …]
  if (parts.length === 1) return { kind: "text", text, query: { ...query, message: text, hash: isHash(text) ? text : "" } };
  const before = parts[0].trim();
  if (before) { if (isHash(before)) query.hash = before; else query.message = before; }
  for (let i = 1; i < parts.length; i += 2) {
    const field = prefixes[parts[i]], value = parts[i + 1].trim();
    if (value) query[field] = query[field] ? `${query[field]} ${value}` : value;
  }
  // Only prefixes so far ("a:"): nothing to look for yet.
  return Object.values(query).some(Boolean) ? { kind: "history", query } : { kind: "none" };
}
/** Whether the whole history is asked: for a prefix always, for plain text when no loaded commit matches. */
export function historyQuery(parsed: ParsedSearch, loadedMatches: number): CommitQuery | null {
  if (parsed.kind === "history") return parsed.query;
  return parsed.kind === "text" && loadedMatches === 0 && parsed.text.length >= 2 ? parsed.query : null;
}
/** Rows of found commits: the history format without parents, so each is a dot of its own and no line leads to a commit that is not shown. */
export function hitHistory(lines: string): string {
  return lines.split("\n").filter(Boolean).map((line) => { const fields = line.split("\0"); fields[1] = ""; return fields.join("\0"); }).join("\n");
}
/** Text to mark in the subjects and in the authors of the shown commits. */
export function searchMarks(parsed: ParsedSearch): { subject: string; author: string } {
  if (parsed.kind === "text") return { subject: parsed.text, author: parsed.text };
  return parsed.kind === "history" ? { subject: parsed.query.message, author: parsed.query.author } : { subject: "", author: "" };
}
/** `text` in pieces, every second one a match of `needle` (case ignored). */
export function splitMatches(text: string, needle: string): string[] {
  if (!needle) return [text];
  const lower = text.toLowerCase(), search = needle.toLowerCase(), pieces: string[] = [];
  // Lower-casing can change the length of a text (İ); then nothing is marked rather than the wrong place.
  if (lower.length !== text.length) return [text];
  let from = 0;
  for (let at = lower.indexOf(search); at >= 0; at = lower.indexOf(search, from)) { pieces.push(text.slice(from, at), text.slice(at, at + search.length)); from = at + search.length; }
  pieces.push(text.slice(from));
  return pieces;
}
/** The row `n` (1) or `N` (-1) goes to, around the end. */
export function nextHit(current: number, count: number, direction: 1 | -1): number {
  return count === 0 ? -1 : current < 0 ? (direction > 0 ? 0 : count - 1) : (current + direction + count) % count;
}
