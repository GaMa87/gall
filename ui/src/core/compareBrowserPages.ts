// The comparison window's page commands (`diffPageOpen`, `diffPageSmall`, `diffPageHunks`, `diffPageClose`) answered by
// the browser worker, for a page that runs outside the desktop app. Only text sources exist there (typed, pasted or
// read from an upload); everything the browser path cannot do is an error with its reason, which the window shows.
import type { CompareHunk, CompareSummary, ComparePage, DiffOptions, DiffResult, DiffSide, DiffSource } from "../api/types";
import { browserTransport, compareInBrowser, currentBrowserEnv, type BrowserEnv } from "./compareBrowser";
import type { CompareOptions, CompareTransport } from "./compareTransport";

type TextResult = NonNullable<DiffResult["text"]>;
interface Held { text: TextResult }

const held = new Map<number, Held>();
let running = false;

/** The lines of a text without their terminators (a final one that ends the text does not start another line). */
export function linesOf(text: string): string[] {
  if (!text) return [];
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  return lines.map((line) => (line.endsWith("\r") ? line.slice(0, -1) : line));
}

function side(source: DiffSource, bytes: Uint8Array, text: string): DiffSide {
  return { label: source.kind === "text" ? source.label : source.kind, path: null, size: bytes.byteLength, crlf: text.includes("\r\n"), finalNewline: text === "" || text.endsWith("\n") };
}

function shape(lines: string[], s: DiffSide) {
  return { lines: lines.length, maxLineColumns: lines.reduce((m, l) => Math.max(m, l.length), 0), crlf: s.crlf, finalNewline: s.finalNewline };
}

function engineOptions(opts: DiffOptions): CompareOptions {
  return { minimal: opts.minimal, ignoreWhitespace: opts.ignoreWhitespace || opts.whitespace === "all", ignoreCase: opts.ignoreCase, movesEdited: opts.movesEdited };
}

/** First differing byte of two inputs, or `null` when they are the same bytes. */
export function firstDifference(a: Uint8Array, b: Uint8Array): number | null {
  const n = Math.min(a.byteLength, b.byteLength);
  for (let i = 0; i < n; i++) if (a[i] !== b[i]) return i;
  return a.byteLength === b.byteLength ? null : n;
}

export async function browserDiffPageOpen(id: number, a: DiffSource, b: DiffSource, opts: DiffOptions, transport: CompareTransport = browserTransport(), env: BrowserEnv = currentBrowserEnv()): Promise<CompareSummary> {
  if (a.kind !== "text" || b.kind !== "text") throw new Error("the browser compares text that is typed, pasted or uploaded, not files of the computer");
  const encoder = new TextEncoder();
  const bytesA = encoder.encode(a.text), bytesB = encoder.encode(b.text);
  const left = side(a, bytesA, a.text), right = side(b, bytesB, b.text);
  // The worker takes the buffers over; the differences of bytes are looked at from copies made before.
  const same = firstDifference(bytesA, bytesB);
  held.delete(id);
  running = true;
  let answer;
  try {
    answer = await compareInBrowser(transport, env, bytesA, bytesB, engineOptions(opts));
  } finally {
    running = false;
  }
  switch (answer.kind) {
    case "text": {
      const leftLines = linesOf(a.text), rightLines = linesOf(b.text);
      const equal = answer.ops.reduce((n, op) => n + (op.tag === "equal" ? op.aLen : 0), 0);
      const total = leftLines.length + rightLines.length;
      const ratio = total === 0 ? 1 : (2 * equal) / total;
      held.set(id, { text: { leftLines, rightLines, ops: answer.ops, pairs: [], added: answer.added, removed: answer.removed, ratio, moves: answer.moves, leftMarks: {}, rightMarks: {} } });
      return { left, right, binary: null, limit: null, text: { left: shape(leftLines, left), right: shape(rightLines, right), onlyCrlfDiffers: answer.identical && left.crlf !== right.crlf, added: answer.added, removed: answer.removed, ratio } };
    }
    case "binary":
      return { left, right, binary: { identical: same === null, firstDifference: same, reason: "binary content" }, limit: null, text: null };
    case "tooLarge":
      return { left, right, binary: null, limit: "tooManyBytes", text: null };
    case "cancelled":
      throw new Error("the comparison was cancelled");
    case "stale":
      throw new Error("a newer comparison replaced this one");
    case "unavailable":
      throw new Error(`the comparison is not available in the browser: ${answer.reason}`);
    default:
      throw new Error(answer.kind === "error" ? answer.message : "unexpected answer of the comparison");
  }
}

export function browserDiffPageSmall(id: number): DiffResult["text"] {
  return held.get(id)?.text ?? null;
}

export function browserDiffPageHunks(id: number, start: number, count: number): ComparePage<CompareHunk> {
  const ops = (held.get(id)?.text.ops ?? []).filter((op) => op.tag !== "equal");
  const items = ops.slice(start, start + count).map((op, i) => ({ ...op, id: `${id}:${start + i}` }));
  const end = start + items.length;
  return { start, next: end < ops.length ? end : null, total: ops.length, items };
}

/** Ends what the window had open up to `through`: held answers are dropped and a running comparison is stopped. */
export function browserDiffPageClose(through: number, transport: CompareTransport = browserTransport()) {
  for (const id of [...held.keys()]) if (id <= through) held.delete(id);
  if (running) transport.cancel();
}
