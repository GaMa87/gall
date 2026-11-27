// The bar of a file picker window (`gall --pick`, used by the xdg-desktop-portal FileChooser
// backend): what is being asked, the file name (save), the file-type filter, Cancel and the accept
// button. The folder panes are the normal ones; Enter on a file accepts it, Escape cancels.
import { useEffect, useRef, useState } from "react";
import { tt } from "../i18n";
import { api } from "../api/ipc";
import { acceptLabel, checkPick, type Chosen } from "../core/pick";
import { exists } from "../state/pick";
import * as A from "../state/actions";
import { controller } from "../state/controllers";
import { getState, setState, useStore } from "../state/store";
import { ThemedSelect } from "./ThemedSelect";

const ASK: Record<string, string> = { open: "Choose a file", save: "Save as", dir: "Choose a folder" };

/** What the active browser has selected (else its focused item; a folder picker only counts what was selected). */
function chosen(b: number, folderMode: boolean): Chosen[] {
  const c = controller(b);
  const sel = (folderMode ? c?.explicitSelection?.() : c?.selection()) ?? [];
  const f = folderMode ? null : c?.focused();
  const list = sel.length ? sel : f ? [f] : [];
  return list.map((e) => ({ path: e.path, name: e.name, isDir: e.isDir }));
}

/** An answer is being given: a second Enter or click must not answer again. */
let answering = false;

export async function accept(typedName: string): Promise<void> {
  if (answering) return;
  const filter = getState().pickFilter;
  const pick = getState().pick;
  if (!pick) return;
  answering = true;
  try {
    await acceptNow(pick, typedName, filter);
  } finally {
    answering = false;
  }
}

async function acceptNow(pick: NonNullable<ReturnType<typeof getState>["pick"]>, typedName: string, filter: number): Promise<void> {
  const b = A.activeBrowser();
  const r = checkPick(pick, { dir: A.activePath(b), selection: pick.mode === "save" ? [] : chosen(b, pick.mode === "dir"), typedName, filter });
  if (!r.ok) {
    A.notify(r.error, "warn");
    return;
  }
  if (pick.mode === "save") {
    const found = await exists(r.paths[0]);
    if (found?.isDir) {
      A.notify(`“${typedName.trim()}” is a folder: choose another name`, "warn");
      return;
    }
    if (found && !(await A.confirm("Replace file?", `“${typedName.trim()}” already exists. Replace it?`, "Replace"))) return;
  }
  const values = getState().pickChoices;
  await api.pickFinish(r.paths, pick.filters.length ? filter : null, pick.choices.map((c) => [c.id, values[c.id] ?? c.initial] as [string, string])).catch(A.fail);
}

export function PickerBar() {
  const pick = useStore((s) => s.pick);
  const [name, setName] = useState(pick?.name ?? "");
  const filter = useStore((s) => s.pickFilter);
  const choiceValues = useStore((s) => s.pickChoices);
  const input = useRef<HTMLInputElement>(null);
  const latest = useRef({ name });
  latest.current = { name };

  useEffect(() => {
    if (pick?.mode === "save") setTimeout(() => input.current?.select(), 200);
  }, [pick?.mode]);

  // Escape cancels, unless a dialog or menu has it, or text is being edited.
  useEffect(() => {
    if (!pick) return;
    const onKey = (e: KeyboardEvent) => {
      if (getState().overlay || getState().menu) return;
      // Ctrl+Enter chooses what is selected, or the current folder: the keyboard way to the accept button.
      if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
        // Not while text is being edited elsewhere (rename, address, filter): only in our own name field.
        const t = e.target as HTMLElement | null;
        if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable) && !t.classList.contains("pick-name")) return;
        e.preventDefault();
        e.stopPropagation();
        void accept(latest.current.name);
        return;
      }
      if (e.key !== "Escape") return;
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT")) return;
      e.preventDefault();
      void api.pickCancel().catch(() => {});
    };
    // Capture phase: before "Open with…" (Ctrl+Enter) of the file list sees the key.
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [pick]);

  if (!pick) return null;
  return (
    <div className="pick-bar" role="group" aria-label={tt("File picker")}>
      <span className="pick-ask">{pick.title || ASK[pick.mode]}</span>
      {pick.mode === "save" && (
        <input ref={input} className="pick-name" type="text" value={name} spellCheck={false} aria-label={tt("File name")}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); void accept(name); } else if (e.key === "Escape") { e.preventDefault(); void api.pickCancel().catch(() => {}); } }} />
      )}
      {pick.filters.length > 0 && (
        <ThemedSelect value={String(filter)} onChange={(e) => setState({ pickFilter: Number(e.target.value) })}>
          {pick.filters.map((f, i) => <option key={i} value={i}>{f.name}</option>)}
        </ThemedSelect>
      )}
      {pick.choices.map((c) => c.options.length === 0 ? (
        <label key={c.id} className="pick-choice">
          <input type="checkbox" checked={choiceValues[c.id] === "true"}
            onChange={(e) => setState({ pickChoices: { ...getState().pickChoices, [c.id]: e.target.checked ? "true" : "false" } })} />
          <span>{c.label.replace(/_/g, "")}</span>
        </label>
      ) : (
        <label key={c.id} className="pick-choice">
          <span>{c.label.replace(/_/g, "")}</span>
          <ThemedSelect value={choiceValues[c.id] ?? c.initial} aria-label={c.label.replace(/_/g, "")}
            onChange={(e) => setState({ pickChoices: { ...getState().pickChoices, [c.id]: e.target.value } })}>
            {c.options.map(([id, label]) => <option key={id} value={id}>{label}</option>)}
          </ThemedSelect>
        </label>
      ))}
      <span className="pick-gap" />
      <button className="btn" onClick={() => void api.pickCancel().catch(() => {})}>{tt("Cancel")}</button>
      <button className="btn primary" onClick={() => void accept(name)}>{acceptLabel(pick)}</button>
    </div>
  );
}
