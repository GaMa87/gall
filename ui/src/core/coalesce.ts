/** A save that may wait for the typing to pause: only the newest value is written, one
 *  write at a time and in order, so an older value never lands after a newer one.
 *  `save` must not reject (report its own errors). */
export function coalesced<T>(save: (value: T) => Promise<void>, delayMs: number) {
  let pending: { value: T } | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let chain: Promise<void> = Promise.resolve();
  const run = (): Promise<void> => {
    if (timer) clearTimeout(timer);
    timer = null;
    if (pending) {
      const { value } = pending;
      pending = null;
      chain = chain.catch(() => {}).then(() => save(value));
    }
    return chain;
  };
  return {
    /** Writes `value` once nothing newer came for `delayMs`. */
    later(value: T) {
      pending = { value };
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => void run(), delayMs);
    },
    /** Writes `value` now (after a write that is under way); resolves when it is written. */
    now(value: T): Promise<void> {
      pending = { value };
      return run();
    },
    /** Writes what is waiting, if anything; resolves when nothing is under way. */
    flush: run,
  };
}
