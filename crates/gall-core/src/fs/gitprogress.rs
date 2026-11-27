//! Structured progress tracking and Git `--progress` stderr parsing for long-running
//! Git module operations.
//!
//! Fast paths pay zero overhead: when no [`ProgressTracker`] is installed on the current
//! thread via [`with_tracker`], helpers return immediately without spawning threads,
//! parsing stderr, or emitting IPC events.

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Target throttle interval (~18 Hz) for steady progress updates. Start, phase changes,
/// determinacy flips, phase completion, and terminal updates bypass the throttle.
pub const DEFAULT_THROTTLE: Duration = Duration::from_millis(55);

/// Structured progress snapshot emitted over IPC for tracked Git operations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitProgress {
    /// Operation/job ID matching the UI tracker.
    pub id: String,
    /// Operation category (e.g. `fetch`, `pull`, `push`, `clone`, `checkout`, `rebase`,
    /// `rebaseConflicts`, `worktree`, `submodule`, `absorb`, `blame`, `search`, `custom`).
    pub operation: String,
    /// Current phase label (English key, localized in the UI).
    pub phase: String,
    /// Real determinate fraction in `0.0..=1.0`, or `None` when totals are unknown.
    pub fraction: Option<f64>,
    /// Real integer percentage `0..=100`, or `None` when indeterminate.
    pub percent: Option<u8>,
    /// Completed items/steps/lines/files/objects in the current phase, if known.
    pub done: Option<u64>,
    /// Total items/steps/lines/files/objects in the current phase, if known.
    pub total: Option<u64>,
    /// Unit name for `done`/`total` (e.g. `objects`, `deltas`, `files`, `commits`, `lines`).
    pub unit: Option<String>,
    /// Transferred size string from Git (e.g. `12.40 MiB`).
    pub transferred: Option<String>,
    /// Transfer rate string from Git (e.g. `4.20 MiB/s`).
    pub rate: Option<String>,
    /// Whether this phase came from remote sideband (`remote: ...`).
    pub remote: bool,
    /// 1-based current phase index for known multi-phase pipelines.
    pub phase_index: Option<u32>,
    /// Total known phases for multi-phase pipelines.
    pub phase_count: Option<u32>,
    /// Optional detail such as the active file, submodule, or commit hash.
    pub detail: Option<String>,
    /// Elapsed milliseconds since operation start.
    pub elapsed_ms: u64,
    /// Whether the operation supports cancellation.
    pub cancellable: bool,
    /// True on the final terminal update when the operation ends.
    pub finished: bool,
}

/// Known multi-phase Git pipelines whose determinate phase bands combine smoothly
/// without fabricating progress during indeterminate phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pipeline {
    #[default]
    Single,
    Fetch,
    Pull,
    Push,
    Clone,
    Worktree,
}

impl Pipeline {
    /// Returns `(start, end, phase_index, phase_count)` for a determinate phase in this
    /// pipeline, or `None` to map directly `0.0..=1.0`.
    fn band(self, phase: &str, remote: bool) -> Option<(f64, f64, u32, u32)> {
        let lower = phase.to_ascii_lowercase();
        match self {
            Pipeline::Single => None,
            Pipeline::Fetch => {
                if lower.starts_with("counting") {
                    Some((0.0, 0.12, 1, 4))
                } else if lower.starts_with("compressing") {
                    Some((0.12, 0.24, 2, 4))
                } else if lower.starts_with("receiving") {
                    Some((0.24, 0.78, 3, 4))
                } else if lower.starts_with("resolving") {
                    Some((0.78, 0.96, 4, 4))
                } else if lower.starts_with("updating") || lower.starts_with("checking out") {
                    Some((0.96, 1.0, 4, 4))
                } else {
                    None
                }
            }
            Pipeline::Pull => {
                if lower.starts_with("counting") {
                    Some((0.0, 0.10, 1, 5))
                } else if lower.starts_with("compressing") {
                    Some((0.10, 0.20, 2, 5))
                } else if lower.starts_with("receiving") {
                    Some((0.20, 0.68, 3, 5))
                } else if lower.starts_with("resolving") {
                    Some((0.68, 0.84, 4, 5))
                } else if lower.starts_with("updating")
                    || lower.starts_with("checking out")
                    || lower.starts_with("rebasing")
                {
                    Some((0.84, 1.0, 5, 5))
                } else {
                    None
                }
            }
            Pipeline::Push => {
                if lower.starts_with("counting") {
                    Some((0.0, 0.15, 1, 4))
                } else if lower.starts_with("compressing") {
                    Some((0.15, 0.35, 2, 4))
                } else if lower.starts_with("writing") {
                    Some((0.35, 0.90, 3, 4))
                } else if lower.starts_with("resolving") || remote {
                    Some((0.90, 1.0, 4, 4))
                } else {
                    None
                }
            }
            Pipeline::Clone => {
                if lower.starts_with("counting") {
                    Some((0.0, 0.08, 1, 5))
                } else if lower.starts_with("compressing") {
                    Some((0.08, 0.16, 2, 5))
                } else if lower.starts_with("receiving") {
                    Some((0.16, 0.62, 3, 5))
                } else if lower.starts_with("resolving") {
                    Some((0.62, 0.78, 4, 5))
                } else if lower.starts_with("updating") || lower.starts_with("checking out") {
                    Some((0.78, 1.0, 5, 5))
                } else {
                    None
                }
            }
            Pipeline::Worktree => {
                if lower.starts_with("updating") || lower.starts_with("checking out") {
                    Some((0.0, 0.65, 1, 2))
                } else if lower.contains("worktreeinclude")
                    || lower.starts_with("copying")
                    || lower.starts_with("linking")
                {
                    Some((0.65, 1.0, 2, 2))
                } else {
                    None
                }
            }
        }
    }
}

/// Parsed representation of one Git stderr progress line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLine {
    pub remote: bool,
    pub phase: String,
    pub percent: Option<u8>,
    pub done: Option<u64>,
    pub total: Option<u64>,
    pub unit: Option<String>,
    pub transferred: Option<String>,
    pub rate: Option<String>,
    pub detail: Option<String>,
    pub phase_done: bool,
}

/// Strips ANSI CSI escape sequences (such as `\x1b[K` clear-to-EOL used by Git sideband).
fn strip_ansi(input: &str) -> String {
    if !input.as_bytes().contains(&0x1b) {
        return input.to_owned();
    }
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            i += 2;
            while i < bytes.len() {
                let b = bytes[i];
                i += 1;
                if (0x40..=0x7e).contains(&b) {
                    break;
                }
            }
        } else {
            let ch = input[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn infer_unit(phase: &str) -> Option<String> {
    let lower = phase.to_ascii_lowercase();
    if lower.contains("object director") {
        Some("directories".into())
    } else if lower.contains("object") {
        Some("objects".into())
    } else if lower.contains("delta") {
        Some("deltas".into())
    } else if lower.contains("file") {
        Some("files".into())
    } else if lower.contains("commit") || lower.starts_with("rebasing") {
        Some("commits".into())
    } else if lower.contains("submodule") {
        Some("submodules".into())
    } else if lower.contains("worktree") {
        Some("worktrees".into())
    } else if lower.contains("pass") {
        Some("passes".into())
    } else {
        None
    }
}

fn looks_like_byte_size(text: &str) -> bool {
    let mut parts = text.split_whitespace();
    let Some(num) = parts.next() else {
        return false;
    };
    let Some(unit) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    num.chars().all(|c| c.is_ascii_digit() || c == '.')
        && matches!(
            unit,
            "bytes" | "B" | "KiB" | "MiB" | "GiB" | "TiB" | "KB" | "MB" | "GB" | "TB"
        )
}

fn parse_transfer_tail(tail: &str) -> (Option<String>, Option<String>) {
    let trimmed = tail.trim().trim_start_matches(',').trim();
    if trimmed.is_empty() {
        return (None, None);
    }
    if let Some((left, right)) = trimmed.split_once('|') {
        let left = left.trim();
        let right = right.trim();
        let transferred = looks_like_byte_size(left).then(|| left.to_owned());
        let rate = (!right.is_empty() && right.ends_with("/s")).then(|| right.to_owned());
        return (transferred, rate);
    }
    if looks_like_byte_size(trimmed) {
        return (Some(trimmed.to_owned()), None);
    }
    if trimmed.ends_with("/s") {
        return (None, Some(trimmed.to_owned()));
    }
    (None, None)
}

/// Parses a single stderr line emitted by Git (or a Git-compatible command).
/// Returns `None` for non-progress diagnostic lines.
pub fn parse_line(raw: &str) -> Option<ParsedLine> {
    let cleaned = strip_ansi(raw);
    let mut line = cleaned.trim();
    if line.is_empty() {
        return None;
    }
    let mut remote = false;
    while let Some(rest) = line.strip_prefix("remote:") {
        remote = true;
        line = rest.trim_start();
    }
    if line.is_empty() {
        return None;
    }

    // `Rebasing (k/n)`
    if let Some(rest) = line.strip_prefix("Rebasing (") {
        let inner = rest.split(')').next()?;
        let (done, total) = inner.split_once('/')?;
        let done: u64 = done.trim().parse().ok()?;
        let total: u64 = total.trim().parse().ok()?;
        if let Some(raw_pct) = done.min(total).saturating_mul(100).checked_div(total) {
            let pct = raw_pct.min(100) as u8;
            return Some(ParsedLine {
                remote,
                phase: "Rebasing".into(),
                percent: Some(pct),
                done: Some(done),
                total: Some(total),
                unit: Some("commits".into()),
                transferred: None,
                rate: None,
                detail: None,
                phase_done: false,
            });
        }
    }

    // `Delta compression using up to N threads`
    if line.starts_with("Delta compression using ") {
        return Some(ParsedLine {
            remote,
            phase: "Delta compression".into(),
            percent: None,
            done: None,
            total: None,
            unit: None,
            transferred: None,
            rate: None,
            detail: None,
            phase_done: false,
        });
    }

    // `Cloning into 'dir'...`
    if let Some(rest) = line.strip_prefix("Cloning into '") {
        let detail = rest.split('\'').next().map(str::to_owned);
        return Some(ParsedLine {
            remote,
            phase: "Cloning".into(),
            percent: None,
            done: None,
            total: None,
            unit: None,
            transferred: None,
            rate: None,
            detail,
            phase_done: false,
        });
    }

    // `Preparing worktree (...)`
    if line.starts_with("Preparing worktree") {
        return Some(ParsedLine {
            remote,
            phase: "Preparing worktree".into(),
            percent: None,
            done: None,
            total: None,
            unit: None,
            transferred: None,
            rate: None,
            detail: None,
            phase_done: false,
        });
    }

    // Standard Git progress: `<Phase>: ...`
    let (phase_raw, rest) = line.split_once(':')?;
    let phase = phase_raw.trim();
    if phase.is_empty()
        || phase.len() > 80
        || phase.starts_with("error")
        || phase.starts_with("fatal")
        || phase.starts_with("warning")
        || phase.starts_with("hint")
    {
        return None;
    }

    let mut body = rest.trim();
    let phase_done = if let Some(stripped) = body.strip_suffix(", done.") {
        body = stripped.trim_end();
        true
    } else if let Some(stripped) = body.strip_suffix(", done") {
        body = stripped.trim_end();
        true
    } else {
        false
    };

    // Case 1: Determinate `<pct>% (<done>/<total>)[, <bytes> | <rate>]`
    if let Some((pct_str, after_pct)) = body.split_once('%') {
        let pct_str = pct_str.trim();
        if let Ok(pct) = pct_str.parse::<u8>() {
            if pct <= 100 {
                let after_pct = after_pct.trim_start();
                let (done, total, tail) = if let Some(paren_rest) = after_pct.strip_prefix('(') {
                    if let Some((counts, tail)) = paren_rest.split_once(')') {
                        if let Some((d, t)) = counts.split_once('/') {
                            match (d.trim().parse::<u64>(), t.trim().parse::<u64>()) {
                                (Ok(done), Ok(total)) if total > 0 => {
                                    (Some(done), Some(total), tail)
                                }
                                _ => (None, None, tail),
                            }
                        } else {
                            (None, None, tail)
                        }
                    } else {
                        (None, None, after_pct)
                    }
                } else {
                    (None, None, after_pct)
                };
                let (transferred, rate) = parse_transfer_tail(tail);
                return Some(ParsedLine {
                    remote,
                    phase: phase.to_owned(),
                    percent: Some(pct),
                    done,
                    total,
                    unit: infer_unit(phase),
                    transferred,
                    rate,
                    detail: None,
                    phase_done,
                });
            }
        }
    }

    // Case 2: Indeterminate counter `<done>[, <bytes> | <rate>]` (e.g. `Enumerating objects: 1234`)
    let (first_token, tail) = match body.split_once(',') {
        Some((head, rest)) => (head.trim(), rest),
        None => (body, ""),
    };
    if !first_token.is_empty() && first_token.bytes().all(|b| b.is_ascii_digit()) {
        if let Ok(done) = first_token.parse::<u64>() {
            let (transferred, rate) = parse_transfer_tail(tail);
            return Some(ParsedLine {
                remote,
                phase: phase.to_owned(),
                percent: None,
                done: Some(done),
                total: None,
                unit: infer_unit(phase),
                transferred,
                rate,
                detail: None,
                phase_done,
            });
        }
    }

    None
}

/// Splits arbitrarily chunked Git stderr streams on `\r` and `\n`, keeping partial
/// lines across chunk boundaries.
#[derive(Default)]
pub struct LineSplitter {
    buf: String,
}

impl LineSplitter {
    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        let mut lines = Vec::new();
        for ch in chunk.chars() {
            if ch == '\r' || ch == '\n' {
                if !self.buf.is_empty() {
                    lines.push(std::mem::take(&mut self.buf));
                }
            } else {
                // Guard against unbounded memory if a command outputs megabytes without newlines.
                if self.buf.len() < 4096 {
                    self.buf.push(ch);
                }
            }
        }
        lines
    }

    pub fn flush(&mut self) -> Option<String> {
        (!self.buf.is_empty()).then(|| std::mem::take(&mut self.buf))
    }
}

/// Stateful, throttled tracker that turns Git stderr chunks and Gall counter updates
/// into smooth, honest [`GitProgress`] events.
pub struct ProgressTracker {
    id: String,
    operation: String,
    pipeline: Pipeline,
    cancellable: bool,
    started: Instant,
    min_interval: Duration,
    last_emit: Option<Instant>,
    last_phase: String,
    last_determinate: Option<bool>,
    max_fraction: f64,
    last_snapshot: Option<GitProgress>,
    splitter: LineSplitter,
    sink: Box<dyn FnMut(GitProgress) + Send>,
}

impl ProgressTracker {
    pub fn new(
        id: impl Into<String>,
        operation: impl Into<String>,
        pipeline: Pipeline,
        cancellable: bool,
        sink: impl FnMut(GitProgress) + Send + 'static,
    ) -> Self {
        Self::new_at(
            id,
            operation,
            pipeline,
            cancellable,
            DEFAULT_THROTTLE,
            Instant::now(),
            sink,
        )
    }

    pub fn new_at(
        id: impl Into<String>,
        operation: impl Into<String>,
        pipeline: Pipeline,
        cancellable: bool,
        min_interval: Duration,
        started: Instant,
        sink: impl FnMut(GitProgress) + Send + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            operation: operation.into(),
            pipeline,
            cancellable,
            started,
            min_interval,
            last_emit: None,
            last_phase: String::new(),
            last_determinate: None,
            max_fraction: 0.0,
            last_snapshot: None,
            splitter: LineSplitter::default(),
            sink: Box::new(sink),
        }
    }

    /// Emits the initial `t = 0` snapshot immediately so the UI renders without delay.
    pub fn start(&mut self, phase: impl Into<String>) {
        self.start_at(phase, self.started);
    }

    pub fn start_at(&mut self, phase: impl Into<String>, now: Instant) {
        let phase = phase.into();
        let snapshot = GitProgress {
            id: self.id.clone(),
            operation: self.operation.clone(),
            phase,
            fraction: None,
            percent: None,
            done: None,
            total: None,
            unit: None,
            transferred: None,
            rate: None,
            remote: false,
            phase_index: None,
            phase_count: None,
            detail: None,
            elapsed_ms: now.saturating_duration_since(self.started).as_millis() as u64,
            cancellable: self.cancellable,
            finished: false,
        };
        self.emit_snapshot(snapshot, now, true);
    }

    /// Transitions immediately to a new named phase (indeterminate until totals arrive).
    pub fn set_phase(&mut self, phase: impl Into<String>, detail: Option<String>) {
        self.set_phase_at(phase, detail, Instant::now());
    }

    pub fn set_phase_at(&mut self, phase: impl Into<String>, detail: Option<String>, now: Instant) {
        let phase = phase.into();
        let snapshot = GitProgress {
            id: self.id.clone(),
            operation: self.operation.clone(),
            phase,
            fraction: None,
            percent: None,
            done: None,
            total: None,
            unit: None,
            transferred: None,
            rate: None,
            remote: false,
            phase_index: None,
            phase_count: None,
            detail,
            elapsed_ms: now.saturating_duration_since(self.started).as_millis() as u64,
            cancellable: self.cancellable,
            finished: false,
        };
        self.emit_snapshot(snapshot, now, true);
    }

    /// Reports real item counts from Gall (e.g. `.worktreeinclude` files, blame lines,
    /// rebase conflict check commits, absorb hunks). When `total` is `Some(t)` with `t > 0`,
    /// computes real determinate progress; when `total` is `None`, emits honest indeterminate
    /// progress with live `done` count.
    pub fn report(
        &mut self,
        phase: impl Into<String>,
        done: Option<u64>,
        total: Option<u64>,
        unit: Option<&str>,
        detail: Option<String>,
    ) {
        self.report_at(phase, done, total, unit, detail, Instant::now());
    }

    pub fn report_at(
        &mut self,
        phase: impl Into<String>,
        done: Option<u64>,
        total: Option<u64>,
        unit: Option<&str>,
        detail: Option<String>,
        now: Instant,
    ) {
        let phase = phase.into();
        let raw_fraction = match (done, total) {
            (Some(d), Some(t)) if t > 0 => Some((d as f64 / t as f64).clamp(0.0, 1.0)),
            _ => None,
        };
        let (fraction, phase_index, phase_count) =
            self.combine_fraction(&phase, false, raw_fraction);
        let percent = fraction.map(|f| (f * 100.0).round().clamp(0.0, 100.0) as u8);
        let complete = matches!((done, total), (Some(d), Some(t)) if t > 0 && d >= t);
        let snapshot = GitProgress {
            id: self.id.clone(),
            operation: self.operation.clone(),
            phase,
            fraction,
            percent,
            done,
            total,
            unit: unit.map(str::to_owned),
            transferred: None,
            rate: None,
            remote: false,
            phase_index,
            phase_count,
            detail,
            elapsed_ms: now.saturating_duration_since(self.started).as_millis() as u64,
            cancellable: self.cancellable,
            finished: false,
        };
        self.emit_snapshot(snapshot, now, complete);
    }

    /// Feeds a raw stderr chunk from Git, splitting on `\r`/`\n` and emitting throttled
    /// structured progress updates.
    pub fn feed_chunk(&mut self, chunk: &str) {
        self.feed_chunk_at(chunk, Instant::now());
    }

    pub fn feed_chunk_at(&mut self, chunk: &str, now: Instant) {
        let lines = self.splitter.push(chunk);
        for line in lines {
            if let Some(parsed) = parse_line(&line) {
                self.apply_parsed(parsed, now);
            }
        }
    }

    /// Flushes any trailing buffered line and emits a final terminal update (`finished: true`).
    pub fn finish(&mut self) {
        self.finish_at(Instant::now());
    }

    pub fn finish_at(&mut self, now: Instant) {
        if let Some(tail) = self.splitter.flush() {
            if let Some(parsed) = parse_line(&tail) {
                self.apply_parsed(parsed, now);
            }
        }
        let elapsed_ms = now.saturating_duration_since(self.started).as_millis() as u64;
        let mut final_snap = self.last_snapshot.clone().unwrap_or_else(|| GitProgress {
            id: self.id.clone(),
            operation: self.operation.clone(),
            phase: self.last_phase.clone(),
            fraction: None,
            percent: None,
            done: None,
            total: None,
            unit: None,
            transferred: None,
            rate: None,
            remote: false,
            phase_index: None,
            phase_count: None,
            detail: None,
            elapsed_ms,
            cancellable: self.cancellable,
            finished: true,
        });
        final_snap.elapsed_ms = elapsed_ms;
        final_snap.finished = true;
        self.emit_snapshot(final_snap, now, true);
    }

    fn apply_parsed(&mut self, parsed: ParsedLine, now: Instant) {
        let raw_fraction = match (parsed.done, parsed.total, parsed.percent) {
            (Some(d), Some(t), _) if t > 0 => Some((d as f64 / t as f64).clamp(0.0, 1.0)),
            (_, _, Some(p)) => Some((f64::from(p) / 100.0).clamp(0.0, 1.0)),
            _ => None,
        };
        let (fraction, phase_index, phase_count) =
            self.combine_fraction(&parsed.phase, parsed.remote, raw_fraction);
        let percent = fraction.map(|f| (f * 100.0).round().clamp(0.0, 100.0) as u8);
        let snapshot = GitProgress {
            id: self.id.clone(),
            operation: self.operation.clone(),
            phase: parsed.phase,
            fraction,
            percent,
            done: parsed.done,
            total: parsed.total,
            unit: parsed.unit,
            transferred: parsed.transferred,
            rate: parsed.rate,
            remote: parsed.remote,
            phase_index,
            phase_count,
            detail: parsed.detail,
            elapsed_ms: now.saturating_duration_since(self.started).as_millis() as u64,
            cancellable: self.cancellable,
            finished: false,
        };
        self.emit_snapshot(snapshot, now, parsed.phase_done);
    }

    fn combine_fraction(
        &mut self,
        phase: &str,
        remote: bool,
        raw_fraction: Option<f64>,
    ) -> (Option<f64>, Option<u32>, Option<u32>) {
        let Some(raw) = raw_fraction else {
            // Never fabricate a percentage for an indeterminate phase.
            return (None, None, None);
        };
        if let Some((start, end, idx, count)) = self.pipeline.band(phase, remote) {
            let mapped = (start + (end - start) * raw).clamp(0.0, 1.0);
            let monotonic = mapped.max(self.max_fraction);
            self.max_fraction = monotonic;
            (Some(monotonic), Some(idx), Some(count))
        } else {
            (Some(raw), None, None)
        }
    }

    fn emit_snapshot(&mut self, snapshot: GitProgress, now: Instant, force: bool) {
        let phase_changed = snapshot.phase != self.last_phase;
        let determinate = snapshot.fraction.is_some();
        let det_changed = self.last_determinate != Some(determinate);
        let elapsed_ok = self
            .last_emit
            .is_none_or(|prev| now.saturating_duration_since(prev) >= self.min_interval);
        if force || snapshot.finished || phase_changed || det_changed || elapsed_ok {
            self.last_phase = snapshot.phase.clone();
            self.last_determinate = Some(determinate);
            self.last_emit = Some(now);
            self.last_snapshot = Some(snapshot.clone());
            (self.sink)(snapshot);
        } else {
            // Keep latest snapshot cached so `finish()` has the freshest values.
            self.last_snapshot = Some(snapshot);
        }
    }
}

thread_local! {
    static ACTIVE_TRACKER: RefCell<Option<Arc<Mutex<ProgressTracker>>>> = const { RefCell::new(None) };
}

/// Fast-path check (~1 ns) to see if the current thread is tracking Git progress.
pub fn is_active() -> bool {
    ACTIVE_TRACKER.with(|cell| cell.borrow().is_some())
}

/// Returns the active progress tracker for the current thread, if any.
pub fn active() -> Option<Arc<Mutex<ProgressTracker>>> {
    ACTIVE_TRACKER.with(|cell| cell.borrow().clone())
}

struct TrackerGuard {
    previous: Option<Arc<Mutex<ProgressTracker>>>,
    current: Arc<Mutex<ProgressTracker>>,
}

impl Drop for TrackerGuard {
    fn drop(&mut self) {
        if let Ok(mut tracker) = self.current.lock() {
            tracker.finish();
        }
        ACTIVE_TRACKER.with(|cell| {
            *cell.borrow_mut() = self.previous.take();
        });
    }
}

/// Installs `tracker` on the current thread for the duration of `f`, calling `finish()`
/// automatically when `f` returns or unwinds.
pub fn with_tracker<R>(tracker: Arc<Mutex<ProgressTracker>>, f: impl FnOnce() -> R) -> R {
    let previous = ACTIVE_TRACKER.with(|cell| cell.borrow_mut().replace(tracker.clone()));
    let _guard = TrackerGuard {
        previous,
        current: tracker,
    };
    f()
}

/// Reports a named phase transition if a tracker is active on the current thread.
pub fn report_phase(phase: &str, detail: Option<String>) {
    if let Some(tracker) = active() {
        if let Ok(mut guard) = tracker.lock() {
            guard.set_phase(phase, detail);
        }
    }
}

/// Reports real item counts if a tracker is active on the current thread.
pub fn report_counts(
    phase: &str,
    done: u64,
    total: Option<u64>,
    unit: &str,
    detail: Option<String>,
) {
    if let Some(tracker) = active() {
        if let Ok(mut guard) = tracker.lock() {
            guard.report(phase, Some(done), total, Some(unit), detail);
        }
    }
}

/// Feeds a raw stderr chunk if a tracker is active on the current thread.
pub fn feed_stderr(chunk: &str) {
    if let Some(tracker) = active() {
        if let Ok(mut guard) = tracker.lock() {
            guard.feed_chunk(chunk);
        }
    }
}

/// Runs `cmd` while streaming stderr incrementally into `tracker` and collecting
/// full stdout + stderr into a standard [`Output`]. Only called when a progress
/// tracker is active, so uninstrumented fast paths incur zero overhead.
pub(super) fn run_streaming_output(
    cmd: &mut Command,
    tracker: &Arc<Mutex<ProgressTracker>>,
) -> std::io::Result<Output> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let mut stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("missing stdout pipe"))?;
    let mut stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| std::io::Error::other("missing stderr pipe"))?;
    let stdout_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let mut stderr_buf = Vec::new();
    let mut chunk = [0u8; 4096];
    while let Ok(n) = stderr_pipe.read(&mut chunk) {
        if n == 0 {
            break;
        }
        stderr_buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&chunk[..n]);
        if let Ok(mut guard) = tracker.lock() {
            guard.feed_chunk(&text);
        }
    }
    let status = child.wait()?;
    let stdout = stdout_thread.join().unwrap_or_default();
    Ok(Output {
        status,
        stdout,
        stderr: stderr_buf,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_git_progress_lines_and_remote_sideband() {
        let p = parse_line("remote: Enumerating objects: 1234, done.").unwrap();
        assert!(p.remote);
        assert_eq!(p.phase, "Enumerating objects");
        assert_eq!(p.percent, None);
        assert_eq!(p.done, Some(1234));
        assert_eq!(p.total, None);
        assert_eq!(p.unit.as_deref(), Some("objects"));
        assert!(p.phase_done);

        let p = parse_line("remote: Counting objects:  45% (556/1234)   \x1b[K").unwrap();
        assert!(p.remote);
        assert_eq!(p.phase, "Counting objects");
        assert_eq!(p.percent, Some(45));
        assert_eq!(p.done, Some(556));
        assert_eq!(p.total, Some(1234));
        assert!(!p.phase_done);

        let p = parse_line("Receiving objects:  62% (765/1234), 12.40 MiB | 4.20 MiB/s").unwrap();
        assert!(!p.remote);
        assert_eq!(p.phase, "Receiving objects");
        assert_eq!(p.percent, Some(62));
        assert_eq!(p.done, Some(765));
        assert_eq!(p.total, Some(1234));
        assert_eq!(p.transferred.as_deref(), Some("12.40 MiB"));
        assert_eq!(p.rate.as_deref(), Some("4.20 MiB/s"));
        assert!(!p.phase_done);

        let p = parse_line("Writing objects: 100% (80/80), 1.50 MiB | 2.10 MiB/s, done.").unwrap();
        assert_eq!(p.phase, "Writing objects");
        assert_eq!(p.percent, Some(100));
        assert_eq!(p.done, Some(80));
        assert_eq!(p.total, Some(80));
        assert_eq!(p.transferred.as_deref(), Some("1.50 MiB"));
        assert_eq!(p.rate.as_deref(), Some("2.10 MiB/s"));
        assert!(p.phase_done);

        let p = parse_line("Resolving deltas:  50% (100/200)").unwrap();
        assert_eq!(p.phase, "Resolving deltas");
        assert_eq!(p.unit.as_deref(), Some("deltas"));
        assert_eq!(p.done, Some(100));
        assert_eq!(p.total, Some(200));

        let p = parse_line("Updating files:  35% (1750/5000)").unwrap();
        assert_eq!(p.phase, "Updating files");
        assert_eq!(p.unit.as_deref(), Some("files"));
        assert_eq!(p.percent, Some(35));

        let p = parse_line("Rebasing (3/12)").unwrap();
        assert_eq!(p.phase, "Rebasing");
        assert_eq!(p.done, Some(3));
        assert_eq!(p.total, Some(12));
        assert_eq!(p.percent, Some(25));
        assert_eq!(p.unit.as_deref(), Some("commits"));

        // Diagnostic errors must not be mistaken for progress lines.
        assert!(parse_line("fatal: repository 'foo' not found").is_none());
        assert!(parse_line("error: failed to push some refs").is_none());
        assert!(parse_line("warning: redirecting to https://example.com").is_none());
    }

    #[test]
    fn splitter_handles_cr_overwrites_and_chunk_boundaries() {
        let mut splitter = LineSplitter::default();
        assert!(splitter.push("Receiving objects:  10%").is_empty());
        let lines = splitter.push(" (10/100)\rReceiving objects:  50% (50/100)\rReceiving ");
        assert_eq!(
            lines,
            vec![
                "Receiving objects:  10% (10/100)",
                "Receiving objects:  50% (50/100)"
            ]
        );
        let more = splitter.push("objects: 100% (100/100), done.\n");
        assert_eq!(more, vec!["Receiving objects: 100% (100/100), done."]);
        assert_eq!(splitter.flush(), None);
    }

    #[test]
    fn multi_phase_fetch_stays_honest_on_indeterminate_and_monotonic_on_determinate() {
        let events: Arc<Mutex<Vec<GitProgress>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let t0 = Instant::now();
        let mut tracker = ProgressTracker::new_at(
            "job-1",
            "fetch",
            Pipeline::Fetch,
            true,
            Duration::from_millis(50),
            t0,
            move |ev| sink.lock().unwrap().push(ev),
        );
        tracker.start_at("Fetching", t0);
        tracker.feed_chunk_at(
            "remote: Enumerating objects: 500\r",
            t0 + Duration::from_millis(10),
        );
        tracker.feed_chunk_at(
            "remote: Counting objects:  50% (250/500)\r",
            t0 + Duration::from_millis(20),
        );
        tracker.feed_chunk_at(
            "remote: Compressing objects: 100% (200/200), done.\n",
            t0 + Duration::from_millis(30),
        );
        tracker.feed_chunk_at(
            "Receiving objects:  50% (250/500), 2.00 MiB | 4.00 MiB/s\r",
            t0 + Duration::from_millis(40),
        );
        tracker.feed_chunk_at(
            "Resolving deltas: 100% (100/100), done.\n",
            t0 + Duration::from_millis(50),
        );
        tracker.finish_at(t0 + Duration::from_millis(60));

        let got = events.lock().unwrap();
        // 1. Start is indeterminate at t=0.
        assert_eq!(got[0].phase, "Fetching");
        assert_eq!(got[0].fraction, None);
        assert_eq!(got[0].percent, None);
        assert_eq!(got[0].elapsed_ms, 0);

        // 2. Enumerating objects has live count but stays indeterminate (no fabricated %).
        assert_eq!(got[1].phase, "Enumerating objects");
        assert_eq!(got[1].done, Some(500));
        assert_eq!(got[1].total, None);
        assert_eq!(got[1].fraction, None);
        assert_eq!(got[1].percent, None);

        // 3. Subsequent determinate phases advance monotonically without resetting to 0%.
        let det_fractions: Vec<f64> = got[2..].iter().filter_map(|ev| ev.fraction).collect();
        assert!(det_fractions.len() >= 4);
        for pair in det_fractions.windows(2) {
            assert!(
                pair[1] >= pair[0],
                "progress must not regress: {} -> {}",
                pair[0],
                pair[1]
            );
        }
        assert!(got.last().unwrap().finished);
    }

    #[test]
    fn throttle_coalesces_rapid_same_phase_updates_and_flushes_phase_changes() {
        let events: Arc<Mutex<Vec<GitProgress>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let t0 = Instant::now();
        let mut tracker = ProgressTracker::new_at(
            "job-2",
            "checkout",
            Pipeline::Single,
            false,
            Duration::from_millis(60),
            t0,
            move |ev| sink.lock().unwrap().push(ev),
        );
        tracker.start_at("Switching branch", t0);
        // Phase change from Switching branch -> Updating files emits immediately even at +5ms.
        tracker.feed_chunk_at(
            "Updating files:  10% (10/100)\r",
            t0 + Duration::from_millis(5),
        );
        // Rapid same-phase updates within 60ms are coalesced.
        tracker.feed_chunk_at(
            "Updating files:  20% (20/100)\r",
            t0 + Duration::from_millis(20),
        );
        tracker.feed_chunk_at(
            "Updating files:  30% (30/100)\r",
            t0 + Duration::from_millis(40),
        );
        // After 60ms interval elapses, next update emits.
        tracker.feed_chunk_at(
            "Updating files:  40% (40/100)\r",
            t0 + Duration::from_millis(70),
        );
        // `, done.` forces immediate emit even within throttle window.
        tracker.feed_chunk_at(
            "Updating files: 100% (100/100), done.\n",
            t0 + Duration::from_millis(80),
        );
        tracker.finish_at(t0 + Duration::from_millis(85));

        let got = events.lock().unwrap();
        let percents: Vec<Option<u8>> = got.iter().map(|e| e.percent).collect();
        assert_eq!(
            percents,
            vec![None, Some(10), Some(40), Some(100), Some(100)]
        );
        assert!(got.last().unwrap().finished);
    }

    #[test]
    fn thread_local_tracker_scoping_clears_and_finishes_on_exit() {
        assert!(!is_active());
        let events: Arc<Mutex<Vec<GitProgress>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let tracker = Arc::new(Mutex::new(ProgressTracker::new(
            "job-3",
            "blame",
            Pipeline::Single,
            true,
            move |ev| sink.lock().unwrap().push(ev),
        )));
        with_tracker(tracker, || {
            assert!(is_active());
            report_counts("Blaming file", 25, Some(100), "lines", Some("a.rs".into()));
        });
        assert!(!is_active());
        let got = events.lock().unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].percent, Some(25));
        assert_eq!(got[0].done, Some(25));
        assert_eq!(got[0].total, Some(100));
        assert!(got[1].finished);
    }
}
