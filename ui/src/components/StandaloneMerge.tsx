import { useEffect, useId, useMemo, useRef, useState } from "react";
import * as A from "../state/actions";
import { api, type TextFileVersion } from "../api/ipc";
import { basename, join, parent } from "../core/path";
import { tt } from "../i18n";
import { GitMergeEditor } from "./GitMerge";
import "./StandaloneMerge.css";
import { useStore } from "../state/store";
import { finishCompareWait, useCompareWaitClose } from "../state/launch";

type Field = "base" | "left" | "right";
type InputVersion = TextFileVersion & { path: string };
interface Loaded { base: string; ours: string; theirs: string; inputs: InputVersion[] }
interface Output { path: string; canonicalPath: string; revision: string | null }
interface StandaloneMergeDialogs { open: () => Promise<string | null>; save: () => Promise<string | null> }
let selfTestDialogs: StandaloneMergeDialogs | null = null;

/** Runs only when the graphical self-test supplies controlled native dialog answers. */
export function setStandaloneMergeDialogsForSelfTest(dialogs: StandaloneMergeDialogs) {
  const previous = selfTestDialogs;
  selfTestDialogs = dialogs;
  return () => { if (selfTestDialogs === dialogs) selfTestDialogs = previous; };
}

function canonicalKey(path: string, platform: string) {
  if (platform === "macos") return path.toLowerCase().normalize("NFD");
  return platform === "windows" ? path.toLowerCase() : path;
}

const reportWaitError = (message: string) => { void finishCompareWait("error", undefined, message); };

/**
 * A three-way merge of three files outside Git: the files are chosen (or come from the selection in a browser; no base
 * means an empty base), the same conflict cards as the Git merge editor decide the result, and the result is saved to a
 * chosen file only when every conflict has a choice. The save is checked against the revision of the target as it was when
 * it was chosen: a file that changed meanwhile is not overwritten.
 */
export function StandaloneMerge({ base, left, right, waitOutput }: { base?: string; left?: string; right?: string; waitOutput?: string }) {
  const wait = waitOutput !== undefined;
  const vim = useStore((s) => !!s.settings?.vimKeys);
  const platform = useStore((s) => s.info?.platform ?? "");
  const [paths, setPaths] = useState<Record<Field, string>>({ base: base ?? "", left: left ?? "", right: right ?? "" });
  const [noBase, setNoBase] = useState(!base);
  const [output, setOutput] = useState<Output | null>(null);
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [error, setError] = useState(""), [busy, setBusy] = useState(false), [dirty, setDirty] = useState(false), [saved, setSaved] = useState<string | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const fieldsId = useId();
  const started = useRef(false);
  const saving = useRef(false);
  const closing = useRef(false);
  const ready = paths.left !== "" && paths.right !== "" && (noBase || paths.base !== "");
  useEffect(() => { root.current?.focus(); }, []);

  const browse = async (field: Field) => {
    const chosen = selfTestDialogs
      ? await selfTestDialogs.open()
      : await (await import("@tauri-apps/plugin-dialog")).open({
        title: tt("Choose a file"), multiple: false, directory: false,
        defaultPath: parent(paths[field] || paths.left || paths.right) ?? undefined,
      });
    if (typeof chosen === "string") setPaths((p) => ({ ...p, [field]: chosen }));
  };
  // The files cannot be changed under a merge in progress: that would drop the choices without asking. This asks first.
  const changeFiles = async () => {
    if (dirty && !(await A.confirm(tt("Discard the merge?"), tt("The choices and edits of this merge have not been saved."), tt("Discard")))) return;
    setLoaded(null); setOutput(null); setDirty(false); setError(""); setSaved(null);
  };
  const merge = async () => {
    if (!ready || busy) return;
    setBusy(true); setError(""); setLoaded(null); setOutput(null); setDirty(false); setSaved(null);
    try {
      const [theirs, ours, basis] = await Promise.all([
        api.readTextFile(paths.right), api.readTextFile(paths.left),
        noBase ? null : api.readTextFile(paths.base),
      ]);
      setLoaded({ base: basis?.text ?? "", ours: ours.text, theirs: theirs.text,
        inputs: [
          { path: paths.left, canonicalPath: ours.canonicalPath, revision: ours.revision },
          { path: paths.right, canonicalPath: theirs.canonicalPath, revision: theirs.revision },
          ...(basis ? [{ path: paths.base, canonicalPath: basis.canonicalPath, revision: basis.revision }] : []),
        ],
      });
    } catch (e) {
      const message = String(e instanceof Error ? e.message : e);
      if (wait) reportWaitError(message);
      else setError(message);
    }
    finally { setBusy(false); }
  };
  useEffect(() => {
    if (!wait || started.current) return;
    started.current = true;
    void merge();
  }, [wait, merge]);
  const chooseOutput = async (): Promise<Output | null> => {
    if (output) return output;
    const chosen = selfTestDialogs
      ? await selfTestDialogs.save()
      : await (await import("@tauri-apps/plugin-dialog")).save({
        title: tt("Save the merged file as"),
        defaultPath: join(parent(paths.left) ?? "", `merged-${basename(paths.left)}`),
      });
    if (!chosen) return null;
    const inputs = loaded?.inputs ?? [];
    const found = await api.textFileVersion(chosen);
    const samePath = (a: string, b: string) => canonicalKey(a, platform) === canonicalKey(b, platform);
    const literal = inputs.find((input) => input.path === chosen) ?? inputs.find((input) => samePath(input.path, chosen));
    const unverified = () => new Error(tt("The selected file could not be verified. Reload the merge before saving."));
    // The literal input could have disappeared or become an alias for another file.
    if (literal && (!found || !samePath(literal.canonicalPath, found.canonicalPath))) throw unverified();
    let revision: string | null = null;
    if (found) {
      const exact = inputs.filter((input) => input.canonicalPath === found.canonicalPath);
      const matches = exact.length ? exact : inputs.filter((input) => samePath(input.canonicalPath, found.canonicalPath));
      revision = matches.length ? matches[0].revision : found.revision;
      if (!revision || matches.some((input) => input.revision !== revision)) throw unverified();
      // Keep the existing UTF-8 check for an unrelated output that already exists.
      if (!matches.length) {
        const existing = await api.readTextFile(found.canonicalPath);
        if (existing.canonicalPath !== found.canonicalPath || existing.revision !== revision) throw unverified();
      }
    }
    const next = { path: chosen, canonicalPath: found?.canonicalPath ?? chosen, revision };
    setOutput(next);
    return next;
  };
  const confirm = async (content: string) => {
    if (saving.current || closing.current) return;
    saving.current = true;
    setBusy(true); setError("");
    try {
      if (wait) {
        await finishCompareWait("accepted", content);
        return;
      }
      const target = await chooseOutput();
      if (!target) return;
      await api.saveText(target.canonicalPath, content, target.revision ?? undefined, target.revision === null);
      // Done: the merge is closed as a piece of work (its choices are in the file), and "Merge again" starts anew.
      setLoaded(null); setOutput(null); setDirty(false); setSaved(target.path);
      A.notify(tt("Saved {name}", { name: basename(target.path) }), "success");
    } catch (e) {
      // The chosen target and its revision stay: trying again cannot overwrite a file that changed meanwhile. Choosing
      // the target again is a separate, visible step.
      setError(String(e instanceof Error ? e.message : e));
    }
    finally { saving.current = false; setBusy(false); }
  };
  const close = async () => {
    if (closing.current || saving.current) return;
    closing.current = true;
    try {
      if (dirty && !(await A.confirm(tt("Discard the merge?"), tt("The choices and edits of this merge have not been saved."), tt("Discard")))) return;
      if (wait) await finishCompareWait("cancelled");
      else A.openOverlay(null);
    } finally { closing.current = false; }
  };
  useCompareWaitClose(wait, () => void close());
  const conflict = useMemo(() => (loaded ? { file: basename(waitOutput ?? output?.path ?? paths.left), base: loaded.base, ours: loaded.ours, theirs: loaded.theirs, result: "" } : null), [loaded, waitOutput, output?.path, paths.left]);

  const row = (field: Field, label: string, hint: string) => {
    const id = `${fieldsId}-${field}`;
    const disabled = busy || !!loaded || (field === "base" && noBase);
    return (
      <div className="sm-row" key={field}>
        <label id={`${id}-label`} htmlFor={id}>{label}</label>
        <input id={id} value={paths[field]} disabled={wait || disabled} spellCheck={false} placeholder={hint} onChange={(e) => setPaths((p) => ({ ...p, [field]: e.target.value }))}
          onKeyDown={(e) => {
            if (e.key !== "Enter" || e.nativeEvent.isComposing || e.keyCode === 229) return;
            e.preventDefault();
            void merge();
          }} />
        {!wait && <button aria-describedby={`${id}-label`} disabled={disabled} onClick={() => void browse(field)}>{tt("Browse…")}</button>}
      </div>
    );
  };

  const panel = (
    <div className="sm" ref={root} tabIndex={-1} role="dialog" aria-modal="true" aria-label={tt("Merge three files")}
      onMouseDown={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        if (e.key !== "Escape" || e.nativeEvent.isComposing || e.keyCode === 229 || (e.target as HTMLElement).closest("textarea")) return;
        e.preventDefault();
        e.stopPropagation();
        void close();
      }}>
      <header className="sm-head">
        <h2>{tt("Merge three files")}</h2>
        <button className="icon-btn" aria-label={tt("Close (Esc)")} title={tt("Close (Esc)")} onClick={() => void close()}>×</button>
      </header>
      <div className="sm-pick">
        {row("base", tt("Base"), tt("The common ancestor"))}
        <label className="sm-nobase"><input type="checkbox" checked={noBase} disabled={wait || busy || !!loaded} onChange={(e) => setNoBase(e.target.checked)} /> {tt("No base: merge against an empty file")}</label>
        {row("left", tt("Local"), tt("Your version"))}
        {row("right", tt("Remote"), tt("The other version"))}
        <div className="sm-actions">
          {!wait && (loaded
            ? <button disabled={busy} onClick={() => void changeFiles()}>{tt("Change the files…")}</button>
            : <button className="gm-confirm" disabled={!ready || busy} onClick={() => void merge()}>{saved ? tt("Merge again") : tt("Merge")}</button>)}
          {waitOutput && <span className="sm-out" title={waitOutput}>{tt("Saves to {name}", { name: basename(waitOutput) })}</span>}
          {output && <span className="sm-out" title={output.path}>{tt("Saves to {name}", { name: basename(output.path) })}</span>}
          {output && error && <button disabled={busy} onClick={() => { setOutput(null); setError(""); }}>{tt("Choose the target again")}</button>}
          {saved && !loaded && <span className="sm-out" role="status" title={saved}>{tt("Saved {name}", { name: basename(saved) })}</span>}
        </div>
        {error && <p className="gm-note" role="alert">{error}</p>}
      </div>
      {conflict
        ? <GitMergeEditor conflict={conflict} vim={vim} busy={busy} onDirty={setDirty} onOpenExternal={() => { if (paths.left) A.editPaths([paths.left]); }} onConfirm={confirm} onFailure={wait ? reportWaitError : undefined} />
        : !error && <p className="sm-hint">{wait ? tt("Computing the merge…") : tt("Choose the files, then Merge. The result is saved only when every conflict has a choice.")}</p>}
    </div>
  );
  return <div className="backdrop glass center" onMouseDown={() => void close()}>{panel}</div>;
}
