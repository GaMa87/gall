import { describe, expect, it, vi } from "vitest";
import { browserCompareBlocker, compareInBrowser, mergeInBrowser } from "./compareBrowser";
import type { CompareTransport } from "./compareTransport";

const web = { desktop: false, hasWorker: true, hasWebAssembly: true };

describe("browser comparison guard", () => {
  it("names why the browser path is unavailable", () => {
    expect(browserCompareBlocker(web)).toBeNull();
    expect(browserCompareBlocker({ ...web, desktop: true })).toMatch(/desktop/);
    expect(browserCompareBlocker({ ...web, hasWorker: false })).toMatch(/Workers/);
    expect(browserCompareBlocker({ ...web, hasWebAssembly: false })).toMatch(/WebAssembly/);
  });

  it("keeps the desktop on IPC and never touches the worker there", async () => {
    const transport = { compare: vi.fn() } as unknown as CompareTransport;
    const answer = await compareInBrowser(transport, { ...web, desktop: true }, new Uint8Array(1), new Uint8Array(1));
    expect(answer.kind).toBe("unavailable");
    expect(transport.compare).not.toHaveBeenCalled();
  });

  it("refuses an input over the browser limit before the worker starts", async () => {
    const transport = { compare: vi.fn() } as unknown as CompareTransport;
    const big = { byteLength: 33 * 1024 * 1024 } as Uint8Array;
    expect((await compareInBrowser(transport, web, big, new Uint8Array(1))).kind).toBe("tooLarge");
    expect(transport.compare).not.toHaveBeenCalled();
  });

  it("passes small inputs to the transport", async () => {
    const transport = { compare: vi.fn().mockResolvedValue({ kind: "binary" }) } as unknown as CompareTransport;
    expect(await compareInBrowser(transport, web, new Uint8Array(2), new Uint8Array(2))).toEqual({ kind: "binary" });
  });
});

describe("browser merge guard", () => {
  it("passes the stages and names a missing base", async () => {
    const merge = vi.fn().mockResolvedValue({ kind: "text" });
    const transport = { merge } as unknown as CompareTransport;
    expect(await mergeInBrowser(transport, web, null, new Uint8Array(1), new Uint8Array(1))).toEqual({ kind: "text" });
    expect(merge.mock.calls[0][3]).toEqual({ missing: ["base"] });
  });

  it("keeps the desktop on IPC and refuses an input over the limit", async () => {
    const merge = vi.fn();
    const transport = { merge } as unknown as CompareTransport;
    expect((await mergeInBrowser(transport, { ...web, desktop: true }, null, new Uint8Array(1), new Uint8Array(1))).kind).toBe("unavailable");
    const big = { byteLength: 33 * 1024 * 1024 } as Uint8Array;
    expect((await mergeInBrowser(transport, web, new Uint8Array(1), big, new Uint8Array(1))).kind).toBe("tooLarge");
    expect(merge).not.toHaveBeenCalled();
  });
});
