// Estimated time left for file operations. Pure, unit-tested.

/** Smoothed transfer rate of one task. */
export interface Rate {
  /** When the task was first seen (ms). */
  t0: number;
  /** Time and amount of the last sample that went into `perSec`. */
  t: number;
  done: number;
  /** Units (bytes or items) per second, smoothed; 0 until known. */
  perSec: number;
}

const MIN_SAMPLE_MS = 250;
/** Time constant of the smoothing: older speed fades out over a few seconds. */
const TAU_MS = 3000;
/** No estimate in the first moments: the first speed readings are unreliable. */
const WARMUP_MS = 1500;

/** Folds a new progress reading (`done` units at `now` ms) into the rate. */
export function updateRate(prev: Rate | undefined, now: number, done: number): Rate {
  if (!prev || done < prev.done) return { t0: now, t: now, done, perSec: 0 };
  const dt = now - prev.t;
  if (dt < MIN_SAMPLE_MS) return prev;
  const inst = ((done - prev.done) * 1000) / dt;
  // Exponential moving average weighted by the sample's duration.
  const a = prev.perSec ? 1 - Math.exp(-dt / TAU_MS) : 1;
  return { t0: prev.t0, t: now, done, perSec: prev.perSec + a * (inst - prev.perSec) };
}

/** Seconds left, or null while there is no reliable estimate yet. */
export function etaSeconds(r: Rate | undefined, total: number, now: number): number | null {
  if (!r || r.perSec <= 0 || total <= 0 || now - r.t0 < WARMUP_MS) return null;
  return Math.max(0, (total - r.done) / r.perSec);
}

/** "~8 s", "~2 min 10 s", "~1 h 5 min". */
export function formatEta(s: number): string {
  const t = Math.ceil(s);
  if (t < 60) return `~${Math.max(1, t)} s`;
  if (t < 3600) {
    const m = Math.floor(t / 60);
    const r = t % 60;
    // Tens of seconds are enough detail above a minute; none above ten.
    return m >= 10 || r < 10 ? `~${m} min` : `~${m} min ${Math.round(r / 10) * 10} s`;
  }
  const h = Math.floor(t / 3600);
  const m = Math.round((t % 3600) / 60);
  return m ? `~${h} h ${m} min` : `~${h} h`;
}
