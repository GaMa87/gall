// The guarded entry of the browser comparison: the desktop keeps IPC, the browser uses the worker through
// `CompareTransport`, and every reason the browser path cannot be used is an `unavailable` answer the window can show.
import { CompareTransport, type CompareAnswer, type CompareOptions, type MergeAnswer, type MergeRequest, type WorkerLike } from "./compareTransport";
import { MAX_BROWSER_INPUT_BYTES } from "./compareInputs";

export interface BrowserEnv { desktop: boolean; hasWorker: boolean; hasWebAssembly: boolean }

/** Why the browser path cannot be used here, or `null` when it can. */
export function browserCompareBlocker(env: BrowserEnv): string | null {
  if (env.desktop) return "the desktop app compares with its own engine";
  if (!env.hasWorker) return "this browser has no Web Workers";
  if (!env.hasWebAssembly) return "this browser has no WebAssembly";
  return null;
}

export function currentBrowserEnv(): BrowserEnv {
  const w = globalThis as { __TAURI_INTERNALS__?: unknown; Worker?: unknown; WebAssembly?: unknown };
  return { desktop: !!w.__TAURI_INTERNALS__, hasWorker: typeof w.Worker === "function", hasWebAssembly: typeof w.WebAssembly === "object" };
}

/** Compares in the browser; refuses inputs over the browser limit and reports the blocker instead of throwing. */
export async function compareInBrowser(transport: CompareTransport, env: BrowserEnv, a: Uint8Array, b: Uint8Array, options: CompareOptions = {}): Promise<CompareAnswer> {
  const blocker = browserCompareBlocker(env);
  if (blocker) return { kind: "unavailable", reason: blocker };
  const size = Math.max(a.byteLength, b.byteLength);
  if (size > MAX_BROWSER_INPUT_BYTES) return { kind: "tooLarge", reason: `an input of ${size} bytes is over the browser limit of ${MAX_BROWSER_INPUT_BYTES} bytes` };
  return transport.compare(a, b, options);
}

/** Merges in the browser with the same guards as `compareInBrowser`; a missing stage is an empty input named in `missing`. */
export async function mergeInBrowser(transport: CompareTransport, env: BrowserEnv, base: Uint8Array | null, left: Uint8Array, right: Uint8Array, request: MergeRequest = {}): Promise<MergeAnswer> {
  const blocker = browserCompareBlocker(env);
  if (blocker) return { kind: "unavailable", reason: blocker };
  const size = Math.max(base?.byteLength ?? 0, left.byteLength, right.byteLength);
  if (size > MAX_BROWSER_INPUT_BYTES) return { kind: "tooLarge", reason: `an input of ${size} bytes is over the browser limit of ${MAX_BROWSER_INPUT_BYTES} bytes` };
  const missing = [...(request.missing ?? [])];
  if (!base && !missing.includes("base")) missing.push("base");
  return transport.merge(base ?? new Uint8Array(0), left, right, { ...request, missing });
}

let shared: CompareTransport | null = null;

/** The one worker of the page, created on the first comparison. */
export function browserTransport(): CompareTransport {
  return (shared ??= new CompareTransport(() => new Worker(new URL("./compare.worker.ts", import.meta.url), { type: "module" }) as unknown as WorkerLike));
}
