import { describe, expect, it } from "vitest";
import { effectiveHidden, hiddenDivergence, planApplyHiddenAll, planToggleHiddenAll, planToggleHiddenBoard } from "./hiddenScope";

describe("hiddenScope self-healing sync", () => {
  it("resolves effective visibility from override or global default", () => {
    expect(effectiveHidden(false, null)).toBe(false);
    expect(effectiveHidden(false, undefined)).toBe(false);
    expect(effectiveHidden(false, true)).toBe(true);
    expect(effectiveHidden(true, null)).toBe(true);
    expect(effectiveHidden(true, false)).toBe(false);
  });

  it("toggles a single board when both boards are shown and synced", () => {
    // Global OFF -> toggle left: only left turns ON, global stays OFF.
    const leftOn = planToggleHiddenBoard(false, [null, null], 0, false);
    expect(leftOn).toEqual({
      nextGlobal: false,
      nextOverrides: [true, null],
      globalChanged: false,
      sessionOp: { b: 0, on: true },
    });

    // Global ON -> toggle right: only right turns OFF, global stays ON.
    const rightOff = planToggleHiddenBoard(true, [null, null], 1, false);
    expect(rightOff).toEqual({
      nextGlobal: true,
      nextOverrides: [null, false],
      globalChanged: false,
      sessionOp: { b: 1, on: false },
    });
  });

  it("self-heals back to global sync when both boards match again", () => {
    // Left was ON (global OFF); toggling left back OFF clears overrides.
    const backOff = planToggleHiddenBoard(false, [true, null], 0, false);
    expect(backOff).toEqual({
      nextGlobal: false,
      nextOverrides: [null, null],
      globalChanged: false,
      sessionOp: { b: null, on: null },
    });

    // Left was ON (global OFF); toggling right ON promotes global to ON and clears overrides.
    const bothOn = planToggleHiddenBoard(false, [true, null], 1, false);
    expect(bothOn).toEqual({
      nextGlobal: true,
      nextOverrides: [null, null],
      globalChanged: true,
      sessionOp: { b: null, on: null },
    });

    // Right was OFF (global ON); toggling left OFF promotes global to OFF and clears overrides.
    const bothOff = planToggleHiddenBoard(true, [null, false], 0, false);
    expect(bothOff).toEqual({
      nextGlobal: false,
      nextOverrides: [null, null],
      globalChanged: true,
      sessionOp: { b: null, on: null },
    });
  });

  it("updates global directly and clears overrides in single-board view", () => {
    expect(planToggleHiddenBoard(false, [null, null], 0, true)).toEqual({
      nextGlobal: true,
      nextOverrides: [null, null],
      globalChanged: true,
      sessionOp: null,
    });
    expect(planToggleHiddenBoard(false, [true, null], 0, true)).toEqual({
      nextGlobal: false,
      nextOverrides: [null, null],
      globalChanged: false,
      sessionOp: { b: null, on: null },
    });
  });

  it("global toggle and apply-all clear overrides and sync both boards", () => {
    // Global OFF with left ON: toggleAll brings both ON (complementary to single-board toggle on left).
    expect(planToggleHiddenAll(false, [true, null])).toEqual({
      nextGlobal: true,
      nextOverrides: [null, null],
      globalChanged: true,
      sessionOp: { b: null, on: null },
    });
    // Global ON with right OFF: toggleAll brings both OFF.
    expect(planToggleHiddenAll(true, [null, false])).toEqual({
      nextGlobal: false,
      nextOverrides: [null, null],
      globalChanged: true,
      sessionOp: { b: null, on: null },
    });
    // Explicit applyHiddenAll(true) when global is already true clears right=false override.
    expect(planApplyHiddenAll(true, [null, false], true)).toEqual({
      nextGlobal: true,
      nextOverrides: [null, null],
      globalChanged: false,
      sessionOp: { b: null, on: null },
    });
  });

  it("reports divergence only on the overriding board in dual-board mode", () => {
    expect(hiddenDivergence(false, [null, null], 0, false)).toBeNull();
    expect(hiddenDivergence(false, [null, null], 1, false)).toBeNull();
    expect(hiddenDivergence(true, [null, null], 0, false)).toBeNull();

    // Left diverges ON while global is OFF (returns primitive for useSyncExternalStore stability).
    expect(hiddenDivergence(false, [true, null], 0, false)).toBe(true);
    expect(hiddenDivergence(false, [true, null], 1, false)).toBeNull();

    // Right diverges OFF while global is ON.
    expect(hiddenDivergence(true, [null, false], 0, false)).toBeNull();
    expect(hiddenDivergence(true, [null, false], 1, false)).toBe(false);

    // Repeated calls return an identical value under Object.is.
    expect(Object.is(hiddenDivergence(false, [true, null], 0, false), hiddenDivergence(false, [true, null], 0, false))).toBe(true);

    // Single-board view never shows a divergence badge.
    expect(hiddenDivergence(false, [true, null], 0, true)).toBeNull();

    // Corrupted state where both overrides agree shows no divergence.
    expect(hiddenDivergence(false, [true, true], 0, false)).toBeNull();
  });
});
