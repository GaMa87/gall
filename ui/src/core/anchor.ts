/** Where a menu, submenu or dropdown goes. One rule for the whole UI:
 *  - opened with the mouse: at the click point;
 *  - opened with a key: at the element the key acts on (the selected row, the focused control), never at a remembered mouse position;
 *  - clamped to the window, and when an element is to be kept visible (`avoid`) the surface goes beside it rather than over it. */
export interface Rect { left: number; top: number; right: number; bottom: number }
/** Which side of `avoid` is tried first. `beside` (a row): right, below, left, above. `below` (a toolbar button): below, above,
 *  right, left. `submenu` (a menu item): right, else left. */
export type Side = "beside" | "below" | "submenu";
export interface Anchor { x: number; y: number; avoid?: Rect; side?: Side }

const GAP = 6;
const EDGE = 4;
/** A submenu overlaps its parent's border a little, so the mouse never leaves both. */
const SUBMENU_OVERLAP = 2;
const SUBMENU_LIFT = 4;

export function rectOf(box: { left: number; top: number; right: number; bottom: number }): Rect {
  return { left: box.left, top: box.top, right: box.right, bottom: box.bottom };
}

/** A key: the menu opens beside `el`, which it must not cover. `el` is the selected row, the focused button, the cursor line. */
export function elementAnchor(el: Element | null | undefined, fallback: Anchor = { x: 40, y: 80 }): Anchor {
  if (!el) return fallback;
  const box = el.getBoundingClientRect();
  if (!box.width && !box.height) return fallback;
  return { x: box.right + GAP, y: box.top, avoid: rectOf(box) };
}

/** A button that drops a menu: the menu opens under it, over it when there is no room below. */
export function belowAnchor(el: Element | null | undefined, fallback?: Anchor): Anchor {
  const anchor = elementAnchor(el, fallback);
  return anchor.avoid ? { x: anchor.avoid.left, y: anchor.avoid.bottom, avoid: anchor.avoid, side: "below" } : anchor;
}

/** A row that is wider than its text: the menu opens right of the text, level with the row, so it hides neither the text nor
 *  ends up at the far edge of the row. `text` is the element inside `row` that holds the text. */
export function textAnchor(row: Element | null | undefined, text: Element | null | undefined, fallback?: Anchor): Anchor {
  const base = elementAnchor(row, fallback);
  if (!row || !text || !base.avoid) return base;
  const range = document.createRange();
  range.selectNodeContents(text);
  const end = Math.min(range.getBoundingClientRect().right, text.getBoundingClientRect().right);
  if (!(end > base.avoid.left)) return base;
  return { x: end + GAP, y: base.avoid.top, avoid: { ...base.avoid, right: end } };
}

/** A click or a right click: the menu opens at the pointer. A click made with the keyboard (Enter or Space on a button) and a
 *  context menu without a position have no pointer to go to, so the menu opens at the element they were made on. */
export function eventAnchor(event: { type?: string; detail?: number; clientX: number; clientY: number; currentTarget?: unknown; target?: unknown }): Anchor {
  const byKey = event.type === "click" ? event.detail === 0 : event.type === "contextmenu" && event.clientX === 0 && event.clientY === 0;
  const isElement = (value: unknown): value is Element => typeof Element !== "undefined" && value instanceof Element;
  const el = isElement(event.currentTarget) ? event.currentTarget : isElement(event.target) ? event.target : null;
  const point = { x: event.clientX, y: event.clientY };
  if (!byKey || !el) return point;
  return event.type === "click" ? belowAnchor(el, point) : elementAnchor(el, point);
}

/** A submenu of the menu item `item`. Uses the parent `.menu` horizontal edge when present so
 *  the submenu overlaps only the border (`SUBMENU_OVERLAP`) instead of the menu's inner padding. */
export function submenuAnchor(item: Element): Anchor {
  const box = item.getBoundingClientRect();
  const menuBox = typeof item.closest === "function" ? item.closest(".menu")?.getBoundingClientRect() : null;
  const left = menuBox ? menuBox.left : box.left;
  const right = menuBox ? menuBox.right : box.right;
  return {
    x: right - SUBMENU_OVERLAP,
    y: box.top - SUBMENU_LIFT,
    avoid: { left, top: box.top, right, bottom: box.bottom },
    side: "submenu",
  };
}

/** Final position of a `width` x `height` surface for `anchor` in a `winW` x `winH` window. Pure, so it is unit-tested. */
export function placeSurface(anchor: Anchor, width: number, height: number, winW: number, winH: number): { x: number; y: number } {
  const clampX = (x: number) => Math.max(EDGE, Math.min(x, winW - width - EDGE));
  const clampY = (y: number) => Math.max(EDGE, Math.min(y, winH - height - EDGE));
  const a = anchor.avoid;
  if (!a) return { x: clampX(anchor.x), y: clampY(anchor.y) };
  const fitsX = (x: number) => x >= EDGE && x + width <= winW - EDGE;
  const fitsY = (y: number) => y >= EDGE && y + height <= winH - EDGE;
  if (anchor.side === "submenu") {
    const y = clampY(anchor.y), left = a.left - width + SUBMENU_OVERLAP;
    return { x: fitsX(anchor.x) ? anchor.x : fitsX(left) ? left : clampX(anchor.x), y };
  }
  const below = anchor.side === "below";
  const right = { x: Math.max(a.right + GAP, anchor.x), y: clampY(a.top) };
  const left = { x: a.left - GAP - width, y: clampY(a.top) };
  const under = { x: clampX(below ? a.left : Math.min(anchor.x, a.left + GAP)), y: a.bottom + GAP };
  const over = { x: clampX(below ? a.left : a.left + GAP), y: a.top - GAP - height };
  const fits = new Map([[right, fitsX(right.x)], [left, fitsX(left.x)], [under, fitsY(under.y)], [over, fitsY(over.y)]]);
  const order = below ? [under, over, right, left] : [right, under, left, over];
  return order.find((place) => fits.get(place)) ?? { x: clampX(anchor.x), y: clampY(anchor.y) };
}

/** The box of a dropdown list that belongs to a control: as wide as the control (at least `minWidth`), below it, above it when
 *  there is clearly more room there, and never out of the window. `bottom` is set instead of `top` when it opens upwards. */
export interface DropdownBox { left: number; top?: number; bottom?: number; width: number; maxHeight: number }
export function dropdownBox(control: Rect, options: { minWidth: number; maxHeight: number; gap?: number; margin?: number }, winW: number, winH: number): DropdownBox {
  const { minWidth, maxHeight, gap = 5, margin = 8 } = options;
  const below = winH - control.bottom - margin - gap, above = control.top - margin - gap;
  const upwards = below < Math.min(180, maxHeight) && above > below;
  const width = Math.min(Math.max(control.right - control.left, minWidth), winW - margin * 2);
  return {
    left: Math.max(margin, Math.min(control.left, winW - width - margin)),
    ...(upwards ? { bottom: winH - control.top + gap } : { top: control.bottom + gap }),
    width,
    maxHeight: Math.max(0, Math.min(maxHeight, upwards ? above : below)),
  };
}
