import type { ClipContent } from "../api/types";

const norm = (t: string) => t.split(/\r?\n/).map((l) => l.trim()).filter(Boolean).join("\n");

/** Does the system clipboard still hold the files Gall copied or cut (`ours`)?
 *  Gall puts their paths there as text, which reads back as paths, or as plain text
 *  once a file is gone. An unreadable or empty clipboard keeps ours; any other text,
 *  link, image or file list was copied later somewhere else and wins. */
export function ownsClipboard(ours: string[], ext: ClipContent): boolean {
  if (!ours.length) return false;
  const mine = norm(ours.join("\n"));
  switch (ext.kind) {
    case "empty": return true;
    case "paths": return norm(ext.paths.join("\n")) === mine;
    case "text": return norm(ext.text) === mine;
    case "url": return norm(ext.url) === mine;
    default: return false;
  }
}
