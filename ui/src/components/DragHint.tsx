// Floating label that says what a drop will do (O-11).
import { useSyncExternalStore } from "react";
import { getHint, subscribeHint } from "../state/dnd";

const ICON = { move: "➜", copy: "+", link: "⤷", menu: "⋯", compare: "⇄", none: "⊘" } as const;

export function DragHintLabel() {
  const h = useSyncExternalStore(subscribeHint, getHint);
  if (!h) return null;
  const x = Math.min(h.x + 16, window.innerWidth - 300);
  const y = Math.min(h.y + 18, window.innerHeight - 30);
  return (
    <div className={`drag-hint ${h.kind}`} style={{ left: x, top: y }}>
      <span>{ICON[h.kind]}</span>
      <span>{h.text}</span>
      {h.tip && <small>{h.tip}</small>}
    </div>
  );
}
