// Details pane: preview + properties with a quick toolbar (P-01, P-09).
import { useState } from "react";
import { formatDateTime, formatSize } from "../core/format";
import { age, ageWords } from "../core/age";
import * as A from "../state/actions";
import { useStore } from "../state/store";
import { FileIcon, Icon } from "./Icon";
import { Preview, usePreviewInfo } from "./Preview";
import { Splitter, useStoredSize } from "./Splitter";
import { basename } from "../core/path";

export function DetailsPane({ b, path, showToolbar = true }: { b: number; path: string; showToolbar?: boolean }) {
  const entry = useStore((s) => s.focused[b]);
  const info = usePreviewInfo(entry);
  const total = useStore((s) => (entry?.isDir ? s.folderSizes[entry.path] : undefined));
  const [propsH, setPropsH] = useStoredSize("details-props", 150);
  const [start, setStart] = useState(propsH);

  if (!entry) {
    return (
      <div className="details">
        <div className="preview"><div className="pv-icon"><FileIcon entry={{ isDir: true, ext: "", name: basename(path), path }} size={72} /><span>{basename(path)}</span></div></div>
      </div>
    );
  }
  const a = entry.modified != null ? age(entry.modified, Date.now()) : null;
  return (
    <div className="details">
      <div className="preview"><Preview entry={entry} info={info} /></div>
      <Splitter dir="h" onDragStart={() => setStart(propsH)} onDrag={(d) => setPropsH(Math.max(60, Math.min(500, start - d)))} />
      <div className="props" style={{ flexBasis: propsH }}>
        <div className="props-head">
          <span>{entry.isDir ? (info?.folderItems != null ? A.items(info.folderItems) : "… items") : info?.width ? `${info.width} × ${info.height}` : entry.ext.toUpperCase()}</span>
          <span title={entry.isDir ? A.folderSizeTitle(total) || "Files directly in this folder; Ctrl+Alt+S counts everything inside" : undefined}>
            {entry.isDir ? (total ? A.folderSizeLabel(total) : info?.folderBytes != null ? `${formatSize(info.folderBytes)} here` : "") : formatSize(entry.size)}
          </span>
        </div>
        <table>
          <tbody>
            <tr><td>Name</td><td>{entry.name}</td></tr>
            <tr><td>Type</td><td>{entry.isDir ? "Folder" : entry.ext ? `${entry.ext.toUpperCase()} file` : "File"}{entry.isSymlink ? " (link)" : ""}</td></tr>
            {entry.created != null && <tr><td>Created</td><td>{formatDateTime(entry.created)}</td></tr>}
            <tr><td>Modified</td><td>{formatDateTime(entry.modified)}</td></tr>
            {a && <tr><td>Age</td><td title={a.label}>{ageWords(entry.modified!, Date.now())}</td></tr>}
            {entry.readonly && <tr><td>Access</td><td>Read-only</td></tr>}
          </tbody>
        </table>
        {showToolbar && (
          <div className="props-toolbar">
            <button className="icon-btn" title="Cut (Ctrl+X)" onClick={() => A.copyToClipboard([entry], true)}><Icon name="cut" /></button>
            <button className="icon-btn" title="Copy (Ctrl+C)" onClick={() => A.copyToClipboard([entry], false)}><Icon name="copy" /></button>
            <button className="icon-btn" title="Rename" onClick={() => A.renameEntry(b, entry)}><Icon name="rename" /></button>
            <button className="icon-btn" title="Move to trash (Del)" onClick={() => A.trashPaths([entry.path])}><Icon name="trash" /></button>
            <button className="icon-btn" title="Colour tag (Ctrl+Alt+T)" onClick={() => A.openOverlay({ type: "tag", paths: [entry.path] })}><Icon name="tag" /></button>
          </div>
        )}
      </div>
    </div>
  );
}
