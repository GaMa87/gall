// The page's side of the browser comparison worker: one request at a time counts (older answers are dropped by
// generation), the two inputs travel as transferable buffers, the start is probed with a time limit, and a
// cancel ends the worker (a running WebAssembly call cannot be interrupted) and starts a fresh one on demand.
//
// Nothing here needs the network: the worker loads the module from the installed assets (see compare.worker.ts).

export interface CompareOptions { minimal?: boolean; ignoreWhitespace?: boolean; ignoreCase?: boolean; movesEdited?: boolean }

export interface CompareRange { tag: "equal" | "delete" | "insert" | "replace"; a: number; aLen: number; b: number; bLen: number }
export interface CompareMove { id: number; a: number; b: number; len: number; bLen: number; edited: boolean }

/** What the worker answers, as the core's `adapter::Answer`; `stale` and `unavailable` are the page's own. */
export type CompareAnswer =
  | { kind: "text"; ops: CompareRange[]; moves: CompareMove[]; added: number; removed: number; identical: boolean }
  | { kind: "binary" }
  | { kind: "tooLarge"; reason: string }
  | { kind: "cancelled" }
  | { kind: "error"; message: string }
  | { kind: "stale" }
  | { kind: "unavailable"; reason: string };

/** The part of a Web Worker that the transport uses (a fake one runs in the tests). */
export interface WorkerLike {
  postMessage(message: unknown, transfer?: Transferable[]): void;
  terminate(): void;
  onmessage: ((event: { data: unknown }) => void) | null;
  onerror: ((event: { message?: string }) => void) | null;
}

/** `compare` takes `a` and `b`; `merge` takes the three stages in `base`, `a` (left) and `b` (right). */
export interface WorkerRequest { type: "compare" | "merge"; id: number; a: ArrayBuffer; b: ArrayBuffer; base?: ArrayBuffer; options: string }

/** A merge answer is the core's `merge::Outcome` or `merge::Resolution` (told apart by `kind`), or one of the page's own. */
export type MergeAnswer = { kind: string; [field: string]: unknown } | { kind: "stale" } | { kind: "unavailable"; reason: string };

export interface MergeRequest {
  /** Conflict ID to `left | right | both | combine | base`: asks for the resolved result instead of the chunks. */
  choices?: Record<string, string>;
  /** Stages that do not exist (a file added on both sides has no `base`). */
  missing?: ("base" | "left" | "right")[];
}
export interface WorkerProbe { type: "probe" }
export type WorkerReply = { type: "ready" } | { type: "failed"; reason: string } | { type: "answer"; id: number; json: string };

/** The start of the worker is given this long (ms) before the transport reports the browser path unavailable. */
export const STARTUP_LIMIT_MS = 5000;

/** The start was ended by a cancel or a dispose, not by a failure. */
const STOPPED = "the worker was stopped";

/** The bytes of a view as a buffer of their own (a view into a larger buffer is copied, so the right bytes travel). */
function ownBuffer(view: Uint8Array): ArrayBuffer {
  return view.byteOffset === 0 && view.byteLength === view.buffer.byteLength ? (view.buffer as ArrayBuffer) : (view.slice().buffer as ArrayBuffer);
}

interface Pending { id: number; resolve: (a: CompareAnswer | MergeAnswer) => void }

export class CompareTransport {
  private worker: WorkerLike | null = null;
  private generation = 0;
  /** Requests up to this number were ended by a cancel (not replaced by a newer request). */
  private cancelledUpTo = 0;
  private pending = new Map<number, Pending>();
  private ready: Promise<string | null> | null = null;
  private startTimer: ReturnType<typeof setTimeout> | null = null;
  private settleStart: ((reason: string | null) => void) | null = null;

  constructor(private readonly make: () => WorkerLike, private readonly startupMs = STARTUP_LIMIT_MS) {}

  /** Starts the worker if needed. Resolves with `null` when it answered the probe, else with the reason it cannot be used. */
  probe(): Promise<string | null> {
    if (this.ready) return this.ready;
    this.ready = new Promise<string | null>((resolve) => {
      let worker: WorkerLike;
      try {
        worker = this.make();
      } catch (e) {
        // Not remembered: the next request tries again.
        this.ready = null;
        resolve(`the worker could not be started: ${String(e)}`);
        return;
      }
      this.worker = worker;
      let settled = false;
      const finish = (reason: string | null) => {
        if (settled) return;
        settled = true;
        if (this.startTimer) clearTimeout(this.startTimer);
        this.startTimer = null;
        this.settleStart = null;
        if (reason && reason !== STOPPED) this.stop();
        resolve(reason);
      };
      this.settleStart = finish;
      this.startTimer = setTimeout(() => finish("the worker did not start in time"), this.startupMs);
      worker.onerror = (event) => finish(`the worker failed: ${event.message ?? "unknown error"}`);
      worker.onmessage = (event) => {
        const reply = event.data as WorkerReply;
        if (reply.type === "ready") {
          worker.onmessage = (e) => this.answer(e.data as WorkerReply);
          worker.onerror = (e) => this.fail(`the worker failed: ${e.message ?? "unknown error"}`);
          finish(null);
        } else if (reply.type === "failed") finish(reply.reason);
      };
      worker.postMessage({ type: "probe" } satisfies WorkerProbe);
    });
    return this.ready;
  }

  /**
   * Compares two inputs. The buffers are handed over (they are transferred, not copied): do not use them afterwards.
   * Only the latest request is answered; an earlier one that is still running gets `stale`.
   */
  compare(a: Uint8Array, b: Uint8Array, options: CompareOptions = {}): Promise<CompareAnswer> {
    return this.send("compare", undefined, a, b, JSON.stringify(options)) as Promise<CompareAnswer>;
  }

  /** Merges `left` and `right` against `base`, with the same hand-over and latest-only rules as `compare`. */
  merge(base: Uint8Array, left: Uint8Array, right: Uint8Array, request: MergeRequest = {}): Promise<MergeAnswer> {
    return this.send("merge", base, left, right, JSON.stringify(request)) as Promise<MergeAnswer>;
  }

  private async send(type: WorkerRequest["type"], base: Uint8Array | undefined, a: Uint8Array, b: Uint8Array, options: string): Promise<CompareAnswer | MergeAnswer> {
    const id = ++this.generation;
    for (const old of this.pending.values()) old.resolve({ kind: "stale" });
    this.pending.clear();
    const reason = await this.probe();
    if (reason === STOPPED) return id <= this.cancelledUpTo ? { kind: "cancelled" } : { kind: "stale" };
    if (reason) return { kind: "unavailable", reason };
    if (id !== this.generation) return id <= this.cancelledUpTo ? { kind: "cancelled" } : { kind: "stale" };
    return new Promise<CompareAnswer | MergeAnswer>((resolve) => {
      this.pending.set(id, { id, resolve });
      // The same buffer twice cannot be transferred twice.
      const taken = new Set<ArrayBuffer>();
      const own = (view: Uint8Array) => {
        let buffer = ownBuffer(view);
        if (taken.has(buffer)) buffer = buffer.slice(0);
        taken.add(buffer);
        return buffer;
      };
      const request: WorkerRequest = { type, id, a: own(a), b: own(b), options };
      if (base) request.base = own(base);
      this.worker!.postMessage(request, [request.a, request.b, ...(request.base ? [request.base] : [])]);
    });
  }

  /** Ends the running comparison: the worker is terminated (and started again by the next request). */
  cancel() {
    for (const p of this.pending.values()) p.resolve({ kind: "cancelled" });
    this.pending.clear();
    this.cancelledUpTo = this.generation;
    this.generation++;
    this.stop();
  }

  dispose() { this.cancel(); }

  private answer(reply: WorkerReply) {
    if (reply.type !== "answer") return;
    const p = this.pending.get(reply.id);
    // An answer to a request that is not the latest any more is dropped.
    if (!p || reply.id !== this.generation) return;
    this.pending.delete(reply.id);
    try {
      p.resolve(JSON.parse(reply.json) as CompareAnswer | MergeAnswer);
    } catch (e) {
      p.resolve({ kind: "error", message: `unreadable answer: ${String(e)}` });
    }
  }

  private fail(reason: string) {
    for (const p of this.pending.values()) p.resolve({ kind: "error", message: reason });
    this.pending.clear();
    this.stop();
  }

  private stop() {
    if (this.startTimer) clearTimeout(this.startTimer);
    this.startTimer = null;
    // A start that is still waiting ends with the worker: nothing is left to wait for.
    const settle = this.settleStart;
    this.settleStart = null;
    settle?.(STOPPED);
    this.worker?.terminate();
    this.worker = null;
    this.ready = null;
  }
}
