// The checklist of the favorites import: which places start ticked, what is sent for the ticked ones, and the
// short texts. Pure, so it is unit-tested; the component only draws it.

/** What the backend returns for a place (see `favorites_import::Place`): skipped ones carry their reason. */
export type ImportKind = "folder" | "file" | { skipped: string };
export interface ImportPlace { path: string; label: string; hidden: boolean; kind: ImportKind; missing: boolean; duplicate: boolean }
export interface ImportSourceInfo { id: string; label: string }
export interface ImportItem { path: string; alias: string | null }
export interface ImportChoice { source: string; items: ImportItem[] }
export interface ImportedGroup { id: number; paths: string[] }
export interface ImportReceipt { groups: ImportedGroup[]; leftOut: number }

/** The reason a place cannot be taken over, or null. */
export const skippedReason = (place: ImportPlace): string | null => (typeof place.kind === "object" ? place.kind.skipped : null);

/** Whether the user can tick it: not skipped and not already a favorite. */
export const importable = (place: ImportPlace): boolean => skippedReason(place) === null && !place.duplicate;

/** Ticked at first: usable places, except those the other manager hides and those whose folder is gone. */
export const defaultTicked = (place: ImportPlace): boolean => importable(place) && !place.hidden && !place.missing;

/** The indexes of the places that start ticked. */
export function initialTicks(places: ImportPlace[]): Set<number> {
  return new Set(places.flatMap((place, i) => (defaultTicked(place) ? [i] : [])));
}

/** What is sent for one source: the ticked, still importable places in the order the source lists them, with the
 *  label as the alias. Null when nothing is ticked. */
export function choiceFor(source: string, places: ImportPlace[], ticked: ReadonlySet<number>): ImportChoice | null {
  const items = places.flatMap((place, i) => (ticked.has(i) && importable(place) ? [{ path: place.path, alias: place.label.trim() || null }] : []));
  return items.length ? { source, items } : null;
}

/** How many places can be ticked and how many are, for the footer. */
export function tally(places: ImportPlace[], ticked: ReadonlySet<number>): { usable: number; ticked: number } {
  return { usable: places.filter(importable).length, ticked: [...ticked].filter((i) => places[i] && importable(places[i])).length };
}

/** A short hint under the name of a source. */
export const SOURCE_HINTS: Record<string, string> = {
  "gtk-bookmarks": "Files (Nautilus), Thunar, Nemo, Caja and other GTK file managers",
  "kde-places": "Dolphin and the Places panel of KDE file dialogs",
};
