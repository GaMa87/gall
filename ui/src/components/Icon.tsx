// UI line icons (24×24 grid, stroke based) are original. File/folder icons come from the
// Catppuccin icon theme (MIT), see core/fileIcons.ts and public/icons/catppuccin/LICENSE.
import { useEffect, useState, useSyncExternalStore, type CSSProperties } from "react";
import { fileIconId, folderEmblem, folderIconId, iconUrl, isPlainFolder } from "../core/fileIcons";
import { useStore } from "../state/store";

const P: Record<string, string> = {
  folder: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
  file: "M6 3h8l4 4v14H6z M14 3v4h4",
  image: "M4 5h16v14H4z M4 16l5-5 4 4 3-3 4 4 M15 9.5a1.5 1.5 0 1 0 0-.01",
  video: "M4 6h12v12H4z M16 10l5-3v10l-5-3",
  audio: "M9 18V6l10-2v12 M9 18a2.5 2.5 0 1 1-5 0 2.5 2.5 0 0 1 5 0 M19 16a2.5 2.5 0 1 1-5 0 2.5 2.5 0 0 1 5 0",
  pdf: "M6 3h8l4 4v14H6z M14 3v4h4 M8.5 15h7 M8.5 12h7 M8.5 18h4",
  text: "M6 3h8l4 4v14H6z M14 3v4h4 M9 11h6 M9 14h6 M9 17h4",
  code: "M6 3h8l4 4v14H6z M14 3v4h4 M10 12l-2 2 2 2 M14 12l2 2-2 2",
  archive: "M5 4h14v16H5z M11 4v2h2v2h-2v2h2v2h-2 M10 14h4v3h-4z",
  extract: "M4 9h16v11H4z M3 5h18v4H3z M12 12v6 M9 15l3 3 3-3",
  note: "M5 4h14v12l-4 4H5z M15 20v-4h4 M8 9h8 M8 12h6",
  link: "M10 14a4 4 0 0 0 5.66 0l3-3a4 4 0 0 0-5.66-5.66l-1 1 M14 10a4 4 0 0 0-5.66 0l-3 3a4 4 0 0 0 5.66 5.66l1-1",
  drive: "M3 14l2.5-8h13L21 14v4H3z M3 14h18 M17 16.5h.01",
  home: "M4 11l8-7 8 7v9h-5v-6H9v6H4z",
  trash: "M5 7h14 M9 7V4h6v3 M7 7l1 13h8l1-13",
  // A symmetric 8-tooth gear (generated: teeth at 22.5° + k·45°, root r 7.8, tip r 10.5) and its hub.
  settings: "M19.71 13.19L22.17 14.63L21.05 17.33L18.29 16.61L16.61 18.29L17.33 21.05L14.63 22.17L13.19 19.71L10.81 19.71L9.37 22.17L6.67 21.05L7.39 18.29L5.71 16.61L2.95 17.33L1.83 14.63L4.29 13.19L4.29 10.81L1.83 9.37L2.95 6.67L5.71 7.39L7.39 5.71L6.67 2.95L9.37 1.83L10.81 4.29L13.19 4.29L14.63 1.83L17.33 2.95L16.61 5.71L18.29 7.39L21.05 6.67L22.17 9.37L19.71 10.81z M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0z",
  info: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18z M12 11v6 M12 7.5v.01",
  layoutStd: "M3 4h18v16H3z M12 4v16",
  layoutCols: "M3 4h18v16H3z M3 12h18",
  layoutClassic: "M3 4h18v16H3z M3 8h18",
  search: "M10.5 17a6.5 6.5 0 1 0 0-13 6.5 6.5 0 0 0 0 13z M20 20l-4.8-4.8",
  wand: "M4 20L16 8 M14 4l1 2 2 1-2 1-1 2-1-2-2-1 2-1z M19 11l.7 1.3L21 13l-1.3.7L19 15l-.7-1.3L17 13l1.3-.7z",
  window: "M3 5h18v14H3z M3 9h18",
  plus: "M12 5v14 M5 12h14",
  close: "M6 6l12 12 M18 6L6 18",
  chevronRight: "M9 6l6 6-6 6",
  chevronDown: "M6 9l6 6 6-6",
  chevronUp: "M6 15l6-6 6 6",
  chevronLeft: "M15 6l-6 6 6 6",
  up: "M12 19V5 M6 11l6-6 6 6",
  back: "M19 12H5 M11 6l-6 6 6 6",
  forward: "M5 12h14 M13 6l6 6-6 6",
  swap: "M4 8h16 M16 4l4 4-4 4 M20 16H4 M8 12l-4 4 4 4",
  sort: "M7 4v16 M3 16l4 4 4-4 M17 20V4 M13 8l4-4 4 4",
  view: "M4 5h6v6H4z M14 5h6v6h-6z M4 15h6v4H4z M14 15h6v4h-6z",
  list: "M8 6h12 M8 12h12 M8 18h12 M4 6h.01 M4 12h.01 M4 18h.01",
  paste: "M9 4h6v3H9z M7 5H5v16h14V5h-2 M9 13h6 M9 17h4",
  more: "M5 12h.01 M12 12h.01 M19 12h.01",
  eject: "M5 15h14 M12 5l7 7H5z M5 19h14",
  lock: "M6 11h12v9H6z M8 11V8a4 4 0 0 1 8 0v3",
  down: "M12 5v14 M6 13l6 6 6-6",
  download: "M12 4v11 M7 10l5 5 5-5 M5 20h14",
  refresh: "M20 11a8 8 0 1 0-2.3 5.7 M20 4v7h-7",
  compare: "M8 4v16 M16 4v16 M3 8h5 M16 16h5 M5 6l-2 2 2 2 M19 14l2 2-2 2",
  activity: "M3 12h4l3-8 4 16 3-8h4",
  star: "M12 3l2.8 5.7 6.2.9-4.5 4.4 1 6.2L12 17.3 6.5 20.2l1-6.2L3 9.6l6.2-.9z",
  history: "M3 12a9 9 0 1 0 3-6.7 M3 4v5h5 M12 7v5l3 3",
  terminal: "M4 5h16v14H4z M7 9l3 3-3 3 M12 15h5",
  open: "M14 4h6v6 M20 4l-9 9 M18 14v6H4V6h6",
  columns: "M4 5h4v14H4z M10 5h4v14h-4z M16 5h4v14h-4z",
  crumbs: "M3 12h4 M10 12h4 M17 12h4 M6 9l2 3-2 3 M13 9l2 3-2 3",
  foldersTop: "M3 6a1 1 0 0 1 1-1h3l1 1h5a1 1 0 0 1 1 1v3a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1z M17 7v12 M14 16l3 3 3-3 M4 15h7 M4 19h7",
  copy: "M8 8h12v12H8z M16 8V4H4v12h4",
  cut: "M6 7a2.5 2.5 0 1 0 0-.01 M6 17a2.5 2.5 0 1 0 0-.01 M8 8l12 10 M8 16L20 6",
  rename: "M4 20h4L19 9l-4-4L4 16z M13 7l4 4",
  tag: "M3 12V4h8l10 10-8 8z M7.5 8a.5.5 0 1 0 0-.01",
  newFile: "M6 3h8l4 4v14H6z M14 3v4h4 M12 11v6 M9 14h6",
  newFolder: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z M12 10v6 M9 13h6",
  check: "M5 12l5 5 9-10",
  locate: "M12 21s-7-6.5-7-12a7 7 0 0 1 14 0c0 5.5-7 12-7 12z M12 11.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5",
  fit: "M4 9V4h5 M20 9V4h-5 M4 15v5h5 M20 15v5h-5",
  sidebar: "M3 4h18v16H3z M9 4v16",
  undo: "M9 14L4 9l5-5 M4 9h10a6 6 0 0 1 0 12h-3",
  pane: "M3 4h18v16H3z M15 4v16 M15 12h6",
  error: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18z M9 9l6 6 M15 9l-6 6",
  alert: "M12 4l9 16H3z M12 10v4.5 M12 17.5v.01",
  cloud: "M7 18a4.5 4.5 0 0 1-.6-8.96A6 6 0 0 1 17.8 8.5 4.75 4.75 0 0 1 17.25 18z",
  bell: "M6 16V11a6 6 0 0 1 12 0v5l2 2H4z M10 20a2 2 0 0 0 4 0",
  eye: "M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12z M12 9a3 3 0 1 0 0 6a3 3 0 1 0 0-6z",
};

export function Icon({ name, size = 16, color, style, title }: { name: string; size?: number; color?: string; style?: CSSProperties; title?: string }) {
  const d = P[name] ?? P.file;
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color ?? "currentColor"} strokeWidth={1.7}
      strokeLinecap="round" strokeLinejoin="round" style={{ flexShrink: 0, ...style }} aria-hidden={!title}>
      {title && <title>{title}</title>}
      <path d={d} />
    </svg>
  );
}

/** Tracks <html data-theme> so themed icon sets follow light/dark switches (incl. "system"). */
function subscribeTheme(cb: () => void) {
  const mo = new MutationObserver(cb);
  mo.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  return () => mo.disconnect();
}
export function useLightTheme(): boolean {
  return useSyncExternalStore(subscribeTheme, () => document.documentElement.dataset.theme === "light", () => false);
}

type IconEntry = { isDir: boolean; ext: string; isSymlink?: boolean; name?: string; path?: string };

// Filled folder in a 16×16 box, like Explorer's: a darker back with the tab, a lighter front.
const FOLDER_BACK = "M1 3.5A1.5 1.5 0 0 1 2.5 2h3.4a1.5 1.5 0 0 1 1.06.44L8.03 3.5h5.47A1.5 1.5 0 0 1 15 5v7.5a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 1 12.5z";
const FOLDER_FRONT = "M1 6.4A1.4 1.4 0 0 1 2.4 5h11.2A1.4 1.4 0 0 1 15 6.4v6.1a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 1 12.5z";
const FOLDER_FRONT_OPEN = "M3.3 6.5h11.3a.8.8 0 0 1 .77 1.02L13.9 12.9a1.5 1.5 0 0 1-1.44 1.1H2.1a.8.8 0 0 1-.78-.98l1.2-5.9a.8.8 0 0 1 .78-.62z";

/** A filled folder in `color` (default: the theme's folder colour), with the emblem of
 *  a special folder (src, .git, …) taken from its Catppuccin icon. */
export function FolderGlyph({ size, color, url, emblem, open = false }: { size: number; color?: string; url: string; emblem: boolean; open?: boolean }) {
  const [markup, setMarkup] = useState("");
  useEffect(() => {
    let live = true;
    if (emblem) folderEmblem(url).then((m) => live && setMarkup(m));
    else setMarkup("");
    return () => { live = false; };
  }, [url, emblem]);
  const c = color ?? "var(--folder)";
  return (
    <svg className="ficon-folder" data-tint={color} width={size} height={size} viewBox="0 0 16 16" aria-hidden>
      <path d={FOLDER_BACK} fill={`color-mix(in oklch, ${c} 80%, #000)`} />
      <path className="ff" d={open ? FOLDER_FRONT_OPEN : FOLDER_FRONT} fill={c} />
      {!open && <path d="M2.4 5.55h11.2" stroke="#fff" strokeOpacity={0.35} strokeWidth={0.6} strokeLinecap="round" />}
      {markup && (
        <>
          <g className="fe-halo" dangerouslySetInnerHTML={{ __html: markup }} />
          <g dangerouslySetInnerHTML={{ __html: markup }} />
        </>
      )}
    </svg>
  );
}

/** Catppuccin (Zed) icon for a file, or a filled folder; by name when known, else by extension. */
export function FileIcon({ entry, size = 16, open = false }: { entry: IconEntry; size?: number; open?: boolean }) {
  const light = useLightTheme();
  const tint = useStore((s) => (entry.isDir && entry.path ? s.settings?.folderColors[entry.path] : undefined));
  const name = entry.name ?? (entry.ext ? `x.${entry.ext}` : "x");
  const id = entry.isDir ? folderIconId(entry.name ?? "", open) : fileIconId(name);
  const url = iconUrl(id, light);
  return (
    <span className="ficon" style={{ position: "relative", display: "inline-flex", flexShrink: 0 }}>
      {entry.isDir ? (
        <FolderGlyph size={size} color={tint} url={url} emblem={!isPlainFolder(id)} open={open} />
      ) : (
        <img src={url} width={size} height={size} draggable={false} alt="" decoding="async" />
      )}
      {entry.isSymlink && (
        <span style={{ position: "absolute", right: -3, bottom: -3, background: "var(--bg-2)", borderRadius: 3, display: "inline-flex" }}>
          <Icon name="link" size={Math.max(9, size * 0.55)} color="var(--accent)" />
        </span>
      )}
    </span>
  );
}

export const TAG_COLORS = ["", "#e5534b", "#e8913a", "#e2c541", "#3fb27f", "#3d8bfd", "#a371f7", "#db61a2", "#8b949e"];
