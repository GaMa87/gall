import { useEffect, useRef, useState } from "react";
import { api } from "../api/ipc";
import { RECORD_SECONDS, recordGall } from "../core/reportMedia";
import { Icon } from "./Icon";

type Capture = { mode: "screenshot" | "video"; finish: (blob?: Blob, error?: string) => void };
type CaptureSession = Capture & { cancelled: boolean; control?: { stop: (discard?: boolean) => void } };
let begin: ((capture: Capture) => void) | undefined;
let activeCapture = false;

export function startReportCapture(capture: Capture) {
  if (!begin || activeCapture) return false;
  activeCapture = true; begin(capture); return true;
}

export function ReportCapture() {
  const [capture, setCapture] = useState<CaptureSession>();
  const [countdown, setCountdown] = useState(3);
  const [elapsed, setElapsed] = useState(0);
  const [recording, setRecording] = useState(false);
  const [finishing, setFinishing] = useState(false);
  const current = useRef<CaptureSession | undefined>(undefined);
  useEffect(() => {
    begin = (next) => {
      const session = { ...next, cancelled: false };
      current.current = session; setCountdown(3); setElapsed(0); setRecording(false); setFinishing(false); setCapture(session);
    };
    return () => {
      begin = undefined; activeCapture = false;
      if (current.current) { current.current.cancelled = true; current.current.control?.stop(true); current.current = undefined; }
    };
  }, []);
  useEffect(() => {
    if (!capture) return;
    let active = true;
    const finish = (blob?: Blob, error?: string) => {
      if (!active || capture.cancelled || current.current !== capture) return;
      current.current = undefined; activeCapture = false; setCapture(undefined); capture.finish(blob, error);
    };
    const snapshot = async () => new Blob([new Uint8Array(await api.bugReportCapture())], { type: "image/png" });
    const tick = setInterval(() => setCountdown((n) => Math.max(0, n - 1)), 1000);
    const start = setTimeout(() => {
      clearInterval(tick);
      void (async () => {
        try {
          if (capture.mode === "screenshot") {
            // Hide the capture controls and wait for the webview to paint before its snapshot.
            document.documentElement.dataset.reportSnapshot = "true";
            await new Promise((resolve) => setTimeout(resolve, 120));
            const blob = await snapshot();
            finish(blob);
          } else {
            const recorder = await recordGall(snapshot, (blob, error) => {
              finish(blob, error);
            });
            if (capture.cancelled || !active || current.current !== capture) recorder.stop(true);
            else { capture.control = recorder; setRecording(true); setElapsed(0); }
          }
        } catch (error) { finish(undefined, String(error)); }
        finally { delete document.documentElement.dataset.reportSnapshot; }
      })();
    }, 3100);
    return () => { active = false; capture.cancelled = true; clearInterval(tick); clearTimeout(start); capture.control?.stop(true); delete document.documentElement.dataset.reportSnapshot; };
  }, [capture]);
  useEffect(() => {
    if (!recording) return;
    const timer = setInterval(() => setElapsed((n) => Math.min(RECORD_SECONDS, n + 1)), 1000);
    return () => clearInterval(timer);
  }, [recording]);
  function cancel() {
    if (!capture || capture.cancelled) return;
    capture.cancelled = true; capture.control?.stop(true);
    current.current = undefined; activeCapture = false; setCapture(undefined); capture.finish();
  }
  function stop() { if (capture?.control && !finishing) { setFinishing(true); capture.control.stop(); } }
  useEffect(() => {
    if (!capture) return;
    const key = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || !e.shiftKey || e.altKey || !["Enter", "Backspace"].includes(e.key)) return;
      e.preventDefault(); e.stopImmediatePropagation();
      if (e.key === "Backspace") cancel();
      else stop();
    };
    window.addEventListener("keydown", key, true);
    return () => window.removeEventListener("keydown", key, true);
  }, [capture, finishing]);
  if (!capture) return null;
  return <aside className="report-capture" aria-label="Gall capture controls" onKeyDown={(e) => {
    e.stopPropagation();
    if (e.key === "Escape") { e.preventDefault(); cancel(); }
  }}>
    <span className={recording ? "report-record-dot" : "report-capture-icon"}><Icon name={capture.mode === "video" ? "video" : "image"} size={18} /></span>
    <div role="status"><strong>{finishing ? "Preparing clip..." : recording ? `Recording Gall · ${elapsed}s / ${RECORD_SECONDS}s` : countdown ? `${capture.mode === "video" ? "Recording" : "Screenshot"} in ${countdown}...` : "Preparing capture..."}</strong>
      <small>{capture.mode === "video" ? "Gall only · no audio · stays local" : "Arrange Gall to show the problem"}</small>
      <small>Ctrl/Cmd+Shift+Enter: stop · Ctrl/Cmd+Shift+Backspace: cancel</small></div>
    {recording && <button className="btn primary" disabled={finishing} onClick={stop}>Stop recording</button>}
    <button className="btn" onClick={cancel}>Cancel capture</button>
  </aside>;
}
