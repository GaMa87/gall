// Batch rename (F2 with several items selected): a name template with tokens, find &
// replace (text or regular expression), a case change and a counter, with a live preview
// that flags problems before anything is renamed. "Edit as text" lists one name per line
// for editing by hand (as in vidir / oil.nvim). One Ctrl+Z reverts the whole rename.
import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api/ipc";
import type { FileEntry } from "../api/types";
import { DEFAULT_RULES, ISSUE_TEXT, namesFromText, newNames, renameIssues, type CaseMode, type RenameRules } from "../core/batchRename";
import { basename, join, parent } from "../core/path";
import * as A from "../state/actions";
import { controller } from "../state/controllers";
import { useStore } from "../state/store";
import { FileIcon, Icon } from "./Icon";
import { primaryMod } from "../core/keymap";

const TOKENS: { token: string; label: string; title: string }[] = [
  { token: "{name}", label: "Name", title: "The current name without the extension" },
  { token: "{n:2}", label: "Counter", title: "1, 2, 3 … ({n:3} pads to 001); start and step below" },
  { token: "{date}", label: "Date", title: "Modified date, 2026-11-03" },
  { token: "{time}", label: "Time", title: "Modified time, 09.05.07" },
  { token: "{folder}", label: "Folder", title: "Name of the folder the item is in" },
  { token: "{ext}", label: "Ext", title: "The extension, without the dot" },
];

const CASES: { id: CaseMode; label: string }[] = [
  { id: "keep", label: "Keep case" }, { id: "lower", label: "lower" }, { id: "upper", label: "UPPER" }, { id: "title", label: "Title" },
];

export function BatchRename({ b, entries }: { b: number; entries: FileEntry[] }) {
  const dir = parent(entries[0]?.path ?? "") ?? "";
  const platform = useStore((s) => s.info?.platform ?? "linux");
  const [rules, setRules] = useState<RenameRules>(DEFAULT_RULES);
  const [textMode, setTextMode] = useState(false);
  const [text, setText] = useState("");
  const [others, setOthers] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const templateRef = useRef<HTMLInputElement>(null);
  const textRef = useRef<HTMLTextAreaElement>(null);
  const close = () => A.openOverlay(null);
  const set = <K extends keyof RenameRules>(k: K, v: RenameRules[K]) => setRules((r) => ({ ...r, [k]: v }));

  // Names in the folder: a new name must not take one that stays.
  useEffect(() => {
    api.listDir(dir, { showHidden: true, hideTemp: false }).then((l) => setOthers(l.map((e) => e.name))).catch(() => {});
    templateRef.current?.select();
  }, [dir]);

  const oldNames = useMemo(() => entries.map((e) => e.name), [entries]);
  const fromRules = useMemo(
    () => newNames(entries.map((e) => ({ name: e.name, isDir: e.isDir, modified: e.modified, folder: basename(dir) })), rules),
    [entries, rules, dir],
  );
  const typed = textMode ? namesFromText(text, entries.length) : null;
  const names = textMode ? typed ?? oldNames : fromRules.names;
  const issues = useMemo(
    () => renameIssues(oldNames, names, others, { windows: platform === "windows", ignoreCase: platform !== "linux" }),
    [oldNames, names, others, platform],
  );
  const changed = names.filter((n, i) => n !== oldNames[i]).length;
  const problems = issues.filter(Boolean).length;
  const error = textMode
    ? (typed ? null : `${text.replace(/\n$/, "").split("\n").length} lines for ${A.items(entries.length)}: keep one name per line`)
    : fromRules.error;
  const canApply = !busy && changed > 0 && !problems && !error;

  const toText = () => {
    setText(names.join("\n"));
    setTextMode(true);
    setTimeout(() => textRef.current?.focus(), 0);
  };

  const insertToken = (token: string) => {
    const el = templateRef.current;
    const v = rules.template;
    const [a, z] = el ? [el.selectionStart ?? v.length, el.selectionEnd ?? v.length] : [v.length, v.length];
    set("template", v.slice(0, a) + token + v.slice(z));
    setTimeout(() => { el?.focus(); el?.setSelectionRange(a + token.length, a + token.length); }, 0);
  };

  const apply = async () => {
    if (!canApply) return;
    setBusy(true);
    try {
      const pairs = entries.map((e, i) => [e.path, names[i]] as [string, string]).filter(([p, n]) => basename(p) !== n);
      const out = await api.batchRename(pairs);
      close();
      A.notify(`Renamed ${A.items(out.length)}`, "success", { label: "Undo", run: A.undo });
      A.refreshUndo();
      const renamed = names.map((n) => join(dir, n));
      controller(b)?.select(renamed, renamed[0]);
    } catch (e) {
      A.fail(e);
      setBusy(false);
    }
  };

  const onKey = (e: React.KeyboardEvent) => {
    e.stopPropagation();
    if (e.key === "Escape") { e.preventDefault(); return close(); }
    const inText = (e.target as HTMLElement).tagName === "TEXTAREA";
    if (e.key === "Enter" && (primaryMod(e) || !inText)) { e.preventDefault(); apply(); }
  };

  return (
    <div className="backdrop" onMouseDown={close}>
      <div className="modal brn-win" onMouseDown={(e) => e.stopPropagation()} onKeyDown={onKey} tabIndex={-1}>
        <div className="brn-head">
          <Icon name="rename" size={17} />
          <b>Rename {A.items(entries.length)}</b>
          <span className="brn-sum">{changed ? `${A.items(changed)} will change` : "nothing changes yet"}</span>
          <div className="seg">
            <button className={textMode ? "" : "on"} onClick={() => setTextMode(false)} title="Template, find & replace, case">Rules</button>
            <button className={textMode ? "on" : ""} onClick={() => (textMode ? null : toText())} title="One name per line, edited by hand">Edit as text</button>
          </div>
        </div>
        {!textMode && (
          <div className="brn-rules">
            <label className="brn-field">
              <span>Name</span>
              <input ref={templateRef} type="text" value={rules.template} spellCheck={false} placeholder="{name}" onChange={(e) => set("template", e.target.value)} />
            </label>
            <div className="brn-tokens">
              {TOKENS.map((t) => (
                <button key={t.token} className="chip" title={`${t.title} · ${t.token}`} onClick={() => insertToken(t.token)}>{t.label}</button>
              ))}
            </div>
            <label className="brn-field">
              <span>Find</span>
              <input type="text" value={rules.find} spellCheck={false} placeholder="text to replace" onChange={(e) => set("find", e.target.value)} />
              <button className={`tgl ${rules.matchCase ? "on" : ""}`} title="Match case" onClick={() => set("matchCase", !rules.matchCase)}>Aa</button>
              <button className={`tgl ${rules.regex ? "on" : ""}`} title="Regular expression ($1 in Replace refers to a group)" onClick={() => set("regex", !rules.regex)}>.*</button>
            </label>
            <label className="brn-field">
              <span>Replace</span>
              <input type="text" value={rules.replace} spellCheck={false} placeholder="with" onChange={(e) => set("replace", e.target.value)} />
            </label>
            <div className="brn-opts">
              <div className="seg">
                {CASES.map((c) => <button key={c.id} className={rules.caseMode === c.id ? "on" : ""} onClick={() => set("caseMode", c.id)}>{c.label}</button>)}
              </div>
              <label title="Counter: first number">Start <input type="number" value={rules.start} onChange={(e) => set("start", Number(e.target.value) || 0)} /></label>
              <label title="Counter: added for each item">Step <input type="number" value={rules.step} onChange={(e) => set("step", Number(e.target.value) || 1)} /></label>
              <label className="brn-check"><input type="checkbox" checked={rules.withExt} onChange={(e) => set("withExt", e.target.checked)} /> Rename extensions too</label>
            </div>
          </div>
        )}
        {textMode && (
          <textarea ref={textRef} className="brn-text" value={text} spellCheck={false} wrap="off" onChange={(e) => setText(e.target.value)} />
        )}
        {error && <div className="brn-error">{error}</div>}
        <div className="brn-list">
          {entries.map((e, i) => {
            const issue = issues[i];
            const same = names[i] === e.name;
            return (
              <div key={e.path} className={`brn-row ${same ? "same" : ""} ${issue ? "bad" : ""}`}>
                <FileIcon entry={e} />
                <span className="old" title={e.name}>{e.name}</span>
                <span className="arrow">→</span>
                <span className="new" title={names[i]}>{names[i] || " "}</span>
                <span className="issue">{issue ? ISSUE_TEXT[issue] : ""}</span>
              </div>
            );
          })}
        </div>
        <div className="foot brn-foot">
          <span className="brn-hint">{problems ? `${A.items(problems)} to fix` : "Enter renames · one Ctrl+Z undoes all"}</span>
          <button className="btn" onClick={close}>Cancel</button>
          <button className="btn primary" disabled={!canApply} onClick={apply}>Rename{changed ? ` ${A.items(changed)}` : ""}</button>
        </div>
      </div>
    </div>
  );
}
