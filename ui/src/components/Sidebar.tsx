// Sidebar: drives, favourite groups, history and the main toolbar (S-01…S-04, W-08).
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { platformLabel } from "../core/keymap";
import { t, tt } from "../i18n";
import { api } from "../api/ipc";
import type { CloudProvider, Drive, FavoriteGroup } from "../api/types";
import { groupDrives } from "../core/driveGroups";
import { formatSize } from "../core/format";
import { basename, isInside } from "../core/path";
import * as A from "../state/actions";
import { refreshAll } from "../state/controllers";
import { syncViewport } from "../state/desktop";
import { useDpr } from "../state/dpr";
import { snapCss } from "../core/pixelGrid";
import { draggedFav, draggedPaths, dropInto, isFileDrag, onDragOverTarget, startFavDrag } from "../state/dnd";
import { getState, monitorShown, setState, sidebarShown, useStore } from "../state/store";
import { useKeyLabels } from "../state/keyLabels";
import { FileIcon, Icon } from "./Icon";
import { PanePick } from "./PanePick";
import { Splitter, useStoredSize } from "./Splitter";
import { SystemMonitor } from "./SystemMonitor";


function favIcon(path: string, home: string) {
  if (path.startsWith("trash:")) return { name: "trash", color: "var(--fg-dim)" };
  if (path === home) return { name: "home", color: "var(--accent)" };
  return { name: "folder", color: "var(--folder)" };
}

function DrivesGroup({ current }: { current: string }) {
  const drives = useStore((s) => s.drives);
  const [hidden, setHidden] = useState(false);
  const b = A.activeBrowser();
  const best = drives.filter((d) => isInside(current, d.mount)).sort((x, y) => y.mount.length - x.mount.length)[0];
  const [collapsedGroups, setCollapsedGroups] = useState<string[]>(() => {
    try { const v = JSON.parse(localStorage.getItem("sidebar:driveGroups") ?? "[]"); return Array.isArray(v) ? v.filter((x) => typeof x === "string") : []; } catch { return []; }
  });
  const toggleGroup = (mount: string) => setCollapsedGroups((c) => {
    const next = c.includes(mount) ? c.filter((m) => m !== mount) : [...c, mount];
    localStorage.setItem("sidebar:driveGroups", JSON.stringify(next));
    return next;
  });
  const renderRow = (d: Drive, child: boolean, children?: Drive[]) => {
    const folded = collapsedGroups.includes(d.mount);
    // "~" on the root partition (or a btrfs /home subvolume) leaves the space to "/":
    // two bars with the same numbers read as two disks.
    const space = d.total > 0 && !d.spaceShownBy;
    const used = space ? 1 - d.available / d.total : 0;
    return (
      <div key={d.mount} className={`drive ${child ? "child" : ""} ${best?.mount === d.mount ? "current" : ""}`} title={`${d.label && d.label !== d.short ? `${d.label} — ` : ""}${d.mount}${d.fs ? ` · ${d.fs}` : ""}${d.spaceShownBy ? ` · same partition as ${d.spaceShownBy}` : ""}`}
        onClick={() => A.navigate(b, d.mount)}
        onMouseDown={(e) => { if (e.button === 1) { e.preventDefault(); A.sessionCmd({ op: "newTab", b, path: d.mount, activate: false }); } }}
        onDragOver={(e) => onDragOverTarget(e, d.mount)} onDrop={(e) => dropInto(e, d.mount)}
        onContextMenu={(e) => {
          e.preventDefault();
          A.showMenuFor(e, [
            { label: t("menu.openNewTab"), run: () => A.sessionCmd({ op: "newTab", b, path: d.mount }) },
            ...(A.hasOtherTarget(b) ? [{ label: t("menu.openOtherBrowser"), run: () => A.openInOther(b, d.mount) }] : []),
            { label: t("menu.eject"), icon: "eject", disabled: !d.removable, run: () => A.ejectDrive(d) },
            ...(d.lockable ? [{ label: t("menu.lock"), icon: "lock", run: () => A.lockDrive(d) }] : []),
          ]);
        }}>
        <Icon name={d.short === "~" ? "home" : "drive"} size={16} color={d.short === "~" ? "var(--accent)" : undefined} />
        {/* The label (Windows, USB stick …) only when it fits whole: a cut "W…" said nothing. */}
        <span className="drive-name">
          <b>{d.short}</b>{d.label && d.label !== d.short ? <span className="drive-label">{tt(d.label)}</span> : null}
        </span>
        <span className="meta">{space ? `${formatSize(d.total - d.available)} / ${formatSize(d.total)}` : ""}</span>
        {/* Own column at the right end, centred over the name and the usage bar, so
            the size lines up whether or not a drive can be ejected. */}
        <span className="drive-actions">
          {children?.length ? <button className="icon-btn drive-toggle" aria-expanded={!folded} title={folded ? t("sidebar.expand") : t("sidebar.collapse")} onClick={(e) => { e.stopPropagation(); toggleGroup(d.mount); }}><Icon name={folded ? "chevronRight" : "chevronDown"} size={14} /></button> : null}
          {d.lockable ? <button className="icon-btn" title={t("sidebar.unmountLock")} onClick={(e) => { e.stopPropagation(); A.lockDrive(d); }}><Icon name="lock" size={16} /></button> : null}
          {d.removable ? <button className="icon-btn" title={d.fs === "wsl" ? tt("Stop {short} (WSL)", { short: d.short }) : tt("Eject {value}", { value: d.label || d.short })} onClick={(e) => { e.stopPropagation(); A.ejectDrive(d); }}><Icon name="eject" size={16} /></button> : null}
        </span>
        {space && <div className="bar"><div className={used > 0.92 ? "full" : ""} style={{ width: `${used * 100}%` }} /></div>}
      </div>
    );
  };
  return (
    <div className="group">
      <div className="group-head" onDoubleClick={() => setHidden((h) => !h)}>
        <span className="title">{t("sidebar.drives")}</span>
        <button className="icon-btn" title={t("sidebar.more")} onClick={(e) => A.showMenuFor(e, [
          { label: t("sidebar.refreshAll"), shortcut: "F5", run: () => { A.refreshDrives(); refreshAll(); A.invalidateAllFolderSizes(); } },
          { label: hidden ? t("sidebar.showDrives") : t("sidebar.collapseGroup"), run: () => setHidden((h) => !h) },
        ])}><Icon name="more" /></button>
      </div>
      {!hidden && groupDrives(drives).map((g) => {
        const folded = collapsedGroups.includes(g.drive.mount);
        return (
          <div key={g.drive.mount} className="drive-group">
            {renderRow(g.drive, false, g.children)}
            {!folded && g.children.map((c) => renderRow(c, true))}
          </div>
        );
      })}
      {!hidden && <NotMounted />}
    </div>
  );
}

/** Volumes that are not mounted yet, folded into one quiet row until opened. Clicking
 *  one mounts it (or asks for its passphrase); it then moves up among the drives. */
function NotMounted() {
  const volumes = useStore((s) => s.volumes);
  const [open, setOpen] = useState(() => localStorage.getItem("sidebar:notMounted") === "1");
  const [busy, setBusy] = useState<string | null>(null);
  if (!volumes.length) return null;
  const locked = volumes.filter((v) => v.locked).length;
  const toggle = () => setOpen((o) => { localStorage.setItem("sidebar:notMounted", o ? "0" : "1"); return !o; });
  const mount = async (id: string) => {
    const v = volumes.find((x) => x.id === id);
    if (!v || busy) return;
    setBusy(id);
    await A.mountVolume(v);
    setBusy(null);
  };
  const powerOff = async (v: typeof volumes[number]) => {
    if (busy) return;
    setBusy(`off:${v.id}`); // not v.id: the row keeps its size instead of saying "mounting…"
    await A.powerOffVolume(v);
    setBusy(null);
  };
  return (
    <>
      <div className="drive not-mounted-head" onClick={toggle} title="">
        <Icon name={open ? "chevronDown" : "chevronRight"} size={12} />
        <span>{t("sidebar.notMounted")}</span>
        <span className="meta">{locked ? <><Icon name="lock" size={11} /> {locked} · </> : null}{volumes.length}</span>
      </div>
      {open && volumes.map((v) => (
        <div key={v.id} className={`drive not-mounted ${busy === v.id ? "busy" : ""}`} title={`${v.id}  (${v.fs})\n${v.locked ? tt("Click to unlock") : tt("Click to mount")}`}
          onClick={() => mount(v.id)}>
          <Icon name={v.locked ? "lock" : "drive"} size={16} />
          <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{v.label}</span>
          <span className="meta">{busy === v.id ? (v.locked ? "unlocking…" : v.fs === "wsl" ? "starting…" : "mounting…") : v.size ? formatSize(v.size) : v.fs}</span>
          {v.removable && v.fs !== "wsl" ? <button className="icon-btn" title={tt("Safely remove {value}", { value: v.label })} disabled={!!busy} onClick={(e) => { e.stopPropagation(); void powerOff(v); }}><Icon name="eject" size={16} /></button> : null}
        </div>
      ))}
    </>
  );
}

/** Tint per service, so several accounts are easy to tell apart (a plain cloud, no logos). */
const CLOUD_TINT: Record<CloudProvider, string> = {
  onedrive: "#4c9be8", dropbox: "#5b8cff", gdrive: "#e9b44c", icloud: "#8fb8f0", box: "#6aa7e0",
  mega: "#e06c75", pcloud: "#5ccfc0", nextcloud: "#5aa9e6", proton: "#a48cf0", other: "var(--fg-dim)",
};

/** Cloud folders found on this computer (OneDrive, Dropbox, …): nothing to pin by hand. */
function CloudGroup({ current, dual }: { current: string; dual: boolean }) {
  const cloud = useStore((s) => s.cloud);
  const [collapsed, setCollapsed] = useState(false);
  const [dropOn, setDropOn] = useState<string | null>(null);
  const b = A.activeBrowser();
  if (!cloud.length) return null;
  return (
    <div className="group">
      <div className="group-head" onDoubleClick={() => setCollapsed((c) => !c)}>
        <span className="title">{t("sidebar.cloud")}</span>
        <button className="icon-btn" title={t("sidebar.cloudOptions")} onClick={(e) => A.showMenuFor(e, [
          { label: t("sidebar.cloudRescan"), run: () => A.refreshCloud() },
          { label: collapsed ? t("sidebar.expand") : t("sidebar.collapse"), run: () => setCollapsed((c) => !c) },
          { label: t("sidebar.cloudHide"), run: () => A.updateSettings({ showCloud: false }) },
        ])}><Icon name="more" /></button>
      </div>
      {!collapsed && cloud.map((c) => (
        <div key={c.path} className={`fav ${isInside(current, c.path) ? "current" : ""} ${dropOn === c.path ? "drop" : ""}`}
          title={c.path}
          onClick={() => A.navigate(b, c.path)}
          onMouseDown={(e) => { if (e.button === 1) { e.preventDefault(); A.sessionCmd({ op: "newTab", b, path: c.path, activate: false }); } }}
          onDragOver={(e) => onDragOverTarget(e, c.path) && setDropOn(c.path)}
          onDragLeave={() => setDropOn(null)}
          onDrop={(e) => { setDropOn(null); dropInto(e, c.path); }}
          onContextMenu={(e) => {
            e.preventDefault();
            A.showMenuFor(e, [
              { label: t("menu.openNewTab"), run: () => A.sessionCmd({ op: "newTab", b, path: c.path }) },
              ...(A.hasOtherTarget(b) ? [{ label: t("menu.openOtherBrowser"), run: () => A.openInOther(b, c.path) }] : []),
              { separator: true },
              { label: t("menu.showInFileManager"), run: () => api.revealPath(c.path).catch(A.fail) },
              { label: t("menu.copyPath"), run: () => A.copyPaths([], c.path) },
            ]);
          }}>
          <Icon name="cloud" size={16} color={CLOUD_TINT[c.provider]} />
          <span className="label">{c.name}</span>
          {dual && <PanePick className="pick" onPick={(t) => A.navigate(t, c.path)} />}
        </div>
      ))}
    </div>
  );
}

/** GVFS network mounts (SMB, SFTP, NFS, WebDAV, FTP…). Hidden when there are none, except on
 *  Linux where the "+" button stays reachable to connect to a server. */
function NetworkGroup({ current }: { current: string }) {
  const mounts = useStore((s) => s.networkMounts);
  const linux = useStore((s) => s.info?.platform === "linux");
  const [collapsed, setCollapsed] = useState(false);
  const b = A.activeBrowser();
  useEffect(() => {
    if (!linux) return;
    void A.refreshNetwork();
    // Only the focused window polls: every other window (and every picker) catches up on focus.
    const t = setInterval(() => { if (!document.hidden && document.hasFocus()) void A.refreshNetwork(); }, 15_000);
    const onVisible = () => { if (!document.hidden) void A.refreshNetwork(); };
    document.addEventListener("visibilitychange", onVisible);
    window.addEventListener("focus", onVisible);
    return () => { clearInterval(t); document.removeEventListener("visibilitychange", onVisible); window.removeEventListener("focus", onVisible); };
  }, [linux]);
  if (!linux && !mounts.length) return null;
  return (
    <div className="group">
      <div className="group-head" onDoubleClick={() => setCollapsed((c) => !c)}>
        <span className="title">{t("sidebar.network")}</span>
        <button className="icon-btn" title={t("sidebar.networkOptions")} onClick={(e) => A.showMenuFor(e, [
          { label: t("sidebar.connectServer"), run: () => void A.connectToServer() },
          { label: t("sidebar.refresh"), run: () => void A.refreshNetwork() },
          { label: collapsed ? t("sidebar.expand") : t("sidebar.collapse"), run: () => setCollapsed((c) => !c) },
        ])}><Icon name="more" /></button>
        {linux && <button className="icon-btn" title={t("sidebar.connectServer")} onClick={() => void A.connectToServer()}><Icon name="plus" /></button>}
      </div>
      {!collapsed && !mounts.length && (
        <div className="fav" role="button" tabIndex={0} title={t("sidebar.connectServer")} style={{ color: "var(--fg-faint)", fontSize: "0.92em" }}
          onClick={() => void A.connectToServer()}
          onKeyDown={(e) => { if (e.key === "Enter") void A.connectToServer(); }}>
          <Icon name="plus" size={16} color="var(--fg-faint)" />
          <span className="label">{t("sidebar.connectServer")}</span>
        </div>
      )}
      {!collapsed && mounts.map((m) => (
        <div key={m.uri || m.mountPath} className={`fav ${m.mountPath && isInside(current, m.mountPath) ? "current" : ""}`}
          title={`${m.uri}\n${m.mountPath}`}
          onClick={() => m.mountPath && A.navigate(b, m.mountPath)}
          onMouseDown={(e) => { if (e.button === 1 && m.mountPath) { e.preventDefault(); A.sessionCmd({ op: "newTab", b, path: m.mountPath, activate: false }); } }}
          onContextMenu={(e) => {
            e.preventDefault();
            A.showMenuFor(e, [
              { label: t("menu.openNewTab"), disabled: !m.mountPath, run: () => A.sessionCmd({ op: "newTab", b, path: m.mountPath }) },
              ...(A.hasOtherTarget(b) && m.mountPath ? [{ label: t("menu.openOtherBrowser"), run: () => A.openInOther(b, m.mountPath) }] : []),
              { label: t("sidebar.disconnect"), icon: "eject", run: () => void A.disconnectNetwork(m) },
            ]);
          }}>
          <Icon name="cloud" size={16} color="var(--fg-dim)" />
          <span className="label">{m.name}<span className="meta" style={{ marginLeft: 6 }}>{m.scheme}</span></span>
          <button className="icon-btn" title={t("sidebar.disconnectNamed", { name: m.name })} onClick={(e) => { e.stopPropagation(); void A.disconnectNetwork(m); }}><Icon name="eject" size={16} /></button>
        </div>
      ))}
    </div>
  );
}

function FavGroup({ g, current, dual }: { g: FavoriteGroup; current: string; dual: boolean }) {
  const home = useStore((s) => s.info?.home ?? "");
  const [dropIdx, setDropIdx] = useState<number | null>(null);
  const [dropGroup, setDropGroup] = useState(false);
  const [dropOn, setDropOn] = useState<number | null>(null);
  const b = A.activeBrowser();

  const groupMenu = (e: React.MouseEvent) =>
    A.showMenuFor(e, [
      { label: t("sidebar.addCurrent"), icon: "plus", run: () => A.favCmd({ op: "addItem", id: g.id, path: A.activePath(b) }) },
      { label: t("sidebar.renameGroup"), run: async () => { const n = await A.promptText(tt("Rename group"), g.name, "Rename"); if (n) A.favCmd({ op: "renameGroup", id: g.id, name: n }); } },
      { label: t("sidebar.moveUp"), run: () => A.favCmd({ op: "moveGroup", id: g.id, delta: -1 }) },
      { label: t("sidebar.moveDown"), run: () => A.favCmd({ op: "moveGroup", id: g.id, delta: 1 }) },
      { label: g.collapsed ? t("sidebar.expand") : t("sidebar.collapse"), run: () => A.favCmd({ op: "collapse", id: g.id, collapsed: !g.collapsed }) },
      { separator: true },
      { label: t("sidebar.deleteGroup"), danger: true, run: async () => { if (await A.confirm(tt("Delete group?"), tt("Delete favourites group \"{name}\"? Folders themselves are not affected.", { name: g.name }), "Delete", true)) A.favCmd({ op: "deleteGroup", id: g.id }); } },
    ]);

  return (
    <div
      className={`group ${dropGroup ? "drop" : ""}`}
      onDragOver={(e) => {
        if (draggedFav()) { e.preventDefault(); return; }
      }}
      onDrop={(e) => {
        const src = draggedFav();
        if (src) {
          e.preventDefault();
          A.favCmd({ op: "moveItem", fromGroup: src.g, fromIndex: src.i, toGroup: g.id, toIndex: dropIdx ?? g.items.length });
          setDropIdx(null);
        }
      }}
    >
      <div
        className="group-head"
        onDoubleClick={() => A.favCmd({ op: "collapse", id: g.id, collapsed: !g.collapsed })}
        onDragOver={(e) => {
          if (isFileDrag(e)) { e.preventDefault(); e.stopPropagation(); e.dataTransfer.dropEffect = "copy"; setDropGroup(true); }
        }}
        onDragLeave={() => setDropGroup(false)}
        onDrop={(e) => {
          // Dropping a folder on the group title adds it as favourite (S-03).
          setDropGroup(false);
          if (!isFileDrag(e)) return;
          e.preventDefault();
          e.stopPropagation();
          draggedPaths(e.dataTransfer).forEach((p) => A.favCmd({ op: "addItem", id: g.id, path: p }));
        }}
      >
        <span className="title" title={g.name}>{g.name}</span>
        <button className="icon-btn" title={t("sidebar.groupOptions")} onClick={groupMenu}><Icon name="more" /></button>
        <button className="icon-btn" title={t("sidebar.addCurrentToGroup")} onClick={() => A.favCmd({ op: "addItem", id: g.id, path: A.activePath(b) })}><Icon name="plus" /></button>
      </div>
      {!g.collapsed && g.items.map((f, i) => {
        const real = A.resolveSpecial(f.path);
        const isTrash = f.path.startsWith("trash:");
        const ic = favIcon(f.path, home);
        const label = f.alias ? tt(f.alias) : basename(f.path);
        return (
          <div
            key={f.path}
            className={`fav ${real === current ? "current" : ""} ${dropOn === i ? "drop" : ""} ${dropIdx === i ? "insert-before" : ""}`}
            title={isTrash ? label : f.path}
            draggable
            onDragStart={(e) => startFavDrag(e, g.id, i, f.path)}
            onDragOver={(e) => {
              if (draggedFav()) {
                e.preventDefault();
                e.stopPropagation();
                setDropIdx(i);
              } else if (isTrash) {
                // Files dropped on Trash go to the trash (restorable), not into a folder.
                if (!isFileDrag(e)) return;
                e.preventDefault();
                e.stopPropagation();
                e.dataTransfer.dropEffect = "move";
                setDropOn(i);
              } else if (onDragOverTarget(e, real)) setDropOn(i);
            }}
            onDragLeave={() => { setDropOn(null); setDropIdx(null); }}
            onDrop={(e) => {
              setDropOn(null);
              const src = draggedFav();
              if (src) {
                e.preventDefault();
                e.stopPropagation();
                A.favCmd({ op: "moveItem", fromGroup: src.g, fromIndex: src.i, toGroup: g.id, toIndex: i });
                setDropIdx(null);
                return;
              }
              if (isTrash) {
                e.preventDefault();
                e.stopPropagation();
                A.trashPaths(draggedPaths(e.dataTransfer));
                return;
              }
              // Dropping files onto a favourite moves them into that folder (Ctrl copies).
              dropInto(e, real);
            }}
            onClick={() => A.navigate(b, f.path)}
            onMouseDown={(e) => {
              if (e.button === 1) {
                e.preventDefault();
                if (isTrash) A.navigate(b, f.path);
                else A.sessionCmd({ op: "newTab", b, path: real, activate: false });
              }
            }}
            onContextMenu={(e) => {
              e.preventDefault();
              A.showMenuFor(e, [
                ...(isTrash
                  ? [{ label: tt("Open"), run: () => A.navigate(b, f.path) }]
                  : [
                      { label: t("menu.openNewTab"), run: () => A.sessionCmd({ op: "newTab", b, path: real }) },
                      ...(A.hasOtherTarget(b) ? [{ label: t("menu.openOtherBrowser"), run: () => A.openInOther(b, real) }] : []),
                    ]),
                { separator: true },
                { label: t("sidebar.changeAlias"), run: async () => { const v = await A.promptText("Alias", label, "Save"); if (v !== null) A.favCmd({ op: "alias", id: g.id, index: i, alias: v }); } },
                { label: t("sidebar.removeAlias"), disabled: !f.alias, run: () => A.favCmd({ op: "alias", id: g.id, index: i, alias: null }) },
                { label: t("sidebar.removeFromGroup"), danger: true, disabled: isTrash, run: () => A.favCmd({ op: "removeItem", id: g.id, index: i }) },
              ]);
            }}
          >
            {ic.name === "folder" ? <FileIcon entry={{ isDir: true, ext: "", name: basename(real), path: real }} size={16} /> : <Icon name={ic.name} size={16} color={ic.color} />}
            <span className="label">{label}</span>
            {dual && !isTrash && <PanePick className="pick" onPick={(t) => A.navigate(t, f.path)} />}
          </div>
        );
      })}
    </div>
  );
}

function HistoryGroup({ current }: { current: string }) {
  const history = useStore((s) => s.history);
  const [collapsed, setCollapsed] = useState(false);
  const b = A.activeBrowser();
  if (!history.length) return null;
  return (
    <div className="group sidebar-history">
      <div className="group-head" onDoubleClick={() => setCollapsed((c) => !c)}>
        <Icon name="history" size={16} />
        <span className="title">{t("sidebar.history")}</span>
        <button className="icon-btn" title={t("sidebar.historyOptions")} onClick={(e) => A.showMenuFor(e, [
          { label: collapsed ? t("sidebar.expand") : t("sidebar.collapse"), run: () => setCollapsed((c) => !c) },
          { label: t("sidebar.historyClear"), danger: true, run: () => api.historyRemove(null).then(() => setState({ history: [] })).catch(A.fail) },
          { label: t("sidebar.historyHide"), run: () => A.updateSettings({ showHistory: false }) },
        ])}><Icon name="more" /></button>
      </div>
      {!collapsed && history.map((p) => (
        <div key={p} className={`fav ${p === current ? "current" : ""}`} title={p} onClick={() => A.navigate(b, p)}
          onMouseDown={(e) => { if (e.button === 1) { e.preventDefault(); A.sessionCmd({ op: "newTab", b, path: p, activate: false }); } }}
          onDragOver={(e) => onDragOverTarget(e, p)} onDrop={(e) => dropInto(e, p)}
          onContextMenu={(e) => { e.preventDefault(); A.showMenuFor(e, [
            { label: t("menu.openNewTab"), run: () => A.sessionCmd({ op: "newTab", b, path: p }) },
            { label: t("sidebar.historyRemove"), run: () => api.historyRemove(p).then(() => api.historyGet()).then((h) => setState({ history: h })).catch(A.fail) },
          ]); }}>
          <FileIcon entry={{ isDir: true, ext: "", name: basename(p), path: p }} size={16} />
          <span className="label">{basename(p)}</span>
        </div>
      ))}
    </div>
  );
}

export function MainToolbar() {
  const keys = useKeyLabels();
  const showLog = useStore((s) => s.showNotificationLog);
  const monitor = useStore(monitorShown);
  const crowded = useStore((s) => s.monitorCrowded && (s.settings?.showSystemMonitor ?? true));
  const monitorTitle = monitor ? tt("Hide system monitor") : crowded ? tt("Show system monitor (hidden to make room for History)") : tt("Show system monitor");
  return (
    <div className="main-toolbar">
      <button className="icon-btn" title={t("sidebar.settings", { keys: "Ctrl+," })} onClick={() => A.openOverlay({ type: "settings" })}><Icon name="settings" /></button>
      <button className="icon-btn" title={t("sidebar.about")} onClick={() => A.openOverlay({ type: "about" })}><Icon name="info" /></button>

      <button className="icon-btn" title={tt("Search names and tags (F3)")} onClick={() => A.openOverlay({ type: "search", root: A.activePath(A.activeBrowser()) })}><Icon name="tag" /></button>
      <button className="icon-btn" title={tt("Keyboard shortcuts{value}", { value: keys.action("keyHelp") ? ` (${keys.action("keyHelp")})` : "" })} onClick={() => A.openOverlay({ type: "keys" })}><Icon name="wand" /></button>
      <button className={`icon-btn sidebar-monitor-toggle ${monitor ? "active" : ""}`} title={monitorTitle} aria-pressed={monitor} onClick={() => A.toggleSystemMonitor()}><Icon name="activity" /></button>
      <button className={`icon-btn ${showLog ? "active" : ""}`} title={t("sidebar.notifications", { keys: "~" })} onClick={() => setState({ showNotificationLog: !showLog })}><Icon name="bell" /></button>
      <span className="spacer" />
      <button className="icon-btn" title={t("sidebar.collapseSidebar", { keys: "Alt+S" })} onClick={() => A.toggleSidebar(false)}><Icon name="chevronLeft" /></button>
    </div>
  );
}

export function Sidebar() {
  const visible = useStore(sidebarShown);
  const settings = useStore((s) => s.settings)!;
  const favorites = useStore((s) => s.favorites);
  const session = useStore((s) => s.session);
  const dpr = useDpr();
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const monitorRef = useRef<HTMLDivElement>(null);
  // Hidden only for lack of room: the saved setting still asks for it.
  const monitorHidden = useStore((s) => (s.settings?.showSystemMonitor ?? true) && !monitorShown(s));

  useLayoutEffect(() => {
    const scroll = scrollRef.current;
    const content = contentRef.current;
    const monitor = monitorRef.current;
    if (!scroll || !content || !monitor) return;

    const measure = () => {
      const rows = content.querySelectorAll(".sidebar-history > .fav");
      const last = rows[Math.min(rows.length, 5) - 1];
      const { monitorCrowded, monitorOverride } = getState();
      if (!monitor.firstElementChild) {
        // The monitor is turned off: nothing competes for room, and a stale override must not
        // keep it on screen in a crowded sidebar when it is turned on from Settings later.
        if (monitorCrowded || monitorOverride) setState({ monitorCrowded: false, monitorOverride: false });
        return;
      }
      let crowded = false;
      if (last) {
        // Compare with the space that would remain if the monitor were visible.
        // Its hidden wrapper stays measurable without taking up sidebar space.
        const displaced = monitor.classList.contains("is-space-hidden") ? monitor.getBoundingClientRect().height : 0;
        const viewport = scroll.getBoundingClientRect();
        const historyBottom = last.getBoundingClientRect().bottom - viewport.top + scroll.scrollTop;
        crowded = historyBottom > viewport.height - displaced + 0.5;
      }
      if (crowded && !monitorOverride && monitor.contains(document.activeElement)) {
        monitor.parentElement?.querySelector<HTMLButtonElement>(".sidebar-monitor-toggle")?.focus({ preventScroll: true });
      }
      // Once the monitor fits, a manual "show anyway" has done its job: a later shrink hides it again.
      const override = crowded && monitorOverride;
      if (crowded !== monitorCrowded || override !== monitorOverride) setState({ monitorCrowded: crowded, monitorOverride: override });
    };
    // Measure in the next frame: updating the store inside the observer callback changes the
    // layout it is still delivering and ends in "ResizeObserver loop" errors.
    let frame = 0;
    const schedule = () => { if (!frame) frame = requestAnimationFrame(() => { frame = 0; measure(); }); };
    const resize = new ResizeObserver(schedule);
    for (const element of [scroll, content, monitor]) resize.observe(element);
    const mutation = new MutationObserver(schedule);
    mutation.observe(content, { childList: true, subtree: true });
    measure();
    return () => { resize.disconnect(); mutation.disconnect(); cancelAnimationFrame(frame); };
  }, [visible]);

  const [w, setStoredW] = useStoredSize("sidebar", 230);
  const setW = (n: number) => { setStoredW(n); syncViewport(); };
  const [start, setStart] = useState(w);
  const current = session ? A.activePath(session.activeBrowser) : "";
  const dual = session ? session.layout !== "classic" && !session.browsers.some((x) => x.collapsed) : false;

  if (!visible) {
    return (
      <div className="sidebar-rail">
        <button className="icon-btn" title={t("sidebar.showSidebar", { keys: "Alt+S" })} onClick={() => A.toggleSidebar(true)}><Icon name="sidebar" /></button>
        <span className="vtitle">{basename(current)}</span>
      </div>
    );
  }
  return (
    <>
      <div className="sidebar" style={{ width: snapCss(w, dpr) }}>
        <div className="sb-search">
          <button className="search-pill" title={tt("Go to file ({value})", { value: platformLabel("Ctrl+P") })} onClick={() => A.openOverlay({ type: "quickOpen" })}>
            <Icon name="search" size={16} />
            <span>{tt("Search files…")}</span>
            <kbd>{platformLabel(tt("Ctrl P"))}</kbd>
          </button>
          <button className="icon-btn" title={platformLabel(tt("Find in files (Ctrl+F, Ctrl+Shift+F everywhere)"))} onClick={() => A.openOverlay({ type: "grep", root: A.activePath(A.activeBrowser()), scope: "folder" })}><Icon name="text" size={16} /></button>
        </div>
        <div className="sidebar-scroll" ref={scrollRef}>
          <div className="sidebar-content" ref={contentRef}>
            {settings.showDrives && <DrivesGroup current={current} />}
            {settings.showCloud && <CloudGroup current={current} dual={dual} />}
            {settings.showDrives && <NetworkGroup current={current} />}
            {favorites.groups.filter((g) => g.items.length || g.name !== A.PIN_GROUP).map((g) => <FavGroup key={g.id} g={g} current={current} dual={dual} />)}
            {settings.showHistory && <HistoryGroup current={current} />}
          </div>
        </div>
        <button className="new-group" onClick={async () => { const n = await A.promptText(tt("New favourites group"), tt("New group"), "Create"); if (n) A.favCmd({ op: "addGroup", name: n }); }}>{tt("New group")}</button>
        <div className={`sidebar-monitor${monitorHidden ? " is-space-hidden" : ""}`} ref={monitorRef}>
          <SystemMonitor />
        </div>
        <MainToolbar />
      </div>
      <Splitter dir="v" onDragStart={() => setStart(w)} onDrag={(d) => {
        const n = start + d;
        if (n < 90) { A.toggleSidebar(false); return; }
        setW(Math.max(150, Math.min(480, n)));
      }} />
    </>
  );
}
