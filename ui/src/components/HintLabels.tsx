import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { HINT_TARGETS, hintLabels, matchHint, onScreen } from "../core/hintLabels";
import { tt } from "../i18n";
import "./HintLabels.css";

type Target = { el: HTMLElement; left: number; top: number; x: number; y: number };

/** The innermost overlay that has the focus of attention: a menu, else a dialog, else the Git window itself. */
const scopeOf = (root: HTMLElement | null): HTMLElement | null => document.querySelector<HTMLElement>(".menu") ?? [...document.querySelectorAll<HTMLElement>(".modal")].at(-1) ?? root;

/** The controls of the scope that are on screen and not covered (a scrolled-away row or one behind a panel has no label). */
function collect(root: HTMLElement | null): Target[] {
  const scope = scopeOf(root);
  if (!scope) return [];
  const view = { left: 0, top: 0, right: window.innerWidth, bottom: window.innerHeight };
  const found: Target[] = [];
  for (const el of scope.querySelectorAll<HTMLElement>(HINT_TARGETS)) {
    if ((el as HTMLButtonElement).disabled || el.getAttribute("aria-hidden") === "true" || el.closest(".hint-layer")) continue;
    const box = el.getBoundingClientRect();
    if (!onScreen(box, view)) continue;
    const x = Math.min(Math.max(box.left + Math.min(box.width / 2, 24), 0), view.right - 1), y = Math.min(Math.max(box.top + box.height / 2, 0), view.bottom - 1);
    // Covered or scrolled out of its container: the element under its middle is neither it nor part of it.
    const hit = document.elementFromPoint(x, y);
    if (!hit || !(el === hit || el.contains(hit) || hit.contains(el))) continue;
    found.push({ el, left: Math.max(box.left, 0), top: Math.max(box.top, 0), x, y });
  }
  return found;
}

/** Clicks a control as a person would; text fields and selects only get the focus. */
function activate(target: Target, right: boolean) {
  const { el } = target;
  if (right) { el.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, button: 2, clientX: target.x, clientY: target.y })); return; }
  el.focus({ preventScroll: true });
  if (el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement || (el instanceof HTMLInputElement && !["checkbox", "radio", "button", "submit"].includes(el.type))) return;
  el.click();
}

/**
 * Hint mode: a one- or two-letter label on every control that is on screen; typing a label clicks it, Shift with the last
 * letter right-clicks it, Backspace takes a letter back and Esc leaves. Mounted only while the mode is on, so the labels
 * are looked for once, again after a scroll or resize, and never during normal use. While it is on, the keys belong to it.
 */
export function HintLabels({ root, onDone }: { root: HTMLElement | null; onDone: () => void }) {
  const [targets, setTargets] = useState<Target[]>(() => collect(root));
  const [typed, setTyped] = useState("");
  const state = useRef({ targets, typed });
  state.current = { targets, typed };
  const labels = hintLabels(targets.length);
  const shown = useRef(labels);
  shown.current = labels;
  useEffect(() => {
    let frame = 0;
    const refresh = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(() => { setTargets(collect(root)); setTyped(""); }); };
    window.addEventListener("scroll", refresh, true);
    window.addEventListener("resize", refresh);
    const onKey = (e: KeyboardEvent) => {
      if (["Shift", "Control", "Alt", "Meta", "CapsLock"].includes(e.key)) return;
      e.preventDefault(); e.stopPropagation();
      if (e.key === "Escape") { onDone(); return; }
      if (e.key === "Backspace") { setTyped((value) => value.slice(0, -1)); return; }
      if (e.ctrlKey || e.metaKey || e.altKey || !/^[a-z]$/i.test(e.key)) return;
      const next = state.current.typed + e.key.toLowerCase(), found = matchHint(next, shown.current);
      if ("exact" in found) {
        const target = state.current.targets[found.exact], right = e.shiftKey;
        onDone();
        // After the labels are gone, so the click lands on the control and focus can move to it.
        window.setTimeout(() => { if (target.el.isConnected) activate(target, right); }, 0);
      } else if ("partial" in found) setTyped(next);
    };
    window.addEventListener("keydown", onKey, true);
    return () => { cancelAnimationFrame(frame); window.removeEventListener("scroll", refresh, true); window.removeEventListener("resize", refresh); window.removeEventListener("keydown", onKey, true); };
  }, [root, onDone]);
  const open = matchHint(typed, labels);
  const visible = "partial" in open ? new Set(open.partial) : new Set<number>();
  return createPortal(<div className="hint-layer" aria-hidden="true">
    {targets.map((target, index) => visible.has(index) && index < labels.length ? <span key={index} className="hint-label" style={{ left: target.left, top: target.top }}><b>{labels[index].slice(0, typed.length)}</b>{labels[index].slice(typed.length)}</span> : null)}
    <div className="hint-bar" role="status" aria-hidden="false">{tt("Type a label to click it · Shift with the last letter right-clicks · Backspace undoes · Esc leaves")}{targets.length === 0 ? ` · ${tt("nothing to click here")}` : ""}</div>
  </div>, document.body);
}
