/** Event-driven image snapshots. Commit only decoded bytes; never replace a good
 * preview with a failed/partial write. A new event supersedes all pending work. */
export function imageRefresh<T>(
  load: (signal: AbortSignal) => Promise<T>,
  show: (image: T) => void,
  release: (image: T) => void,
) {
  let generation = 0;
  let stopped = false;
  let current: T | undefined;
  let request: AbortController | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const refresh = () => {
    if (stopped) return;
    const mine = ++generation;
    request?.abort();
    clearTimeout(timer);
    const attempt = async (retry: number) => {
      if (stopped || mine !== generation) return;
      const controller = new AbortController();
      request = controller;
      try {
        const next = await load(controller.signal);
        if (stopped || mine !== generation) { release(next); return; }
        const old = current;
        current = next;
        show(next);
        if (old !== undefined) release(old);
      } catch {
        // Bounded retries bridge rename gaps, sharing violations and partial writes.
        // Later filesystem events start a fresh retry budget, even with equal stat data.
        if (!stopped && mine === generation && retry < 8) {
          timer = setTimeout(() => void attempt(retry + 1), Math.min(2000, 150 * 2 ** retry));
        }
      }
    };
    void attempt(0);
  };
  return {
    refresh,
    stop() {
      stopped = true;
      generation++;
      request?.abort();
      clearTimeout(timer);
      if (current !== undefined) release(current);
      current = undefined;
    },
  };
}

export async function loadImageSnapshot(src: string, signal: AbortSignal): Promise<string> {
  const response = await fetch(src, { cache: "no-store", signal });
  if (!response.ok) throw new Error(response.statusText);
  const url = URL.createObjectURL(await response.blob());
  try {
    const image = new Image();
    image.src = url;
    await image.decode();
    if (signal.aborted) throw new Error("Superseded image");
    return url;
  } catch (error) {
    URL.revokeObjectURL(url);
    throw error;
  }
}
