import { useEffect, useRef, useState } from "react";
import * as A from "../state/actions";
import { api } from "../api/ipc";
import type { PatchFormat, PatchPreview } from "../api/types";
import { platformLabel, primaryMod } from "../core/keymap";
import { canApply, effectiveFormat, hunkSummary, needsTarget, parseStrip, type FormatChoice } from "../core/patchImport";
import { basename } from "../core/path";
import { tt } from "../i18n";
import type { Overlay } from "../state/store";
import "./StandaloneMerge.css";

const CHOICES: { id: FormatChoice; label: string }[] = [
  { id: "auto", label: "Automatic" },
  { id: "unified", label: "Unified (diff -u)" },
  { id: "context", label: "Context (diff -c)" },
  { id: "normal", label: "Normal (diff)" },
];

/**
 * Applies a patch (unified, context or normal) to the files below a folder. The patch comes from a file or is pasted; its
 * format is recognised from its own marks and can be chosen; the preview lists every file with its hunks and offsets and every
 * problem together, and nothing can be applied while there is one or while the text and settings differ from the preview. A
 * failed write puts the files already written back, and the message says which could and which could not be.
 */
/** FNV-1a over the UTF-16 units: cheap, and enough to tell two texts of one length apart in a key. */
function hashText(text: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) h = Math.imul(h ^ text.charCodeAt(i), 0x01000193);
  return h >>> 0;
}

export function PatchImport({ root: start, text: given, file, back }: { root?: string; text?: string; file?: string; back?: Overlay }) {
  const [text, setText] = useState(given ?? ""), [root, setRoot] = useState(start ?? "");
  // The file the text was read from, shown until the text is edited.
  const [source, setSource] = useState("");
  const [choice, setChoice] = useState<FormatChoice>("auto"), [sniffed, setSniffed] = useState<PatchFormat | null>(null);
  const [stripText, setStripText] = useState(""), [target, setTarget] = useState("");
  const [preview, setPreview] = useState<{ result: PatchPreview; key: string } | null>(null);
  const [error, setError] = useState(""), [busy, setBusy] = useState(false), [done, setDone] = useState<string[] | null>(null);
  const panel = useRef<HTMLDivElement>(null), sequence = useRef(0);
  const format = effectiveFormat(choice, sniffed);
  const strip = parseStrip(stripText);
  // The settings and the text of the check, as one key (the text is stood for by its length and a hash of all of it, and the
  // backend also refuses an apply whose plan differs from the one checked).
  const keyOf = (f: PatchFormat | null) => JSON.stringify([text.length, hashText(text), root, f, strip, target]);
  const key = keyOf(format);
  const current = preview?.key === key;
  useEffect(() => { panel.current?.focus(); }, []);
  // While a check or an apply runs the fields are disabled and the focus would fall to the page: it comes back to the dialog.
  useEffect(() => { if (!busy && !panel.current?.contains(document.activeElement)) panel.current?.focus(); }, [busy]);
  // The format is read from the text as it changes, a little after the last key.
  useEffect(() => {
    // Any earlier answer is out of date, an empty text included.
    const mine = ++sequence.current;
    if (!text.trim()) { setSniffed(null); return; }
    const timer = setTimeout(() => { api.patchSniff(text.slice(0, 65536)).then((f) => { if (mine === sequence.current) setSniffed(f); }).catch(() => {}); }, 250);
    return () => clearTimeout(timer);
  }, [text]);

  const load = async (chosen: string) => {
    try { setText((await api.readTextFile(chosen)).text); setSource(chosen); setDone(null); setError(""); }
    catch (e) { setError(String(e instanceof Error ? e.message : e)); }
  };
  // Opened on a patch file (its menu in a folder): its text is read once.
  useEffect(() => { if (file) void load(file); }, [file]);
  const chooseFile = async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const chosen = await open({ title: tt("Choose a patch file"), multiple: false, directory: false, filters: [{ name: "Patch", extensions: ["patch", "diff", "txt"] }] });
    if (typeof chosen === "string") await load(chosen);
  };
  const chooseRoot = async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const chosen = await open({ title: tt("Folder the patch is applied in"), multiple: false, directory: true, defaultPath: root || undefined });
    if (typeof chosen === "string") setRoot(chosen);
  };
  const check = async () => {
    if (busy) return;
    if (!text.trim() || !root.trim()) { setError(tt("Give the patch and the folder first.")); return; }
    // A Check right after the text arrived (a file just read) comes before the format is read from it: read it now.
    let f = format;
    if (!f && choice === "auto") {
      f = await api.patchSniff(text.slice(0, 65536)).catch(() => null);
      if (f) setSniffed(f);
    }
    if (!f) { setError(tt("The format of this text is not recognised: choose it.")); return; }
    if (strip === undefined) { setError(tt("The number of leading folders to strip is a whole number from 0 to 20, or empty.")); return; }
    setBusy(true); setError(""); setDone(null);
    try { setPreview({ result: await api.patchPreview(root.trim(), text, f, strip ?? undefined, target.trim() || undefined), key: keyOf(f) }); }
    catch (e) { setPreview(null); setError(String(e instanceof Error ? e.message : e)); }
    finally { setBusy(false); }
  };
  const apply = async () => {
    if (busy || !format || !preview || !canApply(preview.result, current)) return;
    setBusy(true); setError("");
    try {
      const written = await api.patchApply(root.trim(), text, format, strip ?? undefined, target.trim() || undefined, preview.result.fingerprint);
      setDone(written); setPreview(null);
      A.notify(written.length === 1 ? tt("Patch applied to 1 file") : tt("Patch applied to {count} files", { count: written.length }), "success");
    } catch (e) { setError(String(e instanceof Error ? e.message : e)); setPreview(null); }
    finally { setBusy(false); }
  };
  const close = () => A.openOverlay(back ?? null);

  // Over the app like the merge window; a click beside it does not close it (a pasted patch would be lost).
  return (
    <div className="backdrop glass center">
      <div className="sm pi" ref={panel} tabIndex={-1} role="dialog" aria-label={tt("Apply a patch")}
        onKeyDown={(e) => {
          if (e.nativeEvent.isComposing || busy) return;
          // Ctrl+Enter anywhere in the dialog: Check, and once the check is current and clean, Apply.
          if (e.key === "Enter" && primaryMod(e) && !e.altKey && !e.shiftKey) {
            e.preventDefault(); e.stopPropagation();
            if (preview && canApply(preview.result, current)) void apply(); else void check();
            return;
          }
          if (e.key !== "Escape") return;
          e.preventDefault(); e.stopPropagation();
          // In a field Esc leaves the field first (what was typed stays); on the dialog itself it closes.
          if ((e.target as HTMLElement).closest("textarea,input,select")) panel.current?.focus(); else close();
        }}>
        <header className="sm-head"><h2>{tt("Apply a patch")}</h2><button className="icon-btn" aria-label={tt("Close (Esc)")} title={tt("Close (Esc)")} onClick={close}>×</button></header>
        <div className="sm-pick">
          <div className="sm-row"><span>{tt("Patch")}</span><button disabled={busy} onClick={() => void chooseFile()}>{tt("Choose a file…")}</button><span className="sm-out" title={source || undefined}>{source ? basename(source) : tt("or paste it below")}</span></div>
          <textarea aria-label={tt("Text of the patch")} spellCheck={false} rows={8} value={text} disabled={busy} placeholder={"--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-old\n+new"} onChange={(e) => { setText(e.target.value); setSource(""); setDone(null); }} />
          <label className="sm-row"><span>{tt("Format")}</span>
            <select value={choice} disabled={busy} onChange={(e) => setChoice(e.target.value as FormatChoice)}>{CHOICES.map((c) => <option key={c.id} value={c.id}>{tt(c.label)}</option>)}</select>
            <span className="sm-out">{choice === "auto" ? (sniffed ? tt("Recognised: {format}", { format: sniffed }) : text.trim() ? tt("Not recognised") : "") : ""}</span></label>
          <label className="sm-row"><span>{tt("Folder")}</span><input value={root} disabled={busy} spellCheck={false} onChange={(e) => setRoot(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); void check(); } }} /><button disabled={busy} onClick={() => void chooseRoot()}>{tt("Browse…")}</button></label>
          <label className="sm-row"><span>{tt("Strip")}</span><input value={stripText} disabled={busy} spellCheck={false} placeholder={tt("automatic")} aria-invalid={strip === undefined} onChange={(e) => setStripText(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); void check(); } }} /><span className="sm-out">{tt("leading folders of the names in the patch")}</span></label>
          {(needsTarget(preview?.result.problems ?? [], format) || target) && <label className="sm-row"><span>{tt("File")}</span><input value={target} disabled={busy} spellCheck={false} placeholder={tt("the file this patch is for, below the folder")} onChange={(e) => setTarget(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); void check(); } }} /></label>}
          <div className="sm-actions">
            <button disabled={busy || !text.trim()} onClick={() => void check()}>{tt("Check")}</button>
            <button className="gm-confirm" disabled={busy || !canApply(preview?.result ?? null, current)} onClick={() => void apply()} title={current ? undefined : tt("Check the patch again: the text or the settings changed")}>{tt("Apply")}</button>
          </div>
          <p className="pi-keys"><kbd>Enter</kbd> {tt("in a field: Check")} · <kbd>{platformLabel("Ctrl+Enter")}</kbd> {tt("Check, then Apply")} · <kbd>Esc</kbd> {tt("leave the field, then close")}</p>
          {error && <p className="gm-note" role="alert" style={{ whiteSpace: "pre-wrap" }}>{error}</p>}
        </div>
        <div className="pi-result" aria-live="polite">
          {preview && !current && <p className="sm-hint">{tt("The text or the settings changed since the check.")}</p>}
          {preview && preview.result.problems.length > 0 && <ul role="alert" className="pi-problems">{preview.result.problems.map((p, i) => <li key={i}>{p}</li>)}</ul>}
          {preview && preview.result.problems.length === 0 && <ul className="pi-files">{preview.result.files.map((f) => <li key={f.path} title={f.path}><b>{basename(f.path)}</b> <small>{f.name !== basename(f.path) ? `${f.name} · ` : ""}{hunkSummary(f.hunks, f.offsets)}</small></li>)}</ul>}
          {done && <p role="status">{done.length === 1 ? tt("Applied to 1 file:") : tt("Applied to {count} files:", { count: done.length })} {done.map((p) => basename(p)).join(", ")}</p>}
          {!preview && !done && !error && <p className="sm-hint">{tt("Give a patch and a folder, then Check. Nothing is written until you Apply, and only a patch without problems can be applied.")}</p>}
        </div>
      </div>
    </div>
  );
}
