/** Steps a task needs: `keys` counts every key press including the `typed` characters, `clicks` every mouse button press. */
export type Steps = { keys: number; typed: number; clicks: number };
/** Stored yardstick: steps per scenario and route (`1k` keyboard, `1m` mouse), latencies per platform (measured only). */
export type Baseline = { steps: Record<string, Steps>; latencyMs: Record<string, Record<string, number>> };

/** A task got longer: more clicks, or more keys beyond the typed text (a longer text is no longer task). */
export function overBaseline(now: Steps, limit: Steps): boolean {
  return now.clicks > limit.clicks || now.keys - now.typed > limit.keys - limit.typed;
}

export function stepsText(steps: Steps): string {
  return `${steps.keys} keys (${steps.typed} typed), ${steps.clicks} clicks`;
}

/** Where a right-click on an element lands: near its start, vertically centred. */
export function contextMenuPoint(box: { left: number; top: number; width: number; height: number }) {
  return { clientX: box.left + Math.min(box.width / 2, 60), clientY: box.top + box.height / 2 };
}