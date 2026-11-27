// What merging a branch would do (gitforecast.rs), asked in the background, a few at a time.
import type { GitReference } from "./gitRepository";

export type Forecast = { known: boolean; files: string[] };

/** Runs tasks by key, at most `limit` at once and in order; what was asked for is kept. */
export class ForecastQueue {
  private readonly done = new Map<string, Forecast>();
  private readonly waiting = new Map<string, { run: () => Promise<Forecast>; then: ((value: Forecast) => void)[] }>();
  private running = 0;
  constructor(private readonly limit = 2, private readonly keep = 2000) {}
  get(key: string): Forecast | undefined { return this.done.get(key); }
  ask(key: string, run: () => Promise<Forecast>, then: (value: Forecast) => void): void {
    const known = this.done.get(key);
    if (known) { then(known); return; }
    const queued = this.waiting.get(key);
    if (queued) { queued.then.push(then); return; }
    this.waiting.set(key, { run, then: [then] });
    this.next();
  }
  /** Forgets what has not started. */
  clear(): void {
    for (const key of [...this.waiting.keys()]) if (!this.active.has(key)) this.waiting.delete(key);
  }
  private readonly active = new Set<string>();
  private next(): void {
    if (this.running >= this.limit) return;
    const key = [...this.waiting.keys()].find((name) => !this.active.has(name));
    if (key === undefined) return;
    const task = this.waiting.get(key)!;
    this.active.add(key);
    this.running++;
    task.run().then((value) => {
      // Unknown may mean cancelled: asked again another time.
      if (value.known) {
        this.done.set(key, value);
        if (this.done.size > this.keep) this.done.delete(this.done.keys().next().value!);
      }
      for (const then of task.then) then(value);
    }).catch(() => {}).finally(() => {
      this.running--;
      this.active.delete(key);
      this.waiting.delete(key);
      this.next();
    });
    this.next();
  }
}

/** The branches to look at: local ones at another commit than HEAD, none when there are too many. */
export function forecastBranches(refs: GitReference[], head: string, limit: number): GitReference[] {
  if (limit <= 0 || !head) return [];
  const branches = refs.filter((ref) => ref.kind === "branch");
  return branches.length > limit ? [] : branches.filter((ref) => ref.id !== head);
}

/** The first names, and how many more there are. */
export function forecastFiles(forecast: Forecast, shown = 8): { names: string[]; more: number } {
  return { names: forecast.files.slice(0, shown), more: Math.max(0, forecast.files.length - shown) };
}
