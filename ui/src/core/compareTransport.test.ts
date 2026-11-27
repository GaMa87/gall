import { describe, expect, it, vi } from "vitest";
import { CompareTransport, type WorkerLike, type WorkerRequest } from "./compareTransport";

/** A worker that answers when told to, so the order of answers can be chosen. */
class FakeWorker implements WorkerLike {
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onerror: ((event: { message?: string }) => void) | null = null;
  terminated = false;
  requests: WorkerRequest[] = [];
  transfers: Transferable[][] = [];
  constructor(private readonly start: "ready" | "failed" | "silent" = "ready") {}
  postMessage(message: unknown, transfer: Transferable[] = []) {
    const m = message as { type: string };
    if (m.type === "probe") {
      if (this.start === "ready") queueMicrotask(() => this.onmessage?.({ data: { type: "ready" } }));
      if (this.start === "failed") queueMicrotask(() => this.onmessage?.({ data: { type: "failed", reason: "module missing" } }));
    } else {
      this.requests.push(message as WorkerRequest);
      this.transfers.push(transfer);
    }
  }
  terminate() { this.terminated = true; }
  reply(id: number, json: string) { this.onmessage?.({ data: { type: "answer", id, json } }); }
}

const bytes = (s: string) => new TextEncoder().encode(s);
const text = JSON.stringify({ kind: "text", ops: [], moves: [], added: 0, removed: 0, identical: true });

describe("compare transport", () => {
  it("probes the worker and reports the reason when it is not usable", async () => {
    expect(await new CompareTransport(() => new FakeWorker("ready")).probe()).toBeNull();
    expect(await new CompareTransport(() => new FakeWorker("failed")).probe()).toBe("module missing");
    expect(await new CompareTransport(() => { throw new Error("no workers here"); }).probe()).toContain("no workers here");
    const t = new CompareTransport(() => new FakeWorker("failed"));
    expect(await t.compare(bytes("a"), bytes("b"))).toMatchObject({ kind: "unavailable", reason: "module missing" });
  });

  it("gives up on a worker that does not start in time", async () => {
    vi.useFakeTimers();
    try {
      const worker = new FakeWorker("silent");
      const t = new CompareTransport(() => worker, 100);
      const probe = t.probe();
      await vi.advanceTimersByTimeAsync(101);
      expect(await probe).toBe("the worker did not start in time");
      expect(worker.terminated).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it("a cancel during the start answers cancelled at once and the old timer cannot kill the next worker", async () => {
    vi.useFakeTimers();
    try {
      const workers: FakeWorker[] = [];
      const t = new CompareTransport(() => { const w = new FakeWorker(workers.length === 0 ? "silent" : "ready"); workers.push(w); return w; }, 100);
      const first = t.compare(bytes("a"), bytes("b"));
      await Promise.resolve();
      t.cancel();
      expect(await first).toEqual({ kind: "cancelled" });
      const second = t.compare(bytes("c"), bytes("d"));
      await vi.advanceTimersByTimeAsync(200);
      expect(workers[1].terminated).toBe(false);
      workers[1].reply(workers[1].requests[0].id, text);
      expect(await second).toMatchObject({ kind: "text" });
    } finally {
      vi.useRealTimers();
    }
  });

  it("sends the right bytes of a view into a larger buffer and of one buffer given twice", async () => {
    const worker = new FakeWorker();
    const t = new CompareTransport(() => worker);
    const big = bytes("xxabcxx");
    const view = big.subarray(2, 5);
    const pending = t.compare(view, view);
    await t.probe();
    await Promise.resolve();
    const request = worker.requests[0];
    expect(new TextDecoder().decode(new Uint8Array(request.a))).toBe("abc");
    expect(new TextDecoder().decode(new Uint8Array(request.b))).toBe("abc");
    expect(request.a).not.toBe(request.b);
    worker.reply(request.id, text);
    await pending;
  });

  it("transfers the buffers and returns the parsed answer", async () => {
    const worker = new FakeWorker();
    const t = new CompareTransport(() => worker);
    const a = bytes("one"), b = bytes("two");
    const result = t.compare(a, b, { minimal: true });
    await t.probe();
    await Promise.resolve();
    expect(worker.requests).toHaveLength(1);
    expect(worker.transfers[0]).toHaveLength(2);
    expect(JSON.parse(worker.requests[0].options)).toEqual({ minimal: true });
    worker.reply(worker.requests[0].id, text);
    expect(await result).toMatchObject({ kind: "text", identical: true });
  });

  it("drops the answer of a request that is no longer the latest", async () => {
    const worker = new FakeWorker();
    const t = new CompareTransport(() => worker);
    const first = t.compare(bytes("a"), bytes("b"));
    const second = t.compare(bytes("c"), bytes("d"));
    expect(await first).toEqual({ kind: "stale" });
    await t.probe();
    await Promise.resolve();
    // The first request never reached the worker; even a late answer for its id changes nothing.
    worker.reply(1, text);
    worker.reply(worker.requests[worker.requests.length - 1].id, text);
    expect(await second).toMatchObject({ kind: "text" });
  });

  it("cancel ends the worker, resolves the request and starts a new worker for the next one", async () => {
    const workers: FakeWorker[] = [];
    const t = new CompareTransport(() => { const w = new FakeWorker(); workers.push(w); return w; });
    const running = t.compare(bytes("a"), bytes("b"));
    await t.probe();
    await Promise.resolve();
    t.cancel();
    expect(await running).toEqual({ kind: "cancelled" });
    expect(workers[0].terminated).toBe(true);
    const next = t.compare(bytes("c"), bytes("d"));
    await t.probe();
    await Promise.resolve();
    expect(workers).toHaveLength(2);
    workers[1].reply(workers[1].requests[0].id, text);
    expect(await next).toMatchObject({ kind: "text" });
  });

  it("an unreadable answer and a crashing worker become errors", async () => {
    const worker = new FakeWorker();
    const t = new CompareTransport(() => worker);
    const bad = t.compare(bytes("a"), bytes("b"));
    await t.probe();
    await Promise.resolve();
    worker.reply(worker.requests[0].id, "{not json");
    expect(await bad).toMatchObject({ kind: "error" });
    const crash = t.compare(bytes("a"), bytes("b"));
    await t.probe();
    await Promise.resolve();
    worker.onerror?.({ message: "out of memory" });
    expect(await crash).toMatchObject({ kind: "error", message: expect.stringContaining("out of memory") });
  });

  it("sends a merge with its three stages and cancels it like a comparison", async () => {
    const workers: FakeWorker[] = [];
    const t = new CompareTransport(() => { const w = new FakeWorker(); workers.push(w); return w; });
    const merged = t.merge(bytes("base"), bytes("left"), bytes("right"), { choices: { c1: "left" } });
    await t.probe();
    await Promise.resolve();
    const request = workers[0].requests[0];
    expect(request.type).toBe("merge");
    expect(new TextDecoder().decode(new Uint8Array(request.base!))).toBe("base");
    expect(workers[0].transfers[0]).toHaveLength(3);
    expect(JSON.parse(request.options)).toEqual({ choices: { c1: "left" } });
    workers[0].reply(request.id, JSON.stringify({ kind: "resolved", text: "left", provenance: [] }));
    expect(await merged).toMatchObject({ kind: "resolved", text: "left" });
    const running = t.merge(bytes("b"), bytes("l"), bytes("r"));
    await t.probe();
    await Promise.resolve();
    t.cancel();
    expect(await running).toEqual({ kind: "cancelled" });
    expect(workers[0].terminated).toBe(true);
  });
});
