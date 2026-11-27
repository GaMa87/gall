import { expect, it } from "vitest";
import stored from "../gitBaseline.json";
import { contextMenuPoint, overBaseline, stepsText, type Baseline } from "./gitBaseline";

it("fails a scenario that needs more clicks or keys, but not one that only types more", () => {
  const limit = { keys: 20, typed: 15, clicks: 1 };
  expect(overBaseline(limit, limit)).toBe(false);
  expect(overBaseline({ keys: 30, typed: 25, clicks: 1 }, limit)).toBe(false);
  expect(overBaseline({ keys: 21, typed: 15, clicks: 1 }, limit)).toBe(true);
  expect(overBaseline({ keys: 5, typed: 0, clicks: 2 }, limit)).toBe(true);
  // Fewer typed characters must not hide additional command keys.
  expect(overBaseline({ keys: 16, typed: 10, clicks: 1 }, limit)).toBe(true);
});
it("describes steps and right-click positions", () => {
  expect(stepsText({ keys: 3, typed: 1, clicks: 2 })).toBe("3 keys (1 typed), 2 clicks");
  expect(contextMenuPoint({ left: 10, top: 20, width: 400, height: 30 })).toEqual({ clientX: 70, clientY: 35 });
});
it("stores both routes of every benchmark scenario", () => {
  const baseline = stored as Baseline;
  for (const id of ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"]) for (const route of ["k", "m"]) {
    const steps = baseline.steps[`${id}${route}`];
    expect(steps, `${id}${route}`).toBeDefined();
    expect(steps.keys).toBeGreaterThanOrEqual(steps.typed);
  }
});