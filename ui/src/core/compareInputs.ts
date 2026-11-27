// Inputs of the browser comparison: a file chosen by the person (an upload, or a file handle that the browser gave) is
// read into bytes for `CompareTransport.compare`. Pure with respect to the browser: it takes anything that looks like a
// `File` or a `FileSystemFileHandle`, so it is unit-tested with plain objects. A file is read only when it is within the
// size the module takes, and every failure is an answer with its reason, never an exception that ends the window.

/** The most bytes of one input in the browser path (the module holds both inputs and the answer in memory). */
export const MAX_BROWSER_INPUT_BYTES = 32 * 1024 * 1024;

/** What `File` (and `Blob`) offers that is used here. */
export interface ReadableFile {
  name: string;
  size: number;
  arrayBuffer(): Promise<ArrayBuffer>;
}
/** What `FileSystemFileHandle` offers that is used here. */
export interface FileHandleLike {
  getFile(): Promise<ReadableFile>;
}
export type BrowserInput = ReadableFile | FileHandleLike;

export type InputResult =
  | { ok: true; name: string; bytes: Uint8Array }
  | { ok: false; name: string; reason: string };

const isHandle = (input: BrowserInput): input is FileHandleLike => typeof (input as FileHandleLike).getFile === "function";

/** Reads one input. The size is looked at before the bytes are read, so a huge file is refused without being loaded. */
export async function readInput(input: BrowserInput, limit = MAX_BROWSER_INPUT_BYTES): Promise<InputResult> {
  let file: ReadableFile;
  try {
    file = isHandle(input) ? await input.getFile() : input;
  } catch (e) {
    return { ok: false, name: "", reason: `the file could not be opened: ${e instanceof Error ? e.message : String(e)}` };
  }
  if (file.size > limit) {
    return { ok: false, name: file.name, reason: `the file is larger than ${Math.floor(limit / (1024 * 1024))} MiB` };
  }
  try {
    const bytes = new Uint8Array(await file.arrayBuffer());
    // The size said before may be wrong for a file that changed meanwhile: the bytes read are what counts.
    if (bytes.byteLength > limit) return { ok: false, name: file.name, reason: `the file is larger than ${Math.floor(limit / (1024 * 1024))} MiB` };
    return { ok: true, name: file.name, bytes };
  } catch (e) {
    return { ok: false, name: file.name, reason: `the file could not be read: ${e instanceof Error ? e.message : String(e)}` };
  }
}

/** Reads both inputs of a comparison (side by side); the first failure is the reason to show. */
export async function readPair(a: BrowserInput, b: BrowserInput, limit = MAX_BROWSER_INPUT_BYTES): Promise<{ ok: true; a: InputResult & { ok: true }; b: InputResult & { ok: true } } | { ok: false; reason: string }> {
  const [left, right] = await Promise.all([readInput(a, limit), readInput(b, limit)]);
  if (!left.ok) return { ok: false, reason: `${left.name || "left"}: ${left.reason}` };
  if (!right.ok) return { ok: false, reason: `${right.name || "right"}: ${right.reason}` };
  return { ok: true, a: left, b: right };
}

/** Text typed or pasted as the bytes of an input (UTF-8, as the module reads text). */
export function textInput(name: string, text: string): InputResult {
  return { ok: true, name, bytes: new TextEncoder().encode(text) };
}
