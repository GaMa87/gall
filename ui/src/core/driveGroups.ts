// Sidebar drive grouping: bind mounts and btrfs subvolumes (pkg, log …) live on the same
// partition as another row, which the backend marks with `spaceShownBy`. They are shown as
// children of that row instead of as independent drives.
import type { Drive } from "../api/types";

export interface DriveGroup {
  drive: Drive;
  children: Drive[];
}

/** Keeps the incoming order. A row nests under the row whose `short` name is its
 *  `spaceShownBy`, unless it is "~" (Home stays a top-level shortcut), the owner is
 *  missing, or the owner itself nests (no chains, no cycles: it then stays top level). */
export function groupDrives(drives: Drive[]): DriveGroup[] {
  const owner = (d: Drive): Drive | undefined => {
    if (!d.spaceShownBy || d.short === "~") return undefined;
    return drives.find((o) => o !== d && o.short === d.spaceShownBy && !(o.spaceShownBy && o.short !== "~"));
  };
  const groups: DriveGroup[] = [];
  const byDrive = new Map<Drive, DriveGroup>();
  for (const d of drives) {
    if (!owner(d)) {
      const g = { drive: d, children: [] };
      groups.push(g);
      byDrive.set(d, g);
    }
  }
  for (const d of drives) {
    const o = owner(d);
    if (o) byDrive.get(o)?.children.push(d);
  }
  return groups;
}
