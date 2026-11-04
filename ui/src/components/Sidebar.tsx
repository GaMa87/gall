// Sidebar: drives, favourite groups, history and the main toolbar (S-01…S-04, W-08).
import { useState } from "react";
import { api } from "../api/ipc";
import type { CloudProvider, FavoriteGroup } from "../api/types";
import { formatSize } from "../core/format";
import { basename, isInside } from "../core/path";
import * as A from "../state/actions";
import { refreshAll } from "../state/controllers";
import { draggedFav, draggedPaths, dropInto, isFileDrag, onDragOverTarget, startFavDrag } from "../state/dnd";
import { setState, useStore } from "../state/store";
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
  return (
    <div className="group">
      <div className="group-head" onDoubleClick={() => setHidden((h) => !h)}>
        <span className="title">Drives</span>
        <button className="icon-btn" title="More" onClick={(e) => A.showMenu(e.clientX, e.clientY, [
          { label: "Refresh everything", shortcut: "F5", run: () => { A.refreshDrives(); refreshAll(); } },
          { label: hidden ? "Show drives" : "Collapse group", run: () => setHidden((h) => !h) },
        ])}><Icon name="more" /></button>
      </div>
      {!hidden && drives.map((d) => {
        // "~" on the root partition (or a btrfs /home subvolume) leaves the space to "/":
        // two bars with the same numbers read as two disks.
        const space = d.total > 0 && !d.spaceShownBy;
        const used = space ? 1 - d.available / d.total : 0;
        return (
          <div key={d.mount} className={`drive ${best?.mount === d.mount ? "current" : ""}`} title={`${d.label && d.label !== d.short ? `${d.label} — ` : ""}${d.mount}${d.fs ? ` · ${d.fs}` : ""}${d.spaceShownBy ? ` · same partition as ${d.spaceShownBy}` : ""}`}
            onClick={() => A.navigate(b, d.mount)}
            onMouseDown={(e) => { if (e.button === 1) { e.preventDefault(); A.sessionCmd({ op: "newTab", b, path: d.mount, activate: false }); } }}
            onDragOver={(e) => onDragOverTarget(e, d.mount)} onDrop={(e) => dropInto(e, d.mount)}
            onContextMenu={(e) => {
              e.preventDefault();
              A.showMenu(e.clientX, e.clientY, [
                { label: "Open in new tab", run: () => A.sessionCmd({ op: "newTab", b, path: d.mount }) },
                ...(A.hasOtherTarget(b) ? [{ label: "Open in other browser", run: () => A.openInOther(b, d.mount) }] : []),
                { label: "Eject", icon: "eject", disabled: !d.removable, run: () => A.ejectDrive(d) },
                ...(d.lockable ? [{ label: "Lock", icon: "lock", run: () => A.lockDrive(d) }] : []),
              ]);
            }}>
            <Icon name={d.short === "~" ? "home" : "drive"} size={16} color={d.short === "~" ? "var(--accent)" : undefined} />
            {/* The label (Windows, USB stick …) only when it fits whole: a cut "W…" said nothing. */}
            <span className="drive-name">
              <b>{d.short}</b>{d.label && d.label !== d.short ? <span className="drive-label">{d.label}</span> : null}
            </span>
            <span className="meta">{space ? `${formatSize(d.total - d.available)} / ${formatSize(d.total)}` : ""}</span>
            {/* Own column at the right end, centred over the name and the usage bar, so
                the size lines up whether or not a drive can be ejected. */}
            <span className="drive-actions">
              {d.lockable ? <button className="icon-btn" title="Unmount and lock" onClick={(e) => { e.stopPropagation(); A.lockDrive(d); }}><Icon name="lock" size={14} /></button> : null}
              {d.removable ? <button className="icon-btn" title={`Eject ${d.label || d.short}`} onClick={(e) => { e.stopPropagation(); A.ejectDrive(d); }}><Icon name="eject" size={14} /></button> : null}
            </span>
            {space && <div className="bar"><div className={used > 0.92 ? "full" : ""} style={{ width: `${used * 100}%` }} /></div>}
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
  return (
    <>
      <div className="drive not-mounted-head" onClick={toggle} title="Volumes that are not mounted yet: click one to mount or unlock it">
        <Icon name={open ? "chevronDown" : "chevronRight"} size={12} />
        <span>Not mounted</span>
        <span className="meta">{locked ? <><Icon name="lock" size={11} /> {locked} · </> : null}{volumes.length}</span>
      </div>
      {open && volumes.map((v) => (
        <div key={v.id} className={`drive not-mounted ${busy === v.id ? "busy" : ""}`} title={`${v.id}  (${v.fs})\n${v.locked ? "Click to unlock" : "Click to mount"}`}
          onClick={() => mount(v.id)}>
          <Icon name={v.locked ? "lock" : "drive"} size={16} />
          <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{v.label}</span>
          <span className="meta">{busy === v.id ? (v.locked ? "unlocking…" : "mounting…") : v.size ? formatSize(v.size) : v.fs}</span>
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
        <span className="title">Cloud</span>
        <button className="icon-btn" title="Cloud options" onClick={(e) => A.showMenu(e.clientX, e.clientY, [
          { label: "Look for cloud folders again", run: () => A.refreshCloud() },
          { label: collapsed ? "Expand" : "Collapse", run: () => setCollapsed((c) => !c) },
          { label: "Hide cloud folders", run: () => A.updateSettings({ showCloud: false }) },
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
            A.showMenu(e.clientX, e.clientY, [
              { label: "Open in new tab", run: () => A.sessionCmd({ op: "newTab", b, path: c.path }) },
              ...(A.hasOtherTarget(b) ? [{ label: "Open in other browser", run: () => A.openInOther(b, c.path) }] : []),
              { separator: true },
              { label: "Show in system file manager", run: () => api.revealPath(c.path).catch(A.fail) },
              { label: "Copy path", run: () => A.copyPaths([], c.path) },
            ]);
          }}>
          <Icon name="cloud" size={15} color={CLOUD_TINT[c.provider]} />
          <span className="label">{c.name}</span>
          {dual && <PanePick className="pick" onPick={(t) => A.navigate(t, c.path)} />}
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
    A.showMenu(e.clientX, e.clientY, [
      { label: "Add current folder", icon: "plus", run: () => A.favCmd({ op: "addItem", id: g.id, path: A.activePath(b) }) },
      { label: "Rename group…", run: async () => { const n = await A.promptText("Rename group", g.name, "Rename"); if (n) A.favCmd({ op: "renameGroup", id: g.id, name: n }); } },
      { label: "Move up", run: () => A.favCmd({ op: "moveGroup", id: g.id, delta: -1 }) },
      { label: "Move down", run: () => A.favCmd({ op: "moveGroup", id: g.id, delta: 1 }) },
      { label: g.collapsed ? "Expand" : "Collapse", run: () => A.favCmd({ op: "collapse", id: g.id, collapsed: !g.collapsed }) },
      { separator: true },
      { label: "Delete group", danger: true, run: async () => { if (await A.confirm("Delete group?", `Delete favourites group "${g.name}"? Folders themselves are not affected.`, "Delete", true)) A.favCmd({ op: "deleteGroup", id: g.id }); } },
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
        <button className="icon-btn" title="Group options" onClick={groupMenu}><Icon name="more" /></button>
        <button className="icon-btn" title="Add current folder to this group" onClick={() => A.favCmd({ op: "addItem", id: g.id, path: A.activePath(b) })}><Icon name="plus" /></button>
      </div>
      {!g.collapsed && g.items.map((f, i) => {
        const real = A.resolveSpecial(f.path);
        const isTrash = f.path.startsWith("trash:");
        const ic = favIcon(f.path, home);
        const label = f.alias ?? basename(f.path);
        return (
          <div
            key={f.path}
            className={`fav ${real === current ? "current" : ""} ${dropOn === i ? "drop" : ""} ${dropIdx === i ? "insert-before" : ""}`}
            title={f.path}
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
            onMouseDown={(e) => { if (e.button === 1) { e.preventDefault(); A.sessionCmd({ op: "newTab", b, path: real, activate: false }); } }}
            onContextMenu={(e) => {
              e.preventDefault();
              A.showMenu(e.clientX, e.clientY, [
                { label: "Open in new tab", run: () => A.sessionCmd({ op: "newTab", b, path: real }) },
                ...(A.hasOtherTarget(b) ? [{ label: "Open in other browser", run: () => A.openInOther(b, real) }] : []),
                { separator: true },
                { label: "Change alias…", run: async () => { const v = await A.promptText("Alias", label, "Save"); if (v !== null) A.favCmd({ op: "alias", id: g.id, index: i, alias: v }); } },
                { label: "Remove alias", disabled: !f.alias, run: () => A.favCmd({ op: "alias", id: g.id, index: i, alias: null }) },
                { label: "Remove from group", danger: true, disabled: f.path.startsWith("trash:"), run: () => A.favCmd({ op: "removeItem", id: g.id, index: i }) },
              ]);
            }}
          >
            {ic.name === "folder" ? <FileIcon entry={{ isDir: true, ext: "", name: basename(real), path: real }} size={15} /> : <Icon name={ic.name} size={15} color={ic.color} />}
            <span className="label">{label}</span>
            {dual && <PanePick className="pick" onPick={(t) => A.navigate(t, f.path)} />}
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
    <div className="group">
      <div className="group-head" onDoubleClick={() => setCollapsed((c) => !c)}>
        <Icon name="history" size={14} />
        <span className="title">History</span>
        <button className="icon-btn" title="History options" onClick={(e) => A.showMenu(e.clientX, e.clientY, [
          { label: collapsed ? "Expand" : "Collapse", run: () => setCollapsed((c) => !c) },
          { label: "Clear history", danger: true, run: () => api.historyRemove(null).then(() => setState({ history: [] })).catch(A.fail) },
          { label: "Hide history", run: () => A.updateSettings({ showHistory: false }) },
        ])}><Icon name="more" /></button>
      </div>
      {!collapsed && history.map((p) => (
        <div key={p} className={`fav ${p === current ? "current" : ""}`} title={p} onClick={() => A.navigate(b, p)}
          onMouseDown={(e) => { if (e.button === 1) { e.preventDefault(); A.sessionCmd({ op: "newTab", b, path: p, activate: false }); } }}
          onDragOver={(e) => onDragOverTarget(e, p)} onDrop={(e) => dropInto(e, p)}
          onContextMenu={(e) => { e.preventDefault(); A.showMenu(e.clientX, e.clientY, [
            { label: "Open in new tab", run: () => A.sessionCmd({ op: "newTab", b, path: p }) },
            { label: "Remove from history", run: () => api.historyRemove(p).then(() => api.historyGet()).then((h) => setState({ history: h })).catch(A.fail) },
          ]); }}>
          <FileIcon entry={{ isDir: true, ext: "", name: basename(p), path: p }} size={15} />
          <span className="label">{basename(p)}</span>
        </div>
      ))}
    </div>
  );
}

export function MainToolbar() {
  const showLog = useStore((s) => s.showNotificationLog);
  const monitor = useStore((s) => s.settings?.showSystemMonitor ?? true);
  return (
    <div className="main-toolbar">
      <button className="icon-btn" title="Settings (Ctrl+,)" onClick={() => A.openOverlay({ type: "settings" })}><Icon name="settings" /></button>
      <button className="icon-btn" title="About" onClick={() => A.openOverlay({ type: "about" })}><Icon name="info" /></button>

      <button className="icon-btn" title="Search names and tags (F3)" onClick={() => A.openOverlay({ type: "search", root: A.activePath(A.activeBrowser()) })}><Icon name="tag" /></button>
      <button className="icon-btn" title="Keyboard shortcuts (?)" onClick={() => A.openOverlay({ type: "keys" })}><Icon name="wand" /></button>
      <button className={`icon-btn ${monitor ? "active" : ""}`} title={monitor ? "Hide system monitor" : "Show system monitor"} onClick={() => A.updateSettings({ showSystemMonitor: !monitor })}><Icon name="activity" /></button>
      <button className={`icon-btn ${showLog ? "active" : ""}`} title="Notifications (~)" onClick={() => setState({ showNotificationLog: !showLog })}><Icon name="bell" /></button>
      <span className="spacer" />
      <button className="icon-btn" title="Collapse sidebar (Alt+S)" onClick={() => setState({ sidebarVisible: false })}><Icon name="chevronLeft" /></button>
    </div>
  );
}

export function Sidebar() {
  const visible = useStore((s) => s.sidebarVisible);
  const settings = useStore((s) => s.settings)!;
  const favorites = useStore((s) => s.favorites);
  const session = useStore((s) => s.session);
  const [w, setW] = useStoredSize("sidebar", 230);
  const [start, setStart] = useState(w);
  const current = session ? A.activePath(session.activeBrowser) : "";
  const dual = session ? session.layout !== "classic" && !session.browsers.some((x) => x.collapsed) : false;

  if (!visible) {
    return (
      <div className="sidebar-rail">
        <button className="icon-btn" title="Show sidebar (Alt+S)" onClick={() => setState({ sidebarVisible: true })}><Icon name="sidebar" /></button>
        <span className="vtitle">{basename(current)}</span>
      </div>
    );
  }
  return (
    <>
      <div className="sidebar" style={{ width: w }}>
        <div className="sb-search">
          <button className="search-pill" title="Go to file (Ctrl+P)" onClick={() => A.openOverlay({ type: "quickOpen" })}>
            <Icon name="search" size={14} />
            <span>Search files…</span>
            <kbd>Ctrl P</kbd>
          </button>
          <button className="icon-btn" title="Find in files (Ctrl+F, Ctrl+Shift+F everywhere)" onClick={() => A.openOverlay({ type: "grep", root: A.activePath(A.activeBrowser()), scope: "folder" })}><Icon name="text" size={15} /></button>
        </div>
        <div className="sidebar-scroll">
          {settings.showDrives && <DrivesGroup current={current} />}
          {settings.showCloud && <CloudGroup current={current} dual={dual} />}
          {favorites.groups.filter((g) => g.items.length || g.name !== A.PIN_GROUP).map((g) => <FavGroup key={g.id} g={g} current={current} dual={dual} />)}
          {settings.showHistory && <HistoryGroup current={current} />}
        </div>
        <button className="new-group" onClick={async () => { const n = await A.promptText("New favourites group", "New group", "Create"); if (n) A.favCmd({ op: "addGroup", name: n }); }}>New group</button>
        <SystemMonitor />
        <MainToolbar />
      </div>
      <Splitter dir="v" onDragStart={() => setStart(w)} onDrag={(d) => {
        const n = start + d;
        if (n < 90) { setState({ sidebarVisible: false }); return; }
        setW(Math.max(150, Math.min(480, n)));
      }} />
    </>
  );
}
