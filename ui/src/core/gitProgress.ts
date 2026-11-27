export interface GitProgressEvent {
  id: string;
  operation: string;
  phase: string;
  fraction: number | null;
  percent: number | null;
  done: number | null;
  total: number | null;
  unit: string | null;
  transferred: string | null;
  rate: string | null;
  remote: boolean;
  phaseIndex: number | null;
  phaseCount: number | null;
  detail: string | null;
  elapsedMs: number;
  cancellable: boolean;
  finished: boolean;
}

export interface GitProgressState extends GitProgressEvent {
  /** Monotonic wall-clock timestamp (ms) when tracking began on the UI side. */
  startedAt: number;
}

let seq = 0;

export function createProgressId(prefix = "git"): string {
  seq += 1;
  return `${prefix}-${Date.now()}-${seq}`;
}

export function createInitialProgress(
  id: string,
  operation: string,
  phase: string,
  cancellable = false,
  opts?: {
    done?: number | null;
    total?: number | null;
    unit?: string | null;
    detail?: string | null;
    startedAt?: number;
  },
): GitProgressState {
  const done = opts?.done ?? null;
  const total = opts?.total ?? null;
  const hasRealTotal = done !== null && total !== null && total > 0;
  const fraction = hasRealTotal ? Math.max(0, Math.min(1, done / total)) : null;
  const percent = fraction !== null ? Math.round(fraction * 100) : null;
  return {
    id,
    operation,
    phase,
    fraction,
    percent,
    done,
    total: total !== null && total > 0 ? total : null,
    unit: opts?.unit ?? null,
    transferred: null,
    rate: null,
    remote: false,
    phaseIndex: null,
    phaseCount: null,
    detail: opts?.detail ?? null,
    elapsedMs: 0,
    cancellable,
    finished: false,
    startedAt: opts?.startedAt ?? Date.now(),
  };
}

/** Validates incoming IPC progress, ensuring percentages are never fabricated when totals are unknown. */
export function normalizeProgress(event: GitProgressEvent, startedAt = Date.now()): GitProgressState {
  const rawFraction =
    typeof event.fraction === "number" && Number.isFinite(event.fraction)
      ? Math.max(0, Math.min(1, event.fraction))
      : null;
  const computedPercent =
    rawFraction !== null
      ? Math.max(0, Math.min(100, Math.round(rawFraction * 100)))
      : typeof event.percent === "number" && Number.isFinite(event.percent) && event.total !== null && event.total > 0
        ? Math.max(0, Math.min(100, Math.round(event.percent)))
        : null;
  const fraction = rawFraction ?? (computedPercent !== null ? computedPercent / 100 : null);
  return {
    ...event,
    fraction,
    percent: computedPercent,
    done: typeof event.done === "number" && Number.isFinite(event.done) && event.done >= 0 ? event.done : null,
    total: typeof event.total === "number" && Number.isFinite(event.total) && event.total > 0 ? event.total : null,
    elapsedMs: typeof event.elapsedMs === "number" && Number.isFinite(event.elapsedMs) && event.elapsedMs >= 0 ? event.elapsedMs : 0,
    startedAt,
  };
}

export function mergeProgress(prev: GitProgressState | null, incoming: GitProgressEvent): GitProgressState {
  if (prev && prev.id !== incoming.id) return prev;
  const normalized = normalizeProgress(incoming, prev?.startedAt ?? Date.now());
  return {
    ...normalized,
    operation: normalized.operation || prev?.operation || "git",
    phase: normalized.phase || prev?.phase || "Running Git operation",
    cancellable: normalized.cancellable || Boolean(prev?.cancellable),
  };
}

export function formatElapsed(ms: number): string {
  const safe = Math.max(0, Number.isFinite(ms) ? ms : 0);
  if (safe < 60_000) {
    return `${(safe / 1000).toFixed(1)}s`;
  }
  const totalSec = Math.floor(safe / 1000);
  const mins = Math.floor(totalSec / 60);
  const secs = totalSec % 60;
  return `${mins}m ${String(secs).padStart(2, "0")}s`;
}

export function formatProgressMetrics(
  state: Pick<GitProgressState, "done" | "total" | "unit" | "transferred" | "rate">,
  translate: (text: string) => string = (s) => s,
): string {
  const parts: string[] = [];
  if (state.done !== null) {
    const unit = state.unit ? ` ${translate(state.unit)}` : "";
    if (state.total !== null && state.total > 0) {
      parts.push(`${state.done.toLocaleString()} / ${state.total.toLocaleString()}${unit}`);
    } else {
      parts.push(`${state.done.toLocaleString()}${unit}`);
    }
  }
  if (state.transferred) {
    parts.push(state.rate ? `${state.transferred} · ${state.rate}` : state.transferred);
  } else if (state.rate) {
    parts.push(state.rate);
  }
  return parts.join(" · ");
}

export function progressAriaLabel(
  state: Pick<GitProgressState, "phase" | "percent" | "done" | "total" | "unit">,
  translate: (text: string) => string = (s) => s,
): string {
  const phase = translate(state.phase || "Running Git operation");
  if (state.percent !== null) {
    return `${phase} (${state.percent}%)`;
  }
  const metrics = formatProgressMetrics({ ...state, transferred: null, rate: null }, translate);
  return metrics ? `${phase} (${metrics})` : phase;
}
