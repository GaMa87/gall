import { useEffect, useState } from "react";
import { api } from "../api/ipc";
import type { CompareAiConfig } from "../api/types";
import { outcomeMessage } from "../core/explain";
import { tt } from "../i18n";

/** What the test asks the model: two tiny texts with one changed line. */
const TEST_LEFT = "a\nold line\nc\n";
const TEST_RIGHT = "a\nnew line\nc\n";

/**
 * The settings of the local explanation model, which used to be edited by hand in `compare-ai.json`: the program (an
 * absolute path, never searched for), its arguments one per line, the model name and the deadline. Test saves the
 * settings and asks the model about two tiny texts, so that a wrong program or argument shows its reason here and not in
 * the middle of a comparison. Nothing is run until Test (or an explanation) is asked for.
 */
export function CompareAiSettings() {
  const [config, setConfig] = useState<CompareAiConfig | null>(null);
  const [args, setArgs] = useState("");
  const [deadline, setDeadline] = useState("30");
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let live = true;
    api.compareAiConfigGet().then((c) => { if (live) { setConfig(c); setArgs(c.args.join("\n")); setDeadline(String(c.deadlineSecs)); } }).catch((e) => { if (live) setMessage(String(e instanceof Error ? e.message : e)); });
    return () => { live = false; };
  }, []);
  if (!config) return <div className="opt"><div className="opt-text"><small role="status">{message || tt("Loading…")}</small></div></div>;
  const edit = (patch: Partial<CompareAiConfig>) => setConfig((c) => (c ? { ...c, ...patch } : c));
  const current = (): CompareAiConfig | null => {
    const secs = Number(deadline);
    if (!Number.isInteger(secs)) { setMessage(tt("The deadline is a whole number of seconds.")); return null; }
    return { ...config, args: args.split("\n").map((a) => a.trim()).filter(Boolean), deadlineSecs: secs };
  };
  const save = async (): Promise<boolean> => {
    const next = current();
    if (!next) return false;
    try { await api.compareAiConfigSave(next); setConfig(next); setMessage(tt("Saved.")); return true; }
    catch (e) { setMessage(String(e instanceof Error ? e.message : e)); return false; }
  };
  const test = async () => {
    setBusy(true);
    try {
      if (!(await save())) return;
      const outcome = await api.compareAiExplain(Date.now(), "test left", "test right", TEST_LEFT, TEST_RIGHT);
      setMessage(outcome.outcome === "explained" ? tt("The model answered: {summary}", { summary: outcome.explanation.summary }) : outcomeMessage(outcome) ?? "");
    } catch (e) { setMessage(String(e instanceof Error ? e.message : e)); }
    finally { setBusy(false); }
  };
  const choose = async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const chosen = await open({ title: tt("Choose the program of the local model"), multiple: false, directory: false });
    if (typeof chosen === "string") edit({ program: chosen });
  };
  return (
    <>
      <div className="opt">
        <div className="opt-text"><span className="opt-label">{tt("Explain changes with a local model")}</span><small>{tt("Off unless you turn it on. Nothing is sent anywhere, the program runs on this computer, and only when you ask for an explanation.")}</small></div>
        <div className="opt-ctl"><input type="checkbox" aria-label={tt("Explain changes with a local model")} checked={config.enabled} onChange={(e) => edit({ enabled: e.target.checked })} /></div>
      </div>
      <div className="opt">
        <div className="opt-text"><span className="opt-label">{tt("Program")}</span><small>{tt("An absolute path; Gall does not search PATH and starts no shell.")}</small></div>
        <div className="opt-ctl"><input type="text" spellCheck={false} aria-label={tt("Program")} value={config.program} onChange={(e) => edit({ program: e.target.value })} /> <button className="btn" onClick={() => void choose()}>{tt("Browse…")}</button></div>
      </div>
      <div className="opt">
        <div className="opt-text"><span className="opt-label">{tt("Arguments")}</span><small>{tt("One per line.")}</small></div>
        <div className="opt-ctl"><textarea rows={3} spellCheck={false} aria-label={tt("Arguments")} value={args} onChange={(e) => setArgs(e.target.value)} /></div>
      </div>
      <div className="opt">
        <div className="opt-text"><span className="opt-label">{tt("Model name")}</span><small>{tt("Shown with each answer and part of its cache key.")}</small></div>
        <div className="opt-ctl"><input type="text" spellCheck={false} aria-label={tt("Model name")} value={config.model} onChange={(e) => edit({ model: e.target.value })} /></div>
      </div>
      <div className="opt">
        <div className="opt-text"><span className="opt-label">{tt("Deadline (seconds)")}</span><small>{tt("The program is stopped after this long.")}</small></div>
        <div className="opt-ctl"><input type="text" inputMode="numeric" aria-label={tt("Deadline (seconds)")} value={deadline} onChange={(e) => setDeadline(e.target.value)} /></div>
      </div>
      <div className="opt">
        <div className="opt-text">{message && <small role="status" style={{ whiteSpace: "pre-wrap" }}>{message}</small>}</div>
        <div className="opt-ctl"><button className="btn" disabled={busy} onClick={() => void save()}>{tt("Save")}</button> <button className="btn" disabled={busy} onClick={() => void test()}>{busy ? tt("Testing…") : tt("Test")}</button></div>
      </div>
    </>
  );
}
