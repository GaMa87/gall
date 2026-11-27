// "Open with…" palette, also used for "Terminal here with…" and "Edit with…".
// Lists the recommended applications for the selection plus all installed applications
// on the OS; typing filters both or becomes a custom command.
// Enter opens once, Ctrl+Enter remembers in Gall (for the scope picked with Tab:
// everywhere, the project, the folder, or only this file), and Alt+Enter sets the
// OS-wide system default (Linux / macOS).
import { useEffect, useMemo, useRef, useState } from "react";
import { tt } from "../i18n";
import { api } from "../api/ipc";
import type { AppChoice, OpenRule } from "../api/types";
import { fuzzyMatch } from "../core/fuzzy";
import { findRule, sameScope, scopesFor, type Scope } from "../core/openRules";
import { basename } from "../core/path";
import * as A from "../state/actions";
import { useStore } from "../state/store";
import { Icon } from "./Icon";
import { Highlight } from "./QuickOpen";
import { primaryMod } from "../core/keymap";
import { platformLabel } from "../core/keymap";

type Row =
  | { kind: "app"; app: AppChoice; tag: string; star?: boolean; positions: number[]; other?: boolean }
  | { kind: "more"; count: number }
  | { kind: "system"; tag: string }
  | { kind: "dialog" }
  | { kind: "custom"; command: string };

function canSetOsDefault(app: AppChoice, platform: string | undefined): boolean {
  if (platform === "linux") return app.command.startsWith("desktop:");
  if (platform === "macos") return app.command.startsWith("macos:");
  return false;
}

export function OpenWith({ paths, mode }: { paths: string[]; mode: "file" | "terminal" | "editor" }) {
  const settings = useStore((s) => s.settings)!;
  const info = useStore((s) => s.info);
  const [apps, setApps] = useState<AppChoice[] | null>(null);
  const [allApps, setAllApps] = useState<AppChoice[] | null>(null);
  const [showAll, setShowAll] = useState(false);
  const [q, setQ] = useState("");
  const [idx, setIdx] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const terminal = mode === "terminal";
  const editor = mode === "editor";
  // Terminal and editor are one remembered program each; files have rules with scopes.
  const single = terminal || editor;
  // The rule in effect (only when every selected file resolves to the same one).
  const rule = useMemo(() => {
    if (single) return null;
    const rs = paths.map((p) => findRule(settings.openRules, p));
    return rs[0] && rs.every((r) => r === rs[0]) ? rs[0] : null;
  }, [settings.openRules, paths, single]);
  const current = terminal ? settings.terminal.trim() : editor ? settings.editor.trim() : rule?.command ?? "";
  // Where Ctrl+Enter applies; Tab cycles. Starts at the rule in effect, so it is replaced.
  const [gitRoot, setGitRoot] = useState<string | null>(null);
  const scopes = useMemo<Scope[]>(() => (single ? [] : scopesFor(paths, gitRoot)), [paths, gitRoot, single]);
  const [scopeIdx, setScopeIdx] = useState<number | null>(null);
  const si = scopeIdx ?? Math.max(0, rule ? scopes.findIndex((sc) => sameScope(rule, sc)) : 0);
  const scope: Scope | undefined = scopes[si];
  const ruleScope = rule ? scopes.find((sc) => sameScope(rule, sc))?.label ?? "a wider rule" : "";
  const subject = paths.length === 1 ? `“${basename(paths[0])}”` : `${paths.length} files`;

  useEffect(() => {
    let alive = true;
    const load = terminal ? api.terminalApps()
      // Editors: the known ones, then whatever else is registered for the file.
      : editor ? Promise.all([api.editorApps(), api.openWithApps(paths[0]).catch(() => [])]).then(([ed, reg]) => [...ed, ...reg.filter((r) => !ed.some((e) => e.name === r.name))].map((a) => ({ ...a, default: false })))
      : api.openWithApps(paths[0]);
    load.then((res) => { if (alive) setApps(res); }).catch((e) => { if (alive) { A.fail(e); setApps([]); } });
    if (!single) {
      api.gitRoot(paths[0]).then((r) => { if (alive) setGitRoot(r); }).catch(() => {});
      api.openWithAllApps().then((res) => { if (alive) setAllApps(res); }).catch(() => { if (alive) setAllApps([]); });
    }
    inputRef.current?.focus();
    return () => { alive = false; };
  }, [paths, terminal, editor, single]);

  const otherApps = useMemo(() => {
    if (single || !allApps) return [];
    const listed = apps ?? [];
    return allApps.filter((a) => a.command !== current && !listed.some((r) => r.command === a.command || r.id === a.id));
  }, [single, allApps, apps, current]);

  const rows = useMemo<Row[]>(() => {
    const list = apps ?? [];
    const query = q.trim();
    const out: Row[] = [];
    const add = (app: AppChoice, tag: string, star = false, other = false) => {
      const m = fuzzyMatch(query, app.name);
      if (m) out.push({ kind: "app", app, tag, star, positions: m.positions, other });
    };
    // The current default first: the stored rule / terminal, even if it isn't in the list.
    const known = list.find((a) => a.command === current);
    if (current) add(known ?? { id: current, name: single ? current : rule!.name, default: false, command: current }, terminal ? "your terminal" : editor ? "your editor" : `for ${ruleScope}`, true);
    for (const a of list) {
      if (a.command === current) continue;
      add(a, a.default ? (terminal ? "auto-detected" : "system default") : "");
    }
    if (!single && !showAll && !query && otherApps.length > 0) {
      out.push({ kind: "more", count: otherApps.length });
    }
    if (!single && (showAll || query)) {
      for (const a of otherApps) {
        add(a, showAll && !query ? tt("all applications") : "", false, true);
      }
    }
    if (!single && (rule || !list.some((a) => a.default)) && fuzzyMatch(query, "System default"))
      out.push({ kind: "system", tag: rule ? `forget ${rule.name}` : "" });
    if (!single && info?.platform === "windows" && paths.length === 1 && fuzzyMatch(query, "Windows dialog")) out.push({ kind: "dialog" });
    if (query) out.push({ kind: "custom", command: query });
    return out;
  }, [apps, otherApps, showAll, q, current, rule, ruleScope, terminal, editor, single, info, paths.length]);

  const cur = rows[Math.min(idx, rows.length - 1)];
  const close = () => A.openOverlay(null);
  const canOsDefault = !single && cur?.kind === "app" && canSetOsDefault(cur.app, info?.platform);

  const setOsDefault = async (app: AppChoice, thenOpen: boolean) => {
    try {
      const target = await api.openWithSetDefault(paths[0], app.id);
      A.notify(tt("{name} is now the system default for {target}", { name: app.name, target }), "success");
      setApps((prev) => {
        const base = prev ?? [];
        const exists = base.some((a) => a.id === app.id);
        const next = exists ? base.map((a) => ({ ...a, default: a.id === app.id })) : [{ ...app, default: true }, ...base.map((a) => ({ ...a, default: false }))];
        return next.sort((a, b) => Number(b.default) - Number(a.default));
      });
      if (thenOpen) {
        close();
        api.openWithRun(paths, app.command).catch(A.fail);
      }
    } catch (e) {
      A.fail(e);
    }
  };

  const accept = async (row: Row | undefined, always: boolean) => {
    if (!row) return;
    if (row.kind === "more") {
      setShowAll(true);
      inputRef.current?.focus();
      return;
    }
    close();
    if (row.kind === "dialog") {
      api.openWithDialog(paths[0]).catch(A.fail);
      return;
    }
    const command = row.kind === "app" ? row.app.command : row.kind === "custom" ? row.command : "system";
    const name = row.kind === "app" ? row.app.name : row.kind === "custom" ? row.command.split(/\s+/)[0] : "the system default";
    if (terminal) {
      if (always) {
        A.updateSettings({ terminal: command });
        A.notify(`Terminal here now opens ${name}`, "success");
      }
      api.openTerminalWith(paths[0], command).catch(A.fail);
      return;
    }
    if (editor) {
      if (always) {
        A.updateSettings({ editor: command });
        A.notify(`Edit in editor now opens ${name}`, "success");
      }
      api.openWithRun(paths, command).catch(A.fail);
      return;
    }
    if (always) {
      if (!scope) A.notify(platformLabel("Ctrl+Enter remembers a program per file type; select files of one type"), "warn");
      else {
        const rest = settings.openRules.filter((r) => !sameScope(r, scope));
        const next: OpenRule[] = command === "system" ? rest : [...rest, { target: scope.target, ...(scope.dir ? { dir: scope.dir } : {}), name, command }];
        A.updateSettings({ openRules: next });
        A.notify(command === "system" ? `System default again for ${scope.label}` : `${name} now opens ${scope.label}`, "success");
      }
    }
    api.openWithRun(paths, command).catch(A.fail);
  };

  const onKey = (e: React.KeyboardEvent) => {
    e.stopPropagation();
    const n = rows.length;
    const move = (d: number) => {
      e.preventDefault();
      if (n) setIdx((i) => (Math.min(i, n - 1) + d + n) % n);
    };
    if (e.key === "Escape") return close();
    // With an empty filter, j/k move and 1–9 pick (Ctrl = remember); once you type, they are text.
    if (!q && !e.altKey && !e.metaKey) {
      if (!e.ctrlKey && (e.key === "j" || e.key === "k")) return move(e.key === "j" ? 1 : -1);
      if (/^[1-9]$/.test(e.key) && Number(e.key) <= n) {
        e.preventDefault();
        accept(rows[Number(e.key) - 1], primaryMod(e));
        return;
      }
    }
    if (e.key === "Tab" && scopes.length > 1) {
      e.preventDefault();
      setScopeIdx((si + (e.shiftKey ? scopes.length - 1 : 1)) % scopes.length);
      return;
    }
    if (e.key === "ArrowDown" || (e.ctrlKey && (e.key === "j" || e.key === "n"))) return move(1);
    if (e.key === "ArrowUp" || (e.ctrlKey && (e.key === "k" || e.key === "p"))) return move(-1);
    if (e.key === "Enter") {
      e.preventDefault();
      if (e.altKey && canOsDefault && cur?.kind === "app") {
        void setOsDefault(cur.app, true);
        return;
      }
      accept(cur, primaryMod(e));
    }
  };

  const alwaysHint = terminal ? "always use as terminal" : editor ? "always use for Edit" : scope ? "always for" : "always (one file type)";
  return (
    <div className="backdrop glass" onMouseDown={close}>
      <div className="qo ow" onMouseDown={(e) => e.stopPropagation()} tabIndex={-1} onKeyDown={onKey}>
        <div className="qo-input">
          <Icon name={terminal ? "terminal" : editor ? "rename" : "open"} size={17} />
          <span className="ow-title">{terminal ? `Terminal in “${basename(paths[0]) || paths[0]}” with` : editor ? `Edit ${subject} with` : `Open ${subject} with`}</span>
          <input ref={inputRef} value={q} spellCheck={false} placeholder={terminal ? "filter, or type a command: wezterm start --always-new-process --cwd {dir}" : "filter any app, or type a command: zed {path}"}
            onChange={(e) => { setQ(e.target.value); setIdx(0); }} />
        </div>
        <div className="qo-list">
          {apps === null && <div className="qo-empty">{tt("Looking for applications…")}</div>}
          {rows.map((r, i) => (
            <div key={i} className={`qo-row cmd ${r.kind === "more" ? "qo-wider" : ""} ${i === idx ? "on" : ""}`} onMouseMove={() => i !== idx && setIdx(i)} onClick={(e) => accept(r, primaryMod(e))}>
              <span className="ow-star">{r.kind === "app" && r.star ? "★" : ""}</span>
              {!q && i < 9 && r.kind !== "more" && <kbd className="ow-num">{i + 1}</kbd>}
              <span className="qo-name">
                {r.kind === "app" ? <Highlight text={r.app.name} positions={r.positions} />
                  : r.kind === "more" ? tt("Show all applications…")
                  : r.kind === "system" ? "System default"
                  : r.kind === "dialog" ? "Windows “Open with” dialog…"
                  : <>{tt("Run")} <code>{r.command}</code></>}
              </span>
              <span className="ow-tag">
                {r.kind === "app" ? r.tag
                  : r.kind === "more" ? tt("{count} more installed", { count: r.count })
                  : r.kind === "system" ? r.tag
                  : r.kind === "custom" ? (terminal ? "{dir} = the folder" : "{path} = the file, {dir} = its folder") : ""}
              </span>
            </div>
          ))}
          {apps !== null && rows.length === 0 && <div className="qo-empty">{tt("No match. Type a full command to run it.")}</div>}
        </div>
        <div className="qo-foot">
          <span className="qo-status">{!q ? "j/k move · 1–9 pick · " : ""}{terminal ? "Terminal here (F10) uses ★" : editor ? "Edit in editor uses ★" : rule ? `★ opens these on Enter (${ruleScope})` : "Enter on a file uses the system default"}</span>
          <span className="qo-hints">
            <kbd>↵</kbd> open <kbd>{platformLabel("Ctrl ↵")}</kbd> {alwaysHint}
            {scope && (
              <button className="chip on" title={tt("Where Ctrl+Enter applies (Tab / Shift+Tab)")} onClick={() => { setScopeIdx((si + 1) % scopes.length); inputRef.current?.focus(); }}>
                {scope.label}{scopes.length > 1 && <kbd>Tab</kbd>}
              </button>
            )}
            {canOsDefault && cur?.kind === "app" && (
              <button className="chip" title={tt("Set as the operating system default for this file type (Alt+Enter)")} onClick={() => { void setOsDefault(cur.app, false); inputRef.current?.focus(); }}>
                {tt("OS default")} <kbd>Alt ↵</kbd>
              </button>
            )}
            <kbd>Esc</kbd> close
          </span>
        </div>
      </div>
    </div>
  );
}
