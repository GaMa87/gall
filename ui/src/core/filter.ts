// Mirror of crates/gall-core/src/view/filter.rs (F-04).
import { foldText, wantsFold } from "./fuzzy";

export function quickFilterMatches(name: string, query: string): boolean {
  if (!query) return true;
  // macOS may hand out decomposed (NFD) names; match them composed, like the Rust side.
  name = name.normalize("NFC");
  query = query.normalize("NFC");
  // Typed without accents, "porocilo" also finds "Poročilo".
  const n = (wantsFold(query) ? foldText(name) : name).toLowerCase();
  const q = query.toLowerCase();
  if (q.startsWith(".")) {
    const ext = q.slice(1);
    if (!ext) return n.includes(".");
    const i = n.lastIndexOf(".");
    return i > 0 && n.slice(i + 1).startsWith(ext);
  }
  return [...q].length === 1 ? n.startsWith(q) : n.includes(q);
}
