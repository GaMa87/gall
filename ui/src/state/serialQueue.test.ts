import { describe, expect, it } from "vitest";
import { createSerialQueue } from "./serialQueue";

describe("createSerialQueue", () => {
  it("runs operations in submission order and continues after a rejection", async () => {
    const enqueue = createSerialQueue();
    const events: string[] = [];
    let releaseFirst!: () => void;
    const first = enqueue(async () => {
      events.push("first-start");
      await new Promise<void>((resolve) => { releaseFirst = resolve; });
      events.push("first-end");
    });
    const failed = enqueue(async () => {
      events.push("second");
      throw new Error("expected");
    });
    const third = enqueue(async () => { events.push("third"); });

    await Promise.resolve();
    expect(events).toEqual(["first-start"]);
    releaseFirst();
    await first;
    await expect(failed).rejects.toThrow("expected");
    await third;
    expect(events).toEqual(["first-start", "first-end", "second", "third"]);
  });
});
