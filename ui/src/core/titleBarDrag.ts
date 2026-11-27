type MousePoint = { x: number; y: number; button?: number; buttons?: number };

/** Delays a native title-bar move request until a click becomes an actual drag. */
export function createTitleBarGestures(windowActions: { startDragging: () => Promise<unknown>; toggleMaximize: () => Promise<unknown> }) {
  const dragThreshold = 6;
  let origin: { x: number; y: number } | null = null;
  return {
    down(point: MousePoint, interactive: boolean) {
      if (point.button !== 0 || interactive) return;
      origin = { x: point.x, y: point.y };
    },
    move(point: MousePoint) {
      if (!origin) return;
      if (!((point.buttons ?? 0) & 1)) {
        origin = null;
        return;
      }
      if (Math.hypot(point.x - origin.x, point.y - origin.y) < dragThreshold) return;
      origin = null;
      void windowActions.startDragging().catch(() => {});
    },
    up() { origin = null; },
    doubleClick(button: number, interactive: boolean) {
      if (button !== 0 || interactive) return false;
      void windowActions.toggleMaximize().catch(() => {});
      return true;
    },
  };
}

/** Binds the gesture state machine to the title bar and preserves clickable controls. */
export function bindTitleBarGestures(
  element: HTMLElement,
  windowActions: { startDragging: () => Promise<unknown>; toggleMaximize: () => Promise<unknown> },
): () => void {
  const gestures = createTitleBarGestures(windowActions);
  const isInteractive = (target: EventTarget | null) => target instanceof Element && !!target.closest(
    'a, button, input, select, textarea, label, summary, [contenteditable]:not([contenteditable="false"]), [tabindex]:not([tabindex="-1"]), [role="button"], [role="link"], [role="menuitem"], [role="tab"], [role="checkbox"], [role="radio"], [role="switch"], [role="option"]',
  );

  const down = (event: MouseEvent) => {
    gestures.down({ x: event.clientX, y: event.clientY, button: event.button }, isInteractive(event.target));
  };
  const move = (event: MouseEvent) => {
    gestures.move({ x: event.clientX, y: event.clientY, buttons: event.buttons });
  };
  const up = () => { gestures.up(); };
  const doubleClick = (event: MouseEvent) => {
    if (!gestures.doubleClick(event.button, isInteractive(event.target))) return;
    event.preventDefault();
  };

  element.addEventListener("mousedown", down);
  element.addEventListener("dblclick", doubleClick);
  document.addEventListener("mousemove", move, true);
  document.addEventListener("mouseup", up, true);
  return () => {
    element.removeEventListener("mousedown", down);
    element.removeEventListener("dblclick", doubleClick);
    document.removeEventListener("mousemove", move, true);
    document.removeEventListener("mouseup", up, true);
  };
}
