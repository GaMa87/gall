import { describe, expect, it } from "vitest";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  createInitialProgress,
  formatElapsed,
  formatProgressMetrics,
  mergeProgress,
  normalizeProgress,
  progressAriaLabel,
  type GitProgressEvent,
} from "./gitProgress";
import { GitProgressBar } from "../components/GitProgressBar";

describe("gitProgress core helpers", () => {
  it("keeps indeterminate state honest and never fabricates percentages", () => {
    const initial = createInitialProgress("op-1", "fetch", "Fetching from remote", true);
    expect(initial.fraction).toBeNull();
    expect(initial.percent).toBeNull();

    const event: GitProgressEvent = {
      id: "op-1",
      operation: "fetch",
      phase: "Enumerating objects",
      fraction: null,
      percent: 42, // ignored because total is unknown and fraction is null
      done: 500,
      total: null,
      unit: "objects",
      transferred: null,
      rate: null,
      remote: true,
      phaseIndex: null,
      phaseCount: null,
      detail: null,
      elapsedMs: 320,
      cancellable: true,
      finished: false,
    };
    const merged = mergeProgress(initial, event);
    expect(merged.fraction).toBeNull();
    expect(merged.percent).toBeNull();
    expect(merged.done).toBe(500);
    expect(formatProgressMetrics(merged)).toBe("500 objects");
  });

  it("computes real determinate progress and formats counters, bytes, and elapsed time", () => {
    const ev = normalizeProgress({
      id: "op-2",
      operation: "fetch",
      phase: "Receiving objects",
      fraction: 0.624,
      percent: 62,
      done: 765,
      total: 1234,
      unit: "objects",
      transferred: "12.40 MiB",
      rate: "4.20 MiB/s",
      remote: false,
      phaseIndex: 3,
      phaseCount: 4,
      detail: null,
      elapsedMs: 2450,
      cancellable: true,
      finished: false,
    });
    expect(ev.percent).toBe(62);
    expect(formatProgressMetrics(ev)).toContain("765");
    expect(formatProgressMetrics(ev)).toContain("12.40 MiB · 4.20 MiB/s");
    expect(formatElapsed(450)).toBe("0.5s");
    expect(formatElapsed(2450)).toBe("2.5s");
    expect(formatElapsed(75_000)).toBe("1m 15s");
    expect(progressAriaLabel(ev)).toBe("Receiving objects (62%)");
  });

  it("ignores progress events for a different operation ID", () => {
    const initial = createInitialProgress("op-a", "push", "Pushing to remote", true);
    const other: GitProgressEvent = {
      ...initial,
      id: "op-b",
      phase: "Writing objects",
      fraction: 0.5,
      percent: 50,
    };
    expect(mergeProgress(initial, other)).toBe(initial);
  });
});

describe("GitProgressBar accessible rendering", () => {
  it("renders determinate semantic <progress> with aria-valuenow, percentage, and cancel button", () => {
    const state = normalizeProgress({
      id: "op-det",
      operation: "fetch",
      phase: "Receiving objects",
      fraction: 0.62,
      percent: 62,
      done: 620,
      total: 1000,
      unit: "objects",
      transferred: "4.10 MiB",
      rate: "2.05 MiB/s",
      remote: false,
      phaseIndex: 3,
      phaseCount: 4,
      detail: "origin",
      elapsedMs: 1400,
      cancellable: true,
      finished: false,
    });
    const html = renderToStaticMarkup(
      createElement(GitProgressBar, {
        progress: state,
        onCancel: () => {},
      }),
    );
    expect(html).toContain('role="status"');
    expect(html).toContain('value="62"');
    expect(html).toContain('max="100"');
    expect(html).toContain('aria-valuenow="62"');
    expect(html).toContain("62%");
    expect(html).toContain("4.10 MiB · 2.05 MiB/s");
    expect(html).toContain("1.4s");
    expect(html).toContain("Cancel");
  });

  it("renders indeterminate semantic <progress> without value attribute or fake percentage", () => {
    const state = createInitialProgress("op-indet", "search", "Searching history", true, {
      done: 42,
      total: null,
      unit: "matches",
    });
    state.elapsedMs = 800;
    const html = renderToStaticMarkup(
      createElement(GitProgressBar, {
        progress: state,
        compact: true,
      }),
    );
    expect(html).toContain("<progress");
    expect(html).not.toContain("value=");
    expect(html).not.toContain("aria-valuenow");
    expect(html).not.toContain("%");
    expect(html).toContain("42 matches");
    expect(html).toContain("0.8s");
  });
});
