// Page of `scripts/compare-browser-check.mjs`: runs the browser comparison against the installed module in a real
// browser and reports what happened as JSON (`window.__compareCheck()`). Not part of the application bundle.
import { browserDiffPageClose, browserDiffPageHunks, browserDiffPageOpen, browserDiffPageSmall } from "./core/compareBrowserPages";
import { browserTransport, compareInBrowser, currentBrowserEnv, mergeInBrowser } from "./core/compareBrowser";
import type { DiffOptions } from "./api/types";

const opts: DiffOptions = { ignoreCase: false, ignoreEol: false, minimal: false, structural: false };
const text = (t: string) => ({ kind: "text" as const, label: "x", text: t });
const enc = (s: string) => new TextEncoder().encode(s);

/** Frame intervals of the main thread while `work` runs. */
async function frames<T>(work: () => Promise<T>): Promise<{ result: T; p95: number; max: number; count: number }> {
  const stamps: number[] = [];
  let live = true;
  const tick = (t: number) => { stamps.push(t); if (live) requestAnimationFrame(tick); };
  requestAnimationFrame(tick);
  const result = await work();
  live = false;
  const gaps = stamps.slice(1).map((t, i) => t - stamps[i]).sort((a, b) => a - b);
  return { result, p95: gaps[Math.floor(gaps.length * 0.95)] ?? 0, max: gaps[gaps.length - 1] ?? 0, count: gaps.length };
}

function scaleText(lines: number, change: number): [string, string] {
  const a: string[] = [], b: string[] = [];
  for (let i = 0; i < lines; i++) {
    const line = `line ${i} ${"x".repeat(40)}`;
    a.push(line);
    b.push(i % change === 0 ? `${line} changed` : line);
  }
  return [a.join("\n") + "\n", b.join("\n") + "\n"];
}

async function run() {
  const report: Record<string, unknown> = { env: currentBrowserEnv(), moduleUrl: new URL("/compare-wasm/gall_compare.js", location.href).href };
  const transport = browserTransport();
  report.probe = await transport.probe();

  const summary = await browserDiffPageOpen(1, text("a\nb\nc\n"), text("a\nB\nc\nd\n"), opts);
  report.text = { added: summary.text?.added, removed: summary.text?.removed, ops: browserDiffPageSmall(1)?.ops.map((o) => o.tag), hunks: browserDiffPageHunks(1, 0, 10).total };
  browserDiffPageClose(1);

  const binary = await browserDiffPageOpen(2, text("a\0b"), text("a\0c"), opts);
  report.binary = binary.binary;
  report.identical = (await browserDiffPageOpen(3, text("same\n"), text("same\n"), opts)).text;

  const base = enc("a\nb\nc\n"), left = enc("a\nours\nc\n"), right = enc("a\ntheirs\nc\n");
  const env = currentBrowserEnv();
  const open = await mergeInBrowser(transport, env, base, left, right) as { kind: string; merge?: { conflicts: string[] } };
  const id = open.merge?.conflicts[0] ?? "";
  const resolved = await mergeInBrowser(transport, env, enc("a\nb\nc\n"), enc("a\nours\nc\n"), enc("a\ntheirs\nc\n"), { choices: { [id]: "both" } }) as { kind: string; text?: string };
  const unresolved = await mergeInBrowser(transport, env, enc("a\nb\nc\n"), enc("a\nours\nc\n"), enc("a\ntheirs\nc\n"), { choices: {} }) as { kind: string };
  const added = await mergeInBrowser(transport, env, null, enc("x\n"), enc("y\n")) as { kind: string };
  report.merge = { open: open.kind, conflicts: open.merge?.conflicts.length, resolved: resolved.kind, text: resolved.text, unresolved: unresolved.kind, withoutBase: added.kind };

  // Cancel: a comparison that takes a while is ended, and the next one starts a fresh worker.
  const [big1, big2] = scaleText(200_000, 3);
  const slow = compareInBrowser(transport, env, enc(big1), enc(big2), { minimal: true });
  await new Promise((r) => setTimeout(r, 20));
  transport.cancel();
  const cancelled = await slow;
  const after = await compareInBrowser(transport, env, enc("a\n"), enc("b\n"));
  report.cancel = { first: cancelled.kind, next: after.kind };

  // Frame budget of the main thread while a T01-scale comparison (200,000 lines, the engine's line limit) runs.
  const [s1, s2] = scaleText(200_000, 50);
  const bytesA = enc(s1), bytesB = enc(s2);
  const bytes = bytesA.byteLength;
  const started = performance.now();
  const measured = await frames(() => compareInBrowser(transport, env, bytesA, bytesB));
  report.scale = { kind: measured.result.kind, ms: Math.round(performance.now() - started), frames: measured.count, p95: measured.p95, max: measured.max, bytes };
  report.limit = (await compareInBrowser(transport, env, new Uint8Array(33 * 1024 * 1024), new Uint8Array(1))).kind;
  return report;
}

(window as unknown as { __compareCheck: () => Promise<unknown> }).__compareCheck = run;
