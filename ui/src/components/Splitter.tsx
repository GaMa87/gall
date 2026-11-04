// Draggable divider with optional collapse arrows (W-06, W-07).
import { useRef, useState } from "react";

interface Props {
  dir: "v" | "h";
  /** Called with the pointer delta in px since drag start and the initial size. */
  onDrag: (delta: number) => void;
  onDragStart?: () => void;
  onDragEnd?: () => void;
  arrows?: { a?: () => void; b?: () => void };
  className?: string;
  /** Double-click on the divider (e.g. back to the middle). */
  onReset?: () => void;
}

export function Splitter({ dir, onDrag, onDragStart, onDragEnd, arrows, className, onReset }: Props) {
  const [drag, setDrag] = useState(false);
  const start = useRef(0);
  // The window listeners live for the whole drag, but the parent's callbacks close over
  // state it sets in onDragStart (its start size): always call the latest ones, or the
  // first move would use the size from before (the divider jumped to the old spot).
  const cb = useRef({ onDrag, onDragEnd });
  cb.current = { onDrag, onDragEnd };
  return (
    <div
      className={`splitter ${dir} ${drag ? "drag" : ""} ${className ?? ""}`}
      title={onReset ? "Drag to resize, double-click to centre" : undefined}
      onDoubleClick={(e) => {
        if (onReset && (e.target as HTMLElement).tagName !== "BUTTON") onReset();
      }}
      onMouseDown={(e) => {
        if ((e.target as HTMLElement).tagName === "BUTTON") return;
        e.preventDefault();
        start.current = dir === "v" ? e.clientX : e.clientY;
        setDrag(true);
        onDragStart?.();
        const move = (ev: MouseEvent) => cb.current.onDrag((dir === "v" ? ev.clientX : ev.clientY) - start.current);
        const up = () => {
          setDrag(false);
          cb.current.onDragEnd?.();
          window.removeEventListener("mousemove", move);
          window.removeEventListener("mouseup", up);
        };
        window.addEventListener("mousemove", move);
        window.addEventListener("mouseup", up);
      }}
    >
      {arrows && (
        <div className="grip">
          {arrows.a && <button onClick={arrows.a} title="Move divider to the end">{dir === "v" ? "‹" : "˄"}</button>}
          {arrows.b && <button onClick={arrows.b} title="Move divider to the end">{dir === "v" ? "›" : "˅"}</button>}
        </div>
      )}
    </div>
  );
}

/** Persisted pane sizes (per UI element) in localStorage. */
export function useStoredSize(key: string, initial: number): [number, (v: number) => void] {
  const [v, setV] = useState<number>(() => {
    const s = localStorage.getItem("size:" + key);
    return s ? Number(s) : initial;
  });
  return [
    v,
    (n: number) => {
      setV(n);
      localStorage.setItem("size:" + key, String(Math.round(n)));
    },
  ];
}
