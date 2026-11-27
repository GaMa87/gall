import { useEffect, useMemo, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  createInitialProgress,
  formatElapsed,
  formatProgressMetrics,
  mergeProgress,
  progressAriaLabel,
  type GitProgressEvent,
  type GitProgressState,
} from "../core/gitProgress";
import { tt } from "../i18n";
import "./GitProgressBar.css";

type Listener = (event: GitProgressEvent) => void;

const subscribers = new Map<string, Set<Listener>>();
let sharedUnlisten: Promise<UnlistenFn> | null = null;

function ensureListener() {
  if (sharedUnlisten || typeof window === "undefined") return;
  sharedUnlisten = listen<GitProgressEvent>("git-progress", (event) => {
    const set = subscribers.get(event.payload.id);
    if (!set) return;
    set.forEach((cb) => cb(event.payload));
  }).catch(() => () => {});
}

function subscribeGitProgress(id: string, cb: Listener): () => void {
  ensureListener();
  let set = subscribers.get(id);
  if (!set) {
    set = new Set();
    subscribers.set(id, set);
  }
  set.add(cb);
  return () => {
    set?.delete(cb);
    if (set && set.size === 0) subscribers.delete(id);
  };
}

export interface GitProgressSeed {
  operation: string;
  phase: string;
  cancellable?: boolean;
  detail?: string;
}

/**
 * Tracks live backend `"git-progress"` events for `id`, displaying an immediate `t = 0`
 * state from `seed` with zero artificial delay and updating elapsed time smoothly.
 */
export function useGitProgress(id: string | null, seed?: GitProgressSeed): GitProgressState | null {
  const [state, setState] = useState<GitProgressState | null>(() =>
    id ? createInitialProgress(id, seed?.operation ?? "git", seed?.phase ?? "Running Git operation", Boolean(seed?.cancellable), { detail: seed?.detail }) : null,
  );
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (!id) {
      setState(null);
      return;
    }
    const startedAt = Date.now();
    setNow(startedAt);
    setState(
      createInitialProgress(
        id,
        seed?.operation ?? "git",
        seed?.phase ?? "Running Git operation",
        Boolean(seed?.cancellable),
        { detail: seed?.detail, startedAt },
      ),
    );
    const off = subscribeGitProgress(id, (event) => {
      setState((prev) => mergeProgress(prev, event));
    });
    const timer = window.setInterval(() => setNow(Date.now()), 250);
    return () => {
      off();
      window.clearInterval(timer);
    };
  }, [id, seed?.operation, seed?.phase, seed?.cancellable, seed?.detail]);

  if (!id || !state || state.id !== id) return null;
  const elapsedMs = state.finished ? state.elapsedMs : Math.max(state.elapsedMs, now - state.startedAt);
  return { ...state, elapsedMs };
}

/**
 * Immediate `t = 0` progress tracker for client-driven loads and counted loops with zero IPC overhead.
 */
export function useLocalProgress(
  active: boolean,
  phase: string,
  opts?: {
    operation?: string;
    done?: number | null;
    total?: number | null;
    unit?: string | null;
    detail?: string | null;
    cancellable?: boolean;
  },
): GitProgressState | null {
  const [startedAt, setStartedAt] = useState<number>(() => Date.now());
  const [now, setNow] = useState<number>(() => Date.now());

  useEffect(() => {
    if (!active) return;
    const t0 = Date.now();
    setStartedAt(t0);
    setNow(t0);
    const timer = window.setInterval(() => setNow(Date.now()), 250);
    return () => window.clearInterval(timer);
  }, [active]);

  return useMemo(() => {
    if (!active) return null;
    const base = createInitialProgress("local", opts?.operation ?? "load", phase, Boolean(opts?.cancellable), {
      done: opts?.done,
      total: opts?.total,
      unit: opts?.unit,
      detail: opts?.detail,
      startedAt,
    });
    return { ...base, elapsedMs: Math.max(0, now - startedAt) };
  }, [active, phase, opts?.operation, opts?.done, opts?.total, opts?.unit, opts?.detail, opts?.cancellable, startedAt, now]);
}

export interface GitProgressBarProps {
  progress: GitProgressState;
  compact?: boolean;
  inline?: boolean;
  onCancel?: () => void;
  cancelLabel?: string;
  className?: string;
}

/** Unified accessible Git progress bar using semantic `<progress>` and honest indeterminate fallback. */
export function GitProgressBar({
  progress,
  compact = false,
  inline = false,
  onCancel,
  cancelLabel,
  className,
}: GitProgressBarProps) {
  const determinate = progress.percent !== null && progress.fraction !== null;
  const phaseLabel = tt(progress.phase || "Running Git operation");
  const metrics = formatProgressMetrics(progress, tt);
  const elapsed = formatElapsed(progress.elapsedMs);
  const ariaLabel = progressAriaLabel(progress, tt);
  const classes = ["gpb", compact ? "gpb-compact" : "", inline ? "gpb-inline" : "", determinate ? "gpb-det" : "gpb-indet", className ?? ""]
    .filter(Boolean)
    .join(" ");

  return (
    <div className={classes} role="status" aria-live="polite">
      <div className="gpb-row">
        <span className="gpb-phase" title={progress.detail ? `${phaseLabel} · ${progress.detail}` : phaseLabel}>
          <b>{phaseLabel}</b>
          {progress.detail && <span className="gpb-detail">{progress.detail}</span>}
        </span>
        <span className="gpb-meta">
          {determinate && <span className="gpb-pct">{progress.percent}%</span>}
          {metrics && <span className="gpb-metrics">{metrics}</span>}
          <span className="gpb-elapsed" aria-label={tt("Elapsed time")}>{elapsed}</span>
          {onCancel && progress.cancellable && (
            <button type="button" className="gpb-cancel" onClick={onCancel}>
              {cancelLabel ?? tt("Cancel")}
            </button>
          )}
        </span>
      </div>
      {determinate ? (
        <progress
          className="gpb-meter"
          value={progress.percent!}
          max={100}
          aria-valuenow={progress.percent!}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-label={ariaLabel}
        />
      ) : (
        <progress className="gpb-meter" aria-label={ariaLabel} />
      )}
    </div>
  );
}
