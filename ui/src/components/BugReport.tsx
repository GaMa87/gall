import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api } from "../api/ipc";
import { getState } from "../state/store";
import * as A from "../state/actions";
import { join } from "../core/path";
import { issueLink, reportBodyPreview, reproduction } from "../core/bugReport";
import { flattenScreenshot, validateMedia, type ReportMedia } from "../core/reportMedia";
import { beginReportDraft, completeReportDraft, getReportDraft, subscribeReportDraft, updateReportDraft } from "../core/reportDraft";
import { Icon } from "./Icon";
import { ReportImageEditor, useBlobUrl } from "./ReportImageEditor";
import { startReportCapture } from "./ReportCapture";
import "../styles/bugReport.css";

let sequence = 0;
const mediaName = (blob: Blob, kind: "image" | "video") => `gall-${kind === "image" ? "screenshot" : "recording"}-${++sequence}.${kind === "image" ? "png" : blob.type.startsWith("video/mp4") ? "mp4" : "webm"}`;

function Attachment({ media, busy, edit, remove, copy, save }: {
  media: ReportMedia; busy: boolean; edit: () => void; remove: () => void; copy: () => void; save: () => void;
}) {
  const url = useBlobUrl(media.blob);
  return <article className="report-attachment">
    <div className="report-attachment-preview">{media.kind === "image" ? <img src={url} alt="Screenshot attachment preview" /> : <video src={url} controls preload="auto" aria-label="Recording attachment preview" />}</div>
    <div className="report-attachment-info"><strong>{media.name}</strong><span>{(media.blob.size / 1024 / 1024).toFixed(2)} MiB · local only</span></div>
    <div className="report-attachment-actions">
      {media.kind === "image" && <><button className="btn" disabled={busy} onClick={edit}>Annotate</button><button className="btn" disabled={busy} onClick={copy}>Copy screenshot</button></>}
      <button className="btn" disabled={busy} onClick={save}>Save attachment</button>
      <button className="btn report-remove" disabled={busy} onClick={remove} aria-label={`Remove ${media.name}`} title="Remove attachment"><Icon name="trash" /></button>
    </div>
  </article>;
}

export function BugReport({ previous = false, resume = false }: { previous?: boolean; resume?: boolean }) {
  const form = useSyncExternalStore(subscribeReportDraft, getReportDraft);
  const [review, setReview] = useState(false);
  const [report, setReport] = useState<Awaited<ReturnType<typeof api.bugReportPrepare>>>();
  const lastReport = useRef<typeof report>(undefined);
  const [error, setError] = useState("");
  const [status, setStatus] = useState("");
  const [retry, setRetry] = useState(0);
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState<string>();
  const [exporting, setExporting] = useState<"report" | ReportMedia>();
  const [exportPath, setExportPath] = useState("");
  const fileInput = useRef<HTMLInputElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const lock = useRef(false);
  const snapshot = useRef((() => {
    const state = getState();
    return { viewMode: state.session?.layout, activeDialog: "bugReport", tabCount: state.session?.browsers.reduce((n, b) => n + b.tabs.length, 0), paneCount: state.session?.browsers.length ?? 0, taskCount: Object.keys(state.tasks).length };
  })());
  const update = updateReportDraft;
  const description = reproduction(form.description, form.steps, form.expected);
  useLayoutEffect(() => beginReportDraft(resume), [resume]);
  useEffect(() => {
    let active = true;
    setReport(undefined);
    // Typing stays local; diagnostics are prepared after a short pause, not on every keystroke.
    const timer = setTimeout(() => {
      api.bugReportPrepare(description, form.includeLog, snapshot.current, previous).then((next) => {
        if (active) { lastReport.current = next; setReport(next); }
      }).catch((e) => { if (active) setError(`Could not prepare diagnostics: ${String(e)}`); });
    }, 250);
    return () => { active = false; clearTimeout(timer); };
  }, [description, form.includeLog, previous, retry]);
  useEffect(() => { if (review || editing || exporting) heading.current?.focus(); }, [review, editing, exporting]);
  async function attempt(action: () => Promise<void>) {
    if (lock.current) return;
    lock.current = true; setBusy(true); setError(""); setStatus("");
    try { await action(); } catch (e) { setError(String(e)); }
    finally { lock.current = false; setBusy(false); }
  }
  async function addFile(file: File) {
    await attempt(async () => {
      if (getReportDraft().capturing) throw new Error("Finish the current capture before adding a file.");
      if (getReportDraft().media.length >= 3) throw new Error("Keep up to three attachments. Remove one to add another.");
      const kind = validateMedia(file);
      const blob = kind === "image" ? await flattenScreenshot(file, []) : file;
      const media: ReportMedia = { id: crypto.randomUUID(), name: mediaName(blob, kind), blob, kind };
      update((current) => ({ media: [...current.media, media] }));
      if (kind === "image") setEditing(media.id);
      else setStatus("Video added. Play it back and check every frame for private details before sharing.");
    });
  }
  function capture(mode: "screenshot" | "video") {
    const restore = { previous, resume: true };
    update({ captureError: "" });
    const started = startReportCapture({ mode, finish: (blob, message) => {
      update({ capturing: false });
      if (blob) {
        const kind: ReportMedia["kind"] = mode === "screenshot" ? "image" : "video";
        if (getReportDraft().media.length >= 3) message = "The capture could not be added: three attachments are already present. Remove one and capture again.";
        else update((current) => ({ media: [...current.media, { id: crypto.randomUUID(), name: mediaName(blob, kind), blob, kind }] }));
      }
      update({ captureError: message ?? "" });
      const overlay = getState().overlay;
      if (!overlay) A.openOverlay({ type: "bugReport", ...restore });
      else if (overlay.type !== "bugReport") A.notify(message || (blob ? "Capture added to your bug report." : "Capture cancelled."), message ? "warn" : "info", { label: "View report", run: () => A.openOverlay({ type: "bugReport", ...restore }) });
    } });
    if (started) { update({ capturing: true }); A.openOverlay(null); }
    else setError("A capture is already in progress or unavailable. Finish it before starting another.");
  }
  const body = report?.body ?? "";
  const link = report ? issueLink(form.title.trim() || report.title, body) : undefined;
  const budgetReport = report ?? lastReport.current;
  const budget = budgetReport ? issueLink(form.title.trim() || budgetReport.title, reportBodyPreview(budgetReport, description)) : undefined;
  const close = () => { if (!lock.current) A.openOverlay(null); };
  function showExport(target: "report" | ReportMedia) {
    const home = getState().info?.home;
    setExportPath(home ? join(home, target === "report" ? `gall-bug-report-${new Date().toISOString().replace(/[:.]/g, "-")}.zip` : target.name) : "");
    setExporting(target); setError(""); setStatus("");
  }
  const edited = form.media.find((media) => media.id === editing);
  return <div className="backdrop center bug-report-backdrop" onKeyDown={(e) => {
    e.stopPropagation();
    if (e.key === "Escape" && !busy) {
      e.preventDefault();
      if (exporting) setExporting(undefined);
      else if (editing) setEditing(undefined);
      else close();
    }
    if (e.key === "Tab") {
      const fields = Array.from(e.currentTarget.querySelectorAll<HTMLElement>("button:not(:disabled), textarea, input:not([hidden]), summary, [tabindex='0'], video[controls]")).filter((field) => field.getClientRects().length > 0);
      const at = fields.indexOf(document.activeElement as HTMLElement);
      if (fields.length && ((e.shiftKey && at <= 0) || (!e.shiftKey && (at === -1 || at === fields.length - 1)))) {
        e.preventDefault(); fields[e.shiftKey ? fields.length - 1 : 0].focus();
      }
    }
  }}>
    <section className="bug-report" role="dialog" aria-modal="true" aria-labelledby="bug-report-title" aria-describedby="bug-report-subtitle" aria-busy={busy}>
      <header className="report-header">
        <div className="report-brand"><Icon name="activity" size={22} /></div>
        <div><span className="report-eyebrow">MAKE GALL BETTER</span><h2 id="bug-report-title" tabIndex={-1} ref={heading}>{exporting ? exporting === "report" ? "Save your report" : "Save attachment" : editing ? "A picture makes it clear" : review ? "Ready when you are" : "Something not quite right?"}</h2>
          <p id="bug-report-subtitle">{review ? "One last look. You're in control of what you share." : "A few words or a quick capture can make a big difference."}</p></div>
        <button className="report-close" aria-label="Close bug report" disabled={busy} onClick={close}><Icon name="close" size={20} /></button>
      </header>
      <div className="report-scroll">
        {previous && <div className="report-notice"><Icon name="history" /><p>Diagnostics from the previous improperly closed session are included. This does not necessarily mean Gall crashed.</p></div>}
        {exporting ? <form id="report-export" className="bug-report-export" onSubmit={(e) => {
          e.preventDefault(); void attempt(async () => {
            if (exporting === "report") {
              if (!report) return;
              const attachments = await Promise.all(form.media.map(async (media) => ({
                name: media.name, bytes: Array.from(new Uint8Array(await media.blob.arrayBuffer())), mime: media.blob.type.split(";")[0],
              })));
              await api.bugReportExport(exportPath, report.fullBody, attachments);
            }
            else await api.bugReportSaveMedia(exportPath, Array.from(new Uint8Array(await exporting.blob.arrayBuffer())), exporting.blob.type.split(";")[0]);
            setStatus(`${exporting === "report" ? "Report" : "Attachment"} saved to ${exportPath}.`);
            setExporting(undefined);
          });
        }}>
          <label htmlFor="report-path">Absolute file path</label><input id="report-path" autoFocus value={exportPath} disabled={busy} onChange={(e) => setExportPath(e.target.value)} spellCheck={false} />
          <p className="report-hint">Only a new file will be created. Existing files are never replaced.</p>
          {exporting === "report" && <>
            <p className="report-bundle-summary"><Icon name="archive" /> One ZIP with your report{form.media.length === 1 ? " and your attachment" : form.media.length > 1 ? ` and all ${form.media.length} attachments` : ""}. Ready to attach on GitHub.</p>
            {form.capturing && <p className="report-hint">Finish or cancel the current capture before saving the ZIP.</p>}
            <label className="report-log"><input type="checkbox" checked={form.includeLog} onChange={(e) => update({ includeLog: e.target.checked })} disabled={busy} /><span>Include extra diagnostics <small>Saved file only: recent actions, UI context and sanitized logs. Never copied to GitHub. Automatic redaction can miss private details.</small></span></label>
            {form.includeLog && <details className="report-full"><summary>Preview saved diagnostics</summary><pre tabIndex={0}>{report?.fullBody ?? "Preparing diagnostics..."}</pre></details>}
          </>}
        </form> : edited ? <ReportImageEditor blob={edited.blob} cancel={() => setEditing(undefined)} onBusy={(value) => { lock.current = value; setBusy(value); }} save={(blob) => {
          update((current) => ({ media: current.media.map((media) => media.id === edited.id ? { ...media, blob } : media) })); setEditing(undefined);
          setStatus("Changes applied. Only the flattened image is kept; covered pixels are removed.");
        }} /> : <>
          {!review ? <>
            <label className="report-label" htmlFor="report-summary">In a few words <span>optional</span></label>
            <input id="report-summary" autoFocus maxLength={160} value={form.title} onChange={(e) => update({ title: e.target.value })} placeholder="For example: the file list stops updating" />
            <label className="report-label" htmlFor="report-description">What happened? <span>optional</span></label>
            <textarea id="report-description" aria-describedby="report-budget report-budget-hint" maxLength={6000} value={form.description} onChange={(e) => update({ description: e.target.value })} placeholder="What were you doing, and what went wrong? Write it your way." />
            <div className={`report-budget${budget?.needsAttachment ? " over-budget" : ""}`}>
              <span id="report-budget" aria-live="polite">{budget ? budget.needsAttachment ? "Too long to auto-fill on GitHub" : `${budget.remaining.toLocaleString()} link characters left for auto-fill` : "Calculating GitHub space..."}</span>
              {budget?.needsAttachment && <button className="link-btn" disabled={!report || busy || form.capturing} onClick={() => showExport("report")}>Save ZIP instead <Icon name="download" size={13} /></button>}
            </div>
            <p id="report-budget-hint" className="report-budget-hint">{budget?.needsAttachment ? "Save the full report with its images and videos, then attach the ZIP to your GitHub issue." : "Counts the encoded GitHub link, including the title and environment. Accents and emoji use more space."}</p>
            <details className="report-more"><summary>Add steps or expected behavior <span>optional</span></summary>
              <label className="report-label" htmlFor="report-steps">How can we reproduce it?</label><textarea id="report-steps" maxLength={2000} value={form.steps} onChange={(e) => update({ steps: e.target.value })} placeholder={"1. Open a folder\n2. ..."} />
              <label className="report-label" htmlFor="report-expected">What should happen instead?</label><textarea id="report-expected" maxLength={1000} value={form.expected} onChange={(e) => update({ expected: e.target.value })} />
            </details>
          </> : <div className="report-review-intro"><span className="report-review-check"><Icon name="check" size={22} /></span><div><h3>{form.title.trim() || "Your bug report"}</h3><p>Your description, basic diagnostics and {form.media.length ? `${form.media.length} local attachment${form.media.length === 1 ? "" : "s"}` : "no attachments"}. Nothing has been uploaded.</p></div></div>}
          <section className="report-evidence" aria-labelledby="report-evidence-title">
            <div className="report-section-heading"><div><h3 id="report-evidence-title">{review ? "Check your attachments" : "Show the problem"}</h3><p>{review ? "Images and videos can reveal filenames and document contents." : "A screenshot for the details. A short clip for the sequence."}</p></div><span className="report-optional">OPTIONAL</span></div>
            {!review && <div className="report-capture-actions">
              <button className="report-capture-button" disabled={busy || form.capturing || form.media.length >= 3} onClick={() => capture("screenshot")}><Icon name="image" size={22} /><strong>Capture Gall</strong><small>3-second countdown</small></button>
              <button className="report-capture-button" disabled={busy || form.capturing || form.media.length >= 3} onClick={() => capture("video")}><Icon name="video" size={22} /><strong>Record a clip</strong><small>Up to 20s · no audio</small></button>
              <button className="report-capture-button" disabled={busy || form.capturing || form.media.length >= 3} onClick={() => fileInput.current?.click()}><Icon name="plus" size={22} /><strong>Add a file</strong><small>Image or short video</small></button>
              <input ref={fileInput} hidden type="file" accept="image/png,image/jpeg,image/webp,video/mp4,video/webm" onChange={(e) => { const file = e.target.files?.[0]; e.target.value = ""; if (file) void addFile(file); }} />
            </div>}
            <div className="report-attachments">{form.media.map((media) => <Attachment key={media.id} media={media} busy={busy} edit={() => setEditing(media.id)} remove={() => update((current) => ({ media: current.media.filter((item) => item.id !== media.id) }))} save={() => showExport(media)} copy={() => void attempt(async () => {
              await api.bugReportCopyScreenshot(Array.from(new Uint8Array(await media.blob.arrayBuffer())));
              setStatus("Screenshot copied. Paste it into GitHub after the report text. This replaces the clipboard text.");
            })} />)}</div>
            {!review && <p className="report-hint"><Icon name="lock" size={13} /> Captures stay on your device. Up to 3 files, 8 MiB each. Cover private details before sharing.</p>}
          </section>
          <section className="report-diagnostics">
            <div className="report-section-heading"><div><h3>Short report, useful details</h3><p>Only your description, basic environment and latest error are copied. Extra diagnostics are available in Save report.</p></div><Icon name="activity" size={19} /></div>
            {report ? <details className="report-full"><summary>Preview GitHub report</summary><pre tabIndex={0}>{body}</pre></details> : <p className="report-hint" role="status">{error ? "Diagnostics are not ready." : "Preparing diagnostics..."}</p>}
          </section>
          {review && <div className="report-handoff"><Icon name="open" size={20} /><div><h3>Finish on GitHub</h3><p>{link?.needsAttachment ? "This report is too long to fill in automatically. Choose Save report, then attach the ZIP on GitHub. We'll also copy the text as a backup." : "Your short report will already be filled in on GitHub. We'll also copy it as a backup."} Add any screenshots, clips or your saved ZIP, then review and submit. A GitHub account is required.</p><p>The short report and title are sent to GitHub when the page opens. Extra diagnostics stay in saved files only. Gall never uploads attachments; GitHub uploads them when you add them to the public issue.</p>{form.capturing && <p>Finish or cancel the current capture before opening GitHub.</p>}</div></div>}
        </>}
      </div>
      {(error || form.captureError || status) && <div className="report-feedback-area">
        {(error || form.captureError) && <div className="report-feedback report-error" role="alert"><Icon name="alert" /><span>{error || form.captureError}</span>{!report && <button className="btn" onClick={() => { setError(""); setRetry((n) => n + 1); }}>Retry diagnostics</button>}</div>}
        {status && <div className="report-feedback" role="status"><Icon name="check" /><span>{status}</span></div>}
      </div>}
      <footer className="report-footer">
        <span className="report-local"><Icon name="lock" size={13} /> {exporting ? "Saved only on this device" : "Nothing sent without you"}</span>
        <div className="report-footer-actions">{exporting ? <><button className="btn" disabled={busy} onClick={() => { setExporting(undefined); setError(""); setStatus(""); }}>Cancel</button><button className="btn primary" form="report-export" type="submit" disabled={busy || !exportPath.trim() || (exporting === "report" && (!report || form.capturing))}>{busy ? "Saving..." : exporting === "report" ? "Save report" : "Save attachment"}</button></> : editing ? <button className="btn" disabled={busy} onClick={() => setEditing(undefined)}>Back to report</button> : review ? <>
          <button className="btn" disabled={busy} onClick={() => { setReview(false); setStatus(""); }}>Back</button>
          <button className="btn" disabled={!report || busy || form.capturing} onClick={() => showExport("report")}>Save report...</button>
          <button className="btn" disabled={!report || busy} onClick={() => void attempt(async () => { await api.bugReportCopyText(body); setStatus("Report text copied. Paste it into the GitHub issue before copying a screenshot."); })}>Copy report</button>
          <button className="btn primary" disabled={!report || busy || form.capturing} onClick={() => void attempt(async () => {
            if (!report || !link) return;
            await api.bugReportCopyText(body);
            if (["bug-report", "bug-report-text"].includes((await api.startupArgs()).selftestMode)) document.documentElement.dataset.bugReportUrl = link.url;
            else await openUrl(link.url);
            completeReportDraft();
            setStatus(link.needsAttachment ? "GitHub opened. Attach your saved report ZIP, then review and submit. The next report will start fresh." : "GitHub opened with your short report filled in. You can still save attachments here; the next report will start fresh.");
          })}><Icon name="open" />{busy ? "Preparing..." : "Copy & open GitHub"}</button>
        </> : <>
          <button className="btn" disabled={busy} onClick={close}>Not now</button>
          <button className="btn primary" disabled={!report || busy} onClick={() => { setReview(true); setStatus(""); }} >Review report <Icon name="forward" /></button>
        </>}</div>
      </footer>
    </section>
  </div>;
}

export function PreviousBugReport() {
  useEffect(() => { let active = true; api.bugReportStatus().then((available) => { if (active && available) { void api.bugReportDismiss().catch(() => {}); A.notify("Gall's previous session did not close normally.", "warn", { label: "Report a bug", run: () => A.openOverlay({ type: "bugReport", previous: true }) }); } }).catch(() => {}); return () => { active = false; }; }, []);
  return null;
}
