// "Open in the left / right browser" (top / bottom in the columns layout): two small
// window glyphs with the target half filled. Used wherever a folder can be opened in
// either browser. Inside dialogs (links to Gall's own folders) they are always visible:
// the browsers are out of reach there. In the sidebar they show on hover, since the
// browsers are right next to it. Nothing is shown in the classic layout (one browser).
import { useStore } from "../state/store";

/** A window split in two with half `b` filled. */
export function PaneGlyph({ b, vertical }: { b: number; vertical: boolean }) {
  const half = vertical ? { x: 1.5, y: b ? 6 : 1.5, width: 13, height: 4.5 } : { x: b ? 8 : 1.5, y: 1.5, width: 6.5, height: 9 };
  return (
    <svg width="16" height="12" viewBox="0 0 16 12" aria-hidden>
      <rect x="1" y="1" width="14" height="10" rx="1.5" fill="none" stroke="currentColor" strokeWidth="1.2" />
      <path d={vertical ? "M1 6h14" : "M8 1v10"} stroke="currentColor" strokeWidth="1" />
      <rect {...half} rx="0.8" fill="currentColor" opacity="0.75" />
    </svg>
  );
}

/** The browser's name as the layout places it: "left" / "right" or "top" / "bottom". */
export function useBrowserSide(): { dual: boolean; vertical: boolean; side: (b: number) => string } {
  const layout = useStore((s) => s.session?.layout);
  const vertical = layout === "columns";
  return {
    dual: !!layout && layout !== "classic",
    vertical,
    side: (b) => (vertical ? (b ? "bottom" : "top") : b ? "right" : "left"),
  };
}

export function PanePick({ onPick, className = "" }: { onPick: (b: number) => void; className?: string }) {
  const { dual, vertical, side } = useBrowserSide();
  if (!dual) return null;
  return (
    <span className={`pane-pick ${className}`}>
      {[0, 1].map((b) => (
        <button key={b} title={`Open in the ${side(b)} browser`} onClick={(e) => { e.stopPropagation(); onPick(b); }}>
          <PaneGlyph b={b} vertical={vertical} />
        </button>
      ))}
    </span>
  );
}
