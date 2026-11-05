// Drag & drop between panes, tabs, breadcrumbs, favourites and other apps (O-11).
//
// WebKitGTK (Tauri on Linux) specifics handled here:
// * Only standard types (text/plain, text/uri-list) survive GTK drag & drop and a
//   drag without data may not start, so internal payloads are kept in memory and
//   every drag also sets standard data.
// * During a drag, event.ctrlKey/shiftKey/altKey reflect the last negotiated GTK
//   drag action, not the keys actually held (a pressed Ctrl "sticks"). The real
//   modifier state is therefore polled from the backend (GDK) while dragging.
//
// Semantics: a modifier changes the action only while it is held.
//   none  → default action (Settings › Behaviour › Default drop action)
//   Ctrl  → copy · Shift → move · Alt → symbolic link
//   ("ask" — show a menu — is available as a default action only)
import type React from "react";
import { getState } from "./store";
import { fail, named, notify, refreshUndo, showMenu, transfer } from "./actions";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api/ipc";
import { isInside } from "../core/path";

type Internal =
  | { kind: "files"; paths: string[] }
  | { kind: "tab"; b: number; idx: number }
  | { kind: "fav"; g: number; i: number };

export type HintKind = "move" | "copy" | "link" | "menu" | "compare" | "none";
export interface DragHint {
  x: number;
  y: number;
  kind: HintKind;
  text: string;
  tip: string;
}

interface Mods {
  known: boolean;
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
}

let current: Internal | null = null;
let mods: Mods = { known: false, ctrl: false, shift: false, alt: false };
let poll: number | null = null;

// Last described drop target; lets the hint update when only a key changes.
type EvMods = { ctrlKey: boolean; shiftKey: boolean; altKey: boolean };
let target: { dest: string; n: number; srcs: string[]; ev: EvMods } | null = null;
let pointer = { x: 0, y: 0 };
let noopText: string | null = null;
// A file dragged onto another file: dropping opens the comparison.
let compareDest: string | null = null;

// ───────── hint store ─────────

let hint: DragHint | null = null;
const hintSubs = new Set<() => void>();
export const getHint = () => hint;
export function subscribeHint(f: () => void) {
  hintSubs.add(f);
  return () => {
    hintSubs.delete(f);
  };
}
function setHint(h: DragHint | null) {
  hint = h;
  hintSubs.forEach((f) => f());
}

function baseName(p: string) {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? p;
}

function mountOf(p: string): string {
  let best = "";
  for (const d of getState().drives) if (isInside(p, d.mount) && d.mount.length > best.length) best = d.mount;
  return best;
}

/** The action a drop performs right now, from held keys and the default setting. */
export function currentAction(sources: string[], dest: string, ev?: { ctrlKey: boolean; shiftKey: boolean; altKey: boolean }): HintKind {
  // Use GDK state when available; otherwise fall back to the event flags.
  // The self-test sets __gallEventMods to drive modifiers through events.
  const useEvent = !mods.known || (globalThis as { __gallEventMods?: boolean }).__gallEventMods;
  const m = useEvent ? { ctrl: !!ev?.ctrlKey, shift: !!ev?.shiftKey, alt: !!ev?.altKey } : mods;
  if (m.alt) return "link";
  if (m.ctrl) return "copy";
  if (m.shift) return "move";
  switch (getState().settings?.dragBehavior ?? "consistent") {
    case "copy":
      return "copy";
    case "ask":
      return "menu";
    case "system":
      return sources.length && mountOf(sources[0]) !== mountOf(dest) ? "copy" : "move";
    default:
      return "move";
  }
}

function describe(kind: HintKind, n: number, dest: string): { text: string; tip: string } {
  const items = n > 1 ? ` ${n} items` : "";
  const name = baseName(dest);
  switch (kind) {
    case "move":
      return { text: `Move${items} to ${name}`, tip: "hold Ctrl = copy · Alt = link" };
    case "copy":
      return { text: `Copy${items} to ${name}`, tip: "hold Shift = move · Alt = link" };
    case "link":
      return { text: `Link${items} in ${name}`, tip: "symbolic link" };
    case "menu":
      return { text: `Choose action in ${name}`, tip: "hold Ctrl = copy · Shift = move · Alt = link" };
    default:
      return { text: name, tip: "" };
  }
}

function refreshHint() {
  if (!current && !externalDrag) return setHint(null);
  if (current && current.kind !== "files") return setHint(null);
  if (compareDest) {
    setHint({ ...pointer, kind: "compare", text: `Compare with ${baseName(compareDest)}`, tip: "drop to see the differences" });
  } else if (target) {
    const kind = currentAction(target.srcs, target.dest, target.ev);
    const d = describe(kind, target.n, target.dest);
    setHint({ ...pointer, kind, ...d });
  } else {
    setHint({ ...pointer, kind: "none", text: noopText ?? "Drop on a folder", tip: "" });
  }
}

// ───────── modifier polling while dragging ─────────

function startPolling() {
  if (poll !== null) return;
  const tick = () =>
    api
      .modifiers()
      .then((m) => {
        const changed = m.known !== mods.known || m.ctrl !== mods.ctrl || m.shift !== mods.shift || m.alt !== mods.alt;
        mods = m;
        if (changed) refreshHint();
      })
      .catch(() => {});
  tick();
  poll = window.setInterval(tick, 60);
}

function stopPolling() {
  if (poll !== null) clearInterval(poll);
  poll = null;
  mods = { known: false, ctrl: false, shift: false, alt: false };
}

let externalDrag = false;
let raf = 0;

function endDrag() {
  current = null;
  externalDrag = false;
  target = null;
  noopText = null;
  compareDest = null;
  stopPolling();
  setHint(null);
}

if (typeof window !== "undefined") {
  window.addEventListener(
    "dragover",
    (e) => {
      // Capture phase: runs before targets. Reset the target; a target handler
      // will describe itself during this same event.
      pointer = { x: e.clientX, y: e.clientY };
      target = null;
      noopText = null;
      compareDest = null;
      if (!current) {
        const types = Array.from(e.dataTransfer?.types ?? []);
        externalDrag = types.includes("text/uri-list") || types.includes("Files");
        if (externalDrag) startPolling();
      }
      if (!raf)
        raf = requestAnimationFrame(() => {
          raf = 0;
          refreshHint();
        });
    },
    true,
  );
  window.addEventListener("dragend", () => setTimeout(endDrag, 0), true);
  window.addEventListener("drop", () => setTimeout(endDrag, 0), true);
  window.addEventListener("dragleave", (e) => {
    if (e.clientX <= 0 || e.clientY <= 0 || e.clientX >= window.innerWidth || e.clientY >= window.innerHeight) {
      // A drag from another app gets no dragend here, and no drop once it left: end it now,
      // or the modifier keys would be polled every 60 ms until the next drag. (It starts
      // again with the next dragover if the pointer comes back.)
      if (!current && externalDrag) return endDrag();
      target = null;
      setHint(null);
    }
  });
}

// ───────── starting drags ─────────

function begin(e: React.DragEvent, payload: Internal, text: string, uris?: string) {
  current = payload;
  try {
    e.dataTransfer.setData("text/plain", text);
    if (uris) e.dataTransfer.setData("text/uri-list", uris);
  } catch {
    /* the in-memory payload is enough */
  }
  e.dataTransfer.effectAllowed = "all";
  if (payload.kind === "files") startPolling();
}

/** A system drag is under way (Windows, macOS): see [`startDrag`]. */
let nativeActive = false;

export function startDrag(e: React.DragEvent, paths: string[]) {
  // Windows and macOS: the page's drag carries only text, which Explorer / Finder and
  // mail cannot take as files. The app starts a system drag of the files instead; drops
  // inside Gall still see it as our own drag (`current`).
  const platform = getState().info?.platform;
  if (platform === "windows" || platform === "macos") {
    e.preventDefault();
    current = { kind: "files", paths };
    nativeActive = true;
    startPolling();
    api.startNativeDrag(paths)
      .catch((err) => api.uiLog("warn", `drag out: ${err}`).catch(() => {}))
      .finally(() => {
        nativeActive = false;
        setTimeout(endDrag, 0);
      });
    return;
  }
  begin(e, { kind: "files", paths }, paths.join("\n"), paths.map(fileUri).join("\r\n"));
}

/** `file://` URI of an absolute (Unix) path. Each name is encoded whole: `encodeURI` keeps
 *  "#" and "?", so a receiving app read "Track #1.mp3" as "Track " plus a fragment. */
export function fileUri(path: string): string {
  return "file://" + path.split("/").map(encodeURIComponent).join("/");
}

export function startTabDrag(e: React.DragEvent, b: number, idx: number, label: string) {
  begin(e, { kind: "tab", b, idx }, label);
}

export function startFavDrag(e: React.DragEvent, g: number, i: number, path: string) {
  begin(e, { kind: "fav", g, i }, path);
}

export function draggedTab(): { b: number; idx: number } | null {
  return current?.kind === "tab" ? { b: current.b, idx: current.idx } : null;
}

export function draggedFav(): { g: number; i: number } | null {
  return current?.kind === "fav" ? { g: current.g, i: current.i } : null;
}

/** Paths being dragged: our own drag, or files from another app (uri-list). */
export function draggedPaths(dt: DataTransfer): string[] {
  if (current?.kind === "files") return current.paths;
  if (current) return [];
  const uris = dt.getData("text/uri-list");
  if (uris) {
    return uris
      .split(/\r?\n/)
      .map((l) => l.trim())
      .filter((l) => l.startsWith("file://"))
      .map((l) => decodeURIComponent(l.slice(7).replace(/^localhost/, "")));
  }
  return [];
}

export function isFileDrag(e: React.DragEvent): boolean {
  if (current) return current.kind === "files";
  const t = Array.from(e.dataTransfer.types ?? []);
  return t.includes("text/uri-list") || t.includes("Files");
}

// ───────── targets ─────────

/** Marks `dest` as a valid drop target and describes the drop in the hint. */
export function onDragOverTarget(e: React.DragEvent, dest: string) {
  if (!isFileDrag(e)) return false;
  const own = current?.kind === "files" ? current.paths : [];
  if (own.some((p) => isInside(dest, p))) {
    e.dataTransfer.dropEffect = "none";
    noopText = "Cannot drop a folder into itself";
    return false;
  }
  e.preventDefault();
  e.stopPropagation();
  const kind = currentAction(own, dest, e);
  // Never "link" (see onDragOverCompare). A system drag of ours allows only "copy" (for
  // the apps it goes to), and a drop with another effect would be refused: our hint shows
  // the real action, and the drop performs it.
  e.dataTransfer.dropEffect = kind === "move" && !nativeActive ? "move" : "copy";
  target = { dest, n: own.length || 1, srcs: own, ev: { ctrlKey: e.ctrlKey, shiftKey: e.shiftKey, altKey: e.altKey } };
  return true;
}

/** The single file being dragged (our own drag), if exactly one item is dragged. */
export function draggedSingle(): string | null {
  return current?.kind === "files" && current.paths.length === 1 ? current.paths[0] : null;
}

/** Hovering a file with another file: offer a comparison. Returns true when accepted. */
export function onDragOverCompare(e: React.DragEvent, dest: string, srcIsDir: boolean): boolean {
  const src = draggedSingle();
  if (!src || src === dest || srcIsDir) return false;
  e.preventDefault();
  e.stopPropagation();
  // Not "link": Wayland drag-and-drop knows only copy/move/ask, so WebKitGTK
  // refuses a "link" drop and the drop event never fires. Our own hint shows
  // the real action, so the cursor badge does not matter.
  e.dataTransfer.dropEffect = "copy";
  compareDest = dest;
  target = null;
  return true;
}

/** Describes a hovered spot where dropping does nothing (e.g. the source folder). */
export function hintNoop(text: string) {
  noopText = text;
}

/** Performs the drop into folder `dest`, using the keys held at the moment of release. */
export function dropInto(e: React.DragEvent, dest: string) {
  e.preventDefault();
  e.stopPropagation();
  // DataTransfer is only readable during the event: capture everything now.
  const paths = draggedPaths(e.dataTransfer).filter((p) => p !== dest);
  const ev = { ctrlKey: e.ctrlKey, shiftKey: e.shiftKey, altKey: e.altKey };
  const at = { x: e.clientX, y: e.clientY };
  if (!paths.length) {
    // Windows: files from Explorer come without paths; WebView2 tells them to the host,
    // which answers with an "external-drop" event (see src-tauri/src/windrop.rs).
    const files = e.dataTransfer.files;
    const wv = (window as { chrome?: { webview?: { postMessageWithAdditionalObjects?: (m: string, o: unknown) => void } } }).chrome?.webview;
    if (files?.length && wv?.postMessageWithAdditionalObjects) {
      const id = ++dropSeq;
      pendingDrops.set(id, { dest, ev, at });
      api.uiLog("info", `drop on ${dest}: asking the host for the paths of ${files.length} file(s)`).catch(() => {});
      wv.postMessageWithAdditionalObjects(`gall-drop:${id}`, files);
      // A drop that never gets its paths is forgotten.
      setTimeout(() => pendingDrops.delete(id), 10_000);
      return;
    }
    api.uiLog("warn", `drop on ${dest}: no paths (types: ${Array.from(e.dataTransfer.types ?? []).join(",")})`).catch(() => {});
    return;
  }
  performDrop(paths, dest, ev, at);
}

// Drops waiting for their paths from the host (Windows, files from other apps).
let dropSeq = 0;
const pendingDrops = new Map<number, { dest: string; ev: EvMods; at: { x: number; y: number } }>();
if (typeof window !== "undefined" && (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__) {
  listen<{ id: number; paths: string[] }>("external-drop", ({ payload }) => {
    const p = pendingDrops.get(payload.id);
    if (!p) return;
    pendingDrops.delete(payload.id);
    const paths = payload.paths.filter((x) => x !== p.dest);
    if (!paths.length) return notify("Nothing to drop: the dragged items are not files on this computer", "warn");
    performDrop(paths, p.dest, p.ev, p.at);
  }).catch(() => {});
}

/** Moves, copies or links `paths` into `dest`, by the keys held at the drop (`ev`). */
function performDrop(paths: string[], dest: string, ev: EvMods, at: { x: number; y: number }) {
  if (paths.some((p) => isInside(dest, p))) {
    notify("Cannot move or copy a folder into itself", "warn");
    return;
  }
  // Read the real modifier state once more at release time.
  api
    .modifiers()
    .catch(() => mods)
    .then((m) => {
      mods = m;
      const kind = currentAction(paths, dest, ev);
      if (kind === "link") {
        linkInto(paths, dest);
      } else if (kind === "menu") {
        showMenu(at.x, at.y, [
          { label: `Move ${named(paths)} here`, run: () => transfer("move", paths, dest) },
          { label: `Copy ${named(paths)} here`, run: () => transfer("copy", paths, dest) },
          { label: paths.length === 1 ? "Create symbolic link here" : "Create symbolic links here", run: () => linkInto(paths, dest) },
        ]);
      } else transfer(kind === "copy" ? "copy" : "move", paths, dest);
    });
}

/** Symbolic links to `paths` in `dest` (Alt-drop or the drop menu). A failure is said in
 *  words; the menu item used to drop it silently. */
function linkInto(paths: string[], dest: string) {
  Promise.all(paths.map((p) => api.symlink(p, dest)))
    .then(() => notify(`Created ${paths.length === 1 ? "a link to" : "links to"} ${named(paths)} in ${baseName(dest)}`, "success"))
    .catch(fail)
    .finally(refreshUndo);
}

/** Kept for callers that only need copy/move (e.g. tests). */
export function dropEffect(e: React.DragEvent | DragEvent, sources: string[], dest: string): "copy" | "move" {
  return currentAction(sources, dest, e) === "copy" ? "copy" : "move";
}
