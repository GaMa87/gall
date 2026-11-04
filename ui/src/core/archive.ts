// Archive names, mirroring crates/gall-core/src/fs/archive.rs (detect / formats).
export type ArchiveFormat = "zip" | "tar" | "tarGz" | "tarXz" | "tarBz2" | "tarZst" | "sevenZ" | "gz" | "xz" | "bz2" | "zst";

const SUFFIXES: [string, ArchiveFormat][] = [
  [".tar.gz", "tarGz"], [".tgz", "tarGz"], [".tar.xz", "tarXz"], [".txz", "tarXz"],
  [".tar.bz2", "tarBz2"], [".tbz2", "tarBz2"], [".tbz", "tarBz2"], [".tar.zst", "tarZst"], [".tzst", "tarZst"],
  [".tar", "tar"], [".zip", "zip"], [".7z", "sevenZ"], [".gz", "gz"], [".xz", "xz"], [".bz2", "bz2"], [".zst", "zst"],
];

/** Formats Gall can create, in menu order, with their extension. */
export const CREATE_FORMATS: [ArchiveFormat, string][] = [["zip", ".zip"], ["tarGz", ".tar.gz"], ["sevenZ", ".7z"]];

/** The archive format and the name without its archive extension, or null. */
export function archiveInfo(name: string): { format: ArchiveFormat; stem: string } | null {
  const lower = name.toLowerCase();
  for (const [suf, format] of SUFFIXES) {
    if (lower.length > suf.length && lower.endsWith(suf)) return { format, stem: name.slice(0, name.length - suf.length) };
  }
  return null;
}

/** Archives Enter opens as folders (single-file .gz/.xz/… hold one stream, not a tree). */
export function isBrowsableArchive(name: string): boolean {
  const f = archiveInfo(name)?.format;
  return !!f && f !== "gz" && f !== "xz" && f !== "bz2" && f !== "zst";
}

/** The archive a path lies in (`C:\a\photos.zip\2024` → `C:\a\photos.zip`), or null.
 *  By name: a real folder called "x.zip" would count too, which is rare. The archive
 *  file's own path counts as inside (its top level). */
export function archiveOf(path: string): string | null {
  const parts = path.split(/[\\/]/);
  let acc = "";
  for (let i = 0; i < parts.length; i++) {
    acc = i === 0 ? parts[0] : acc + (path[acc.length] ?? "/") + parts[i];
    if (i > 0 && isBrowsableArchive(parts[i])) return acc;
  }
  return null;
}

/** Format to create for a typed file name (by its extension), or null if it isn't one Gall writes. */
export function createFormatFor(name: string): ArchiveFormat | null {
  const f = archiveInfo(name)?.format;
  return f && CREATE_FORMATS.some(([c]) => c === f) ? f : null;
}

/** Base name for a new archive: the item's name (files lose their extension), or the folder name for several. */
export function archiveBaseName(items: { name: string; isDir: boolean }[], folderName: string): string {
  if (items.length !== 1) return folderName || "Archive";
  const { name, isDir } = items[0];
  if (isDir) return name;
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(0, dot) : name;
}
