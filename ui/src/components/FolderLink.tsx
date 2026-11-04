// One of Gall's own folders (themes, settings, data, templates) as a link: the path
// itself, a click opens it in the active browser; the pane glyphs next to it pick the
// left / right browser. Right-click adds the system file manager; so does Shift+click.
import { api } from "../api/ipc";
import { tildify } from "../core/path";
import * as A from "../state/actions";
import { useStore } from "../state/store";
import { Icon } from "./Icon";
import { PanePick, useBrowserSide } from "./PanePick";

export function FolderLink({ path, label, before }: { path: string; label?: string; before?: () => Promise<unknown> }) {
  const home = useStore((s) => s.info?.home);
  const { dual, side } = useBrowserSide();
  const ready = async () => { await before?.().catch(() => {}); };
  const inGall = async (b?: number) => { await ready(); A.showFolder(path, b); };
  const inSystem = async () => { await ready(); api.openPath(path).catch((err) => A.notify(String(err), "error")); };
  if (!path) return null;

  const menu = (e: React.MouseEvent) => {
    e.preventDefault();
    A.showMenu(e.clientX, e.clientY, [
      ...(dual
        ? [0, 1].map((b) => ({ label: `Open in the ${side(b)} browser`, run: () => inGall(b) }))
        : [{ label: "Open in Gall", run: () => inGall() }]),
      { separator: true as const },
      { label: "Open in the system file manager", run: inSystem },
      { label: "Copy path", icon: "copy" as const, run: () => A.copyPaths([], path) },
    ]);
  };
  return (
    <span className="folder-link" onContextMenu={menu}>
      <button className="fl-main" title={`${path}\nClick: open in Gall · Shift+click: system file manager`} onClick={(e) => (e.shiftKey ? inSystem() : inGall())}>
        <Icon name="folder" size={14} />
        <span className="fl-text">{label ?? tildify(path, home)}</span>
      </button>
      <PanePick onPick={inGall} />
    </span>
  );
}
