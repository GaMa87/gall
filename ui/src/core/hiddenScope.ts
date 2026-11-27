// Self-healing per-board + global hidden-files state logic.
// Each browser carries an optional override (`null` = synced with global `settings.showHidden`).
// Whenever both browsers end up in the same state, overrides clear and global updates so
// there are never stale per-board overrides fighting the global default.

export type HiddenOverrides = readonly [boolean | null, boolean | null];

export interface HiddenTogglePlan {
  /** Updated global `settings.showHidden` value. */
  nextGlobal: boolean;
  /** Resulting per-board overrides (`null` = synced with `nextGlobal`). */
  nextOverrides: [boolean | null, boolean | null];
  /** True when global `settings.showHidden` changed. */
  globalChanged: boolean;
  /** Session command to apply, or null if no session change is needed. */
  sessionOp: { b: number | null; on: boolean | null } | null;
}

/** Effective hidden-files visibility of browser `b` (`0` or `1`). */
export function effectiveHidden(global: boolean, override: boolean | null | undefined): boolean {
  return override ?? global;
}

/**
 * Divergence state for browser `b`: returns a primitive (`true` = shown here only,
 * `false` = hidden here only, `null` = synced or single-board) so `useSyncExternalStore`
 * selectors stay referentially stable.
 */
export function hiddenDivergence(
  global: boolean,
  overrides: HiddenOverrides,
  b: number,
  singleBoard: boolean,
): boolean | null {
  if (singleBoard) return null;
  const idx = b === 0 ? 0 : 1;
  const own = effectiveHidden(global, overrides[idx]);
  const other = effectiveHidden(global, overrides[1 - idx]);
  if (own === other) return null;
  // Only the board that diverges from the global baseline carries the override badge.
  if (own === global) return null;
  return own;
}

/**
 * Plans toggling hidden files for a single browser `b`.
 * - Single-board view: flips global directly and clears any overrides.
 * - Dual-board view: flips only `b`; if both boards now match, self-heals by
 *   updating global to the shared state and clearing both overrides.
 */
export function planToggleHiddenBoard(
  global: boolean,
  overrides: HiddenOverrides,
  b: number,
  singleBoard: boolean,
): HiddenTogglePlan {
  const idx = b === 0 ? 0 : 1;
  const cur = effectiveHidden(global, overrides[idx]);
  const next = !cur;
  const hasAnyOverride = overrides[0] != null || overrides[1] != null;

  if (singleBoard) {
    return {
      nextGlobal: next,
      nextOverrides: [null, null],
      globalChanged: next !== global,
      sessionOp: hasAnyOverride ? { b: null, on: null } : null,
    };
  }

  const otherEff = effectiveHidden(global, overrides[1 - idx]);
  if (next === otherEff) {
    // Both boards now agree: self-heal back into global sync.
    return {
      nextGlobal: next,
      nextOverrides: [null, null],
      globalChanged: next !== global,
      sessionOp: hasAnyOverride ? { b: null, on: null } : null,
    };
  }

  // Boards diverge: keep global unchanged and store an override only on the board that differs.
  const eff0 = idx === 0 ? next : otherEff;
  const eff1 = idx === 1 ? next : otherEff;
  const nextOverrides: [boolean | null, boolean | null] = [
    eff0 === global ? null : eff0,
    eff1 === global ? null : eff1,
  ];
  return {
    nextGlobal: global,
    nextOverrides,
    globalChanged: false,
    sessionOp: { b: idx, on: nextOverrides[idx] },
  };
}

/** Plans toggling hidden files globally across both boards and clearing overrides. */
export function planToggleHiddenAll(global: boolean, overrides: HiddenOverrides): HiddenTogglePlan {
  return planApplyHiddenAll(global, overrides, !global);
}

/** Plans setting hidden files to `on` globally across both boards and re-syncing. */
export function planApplyHiddenAll(global: boolean, overrides: HiddenOverrides, on: boolean): HiddenTogglePlan {
  const hasAnyOverride = overrides[0] != null || overrides[1] != null;
  return {
    nextGlobal: on,
    nextOverrides: [null, null],
    globalChanged: on !== global,
    sessionOp: hasAnyOverride ? { b: null, on: null } : null,
  };
}
