// Small rules of the dialog that applies a patch: what the user typed becomes the arguments of the core, the problems
// of a preview are sorted into what the user can fix in the dialog, and the offsets are put into words. Pure, so it is
// unit-tested; the dialog only draws it.
import type { PatchFormat } from "../api/types";

export type FormatChoice = "auto" | PatchFormat;

/** The format to read with: the one chosen, or (automatic) the one the text announces; null when it is not known yet. */
export function effectiveFormat(choice: FormatChoice, sniffed: PatchFormat | null): PatchFormat | null {
  return choice === "auto" ? sniffed : choice;
}

/** A file the patch dialog opens on (its menu in a folder, or the action with it focused): a `.patch` or a `.diff` file. */
export function isPatchFile(entry: { name: string; isDir: boolean }): boolean {
  return !entry.isDir && /\.(patch|diff)$/i.test(entry.name);
}

/** The strip count of the text field: empty is automatic (null); a whole number from 0 to 20 is itself; anything else is refused (undefined). */
export function parseStrip(text: string): number | null | undefined {
  const t = text.trim();
  if (t === "") return null;
  return /^\d{1,2}$/.test(t) && Number(t) <= 20 ? Number(t) : undefined;
}

/** A patch of a normal diff has no file name: the problem says so, and the dialog asks for the file. */
export function needsTarget(problems: readonly string[], format: PatchFormat | null): boolean {
  return format === "normal" || problems.some((p) => p.includes("without a file name"));
}

/** What an offset means: how far from the stated line a hunk was found. */
export function offsetText(offset: number): string {
  if (offset === 0) return "";
  const n = Math.abs(offset);
  return `${n} line${n === 1 ? "" : "s"} ${offset < 0 ? "earlier" : "later"} than stated`;
}

/** The hunks of a file with their offsets, in one short phrase for the preview list. */
export function hunkSummary(hunks: number, offsets: readonly number[]): string {
  const moved = offsets.filter((o) => o !== 0);
  const base = `${hunks} hunk${hunks === 1 ? "" : "s"}`;
  return moved.length ? `${base}, ${moved.length} found away from the stated line (${moved.map(offsetText).join("; ")})` : base;
}

/** The patch can be applied: it was previewed without problems, for this very text and settings. */
export function canApply(preview: { files: unknown[]; problems: string[] } | null, current: boolean): boolean {
  return !!preview && current && preview.problems.length === 0 && preview.files.length > 0;
}
