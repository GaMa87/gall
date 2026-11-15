// Structural marks for a Git patch: its changed lines are compared by their tokens, so
// the views can mark only what really changed and dim lines that were merely reformatted.
import { useEffect, useState } from "react";
import { api } from "../api/ipc";
import { segments } from "../core/diffRows";

/** Per patch line: null (no change, or changed as a whole), [] (formatting only) or the changed ranges. */
export type PatchMarks = ([number, number][] | null)[];

/**
 * Marks of `patch` (the text of a unified diff of the file `name`), or null until they
 * arrive. The patch is readable without them, so a failure leaves it plain.
 */
export function usePatchMarks(name: string | undefined, patch: string | null | undefined): PatchMarks | null {
  const [marks, setMarks] = useState<{ name: string; patch: string; marks: PatchMarks } | null>(null);
  useEffect(() => {
    if (!patch || name == null) return;
    let active = true;
    api.diffPatchMarks(name, patch)
      .then((value) => { if (active) setMarks({ name, patch, marks: value }); })
      .catch(() => { if (active) setMarks(null); });
    return () => { active = false; };
  }, [name, patch]);
  // Marks of an earlier patch never decorate a newer one.
  return marks && marks.name === name && marks.patch === patch ? marks.marks : null;
}

/** True when only the formatting of this patch line differs. */
export const formattingOnly = (marks: PatchMarks | null, index: number) => marks?.[index]?.length === 0;

/** One patch line with its changed tokens marked. */
export function PatchText({ text, spans }: { text: string; spans?: [number, number][] | null }) {
  if (!spans?.length) return <>{text}</>;
  return <>{segments(text, spans).map(([part, changed], i) => (changed ? <mark key={i} className="patch-token">{part}</mark> : <span key={i}>{part}</span>))}</>;
}
