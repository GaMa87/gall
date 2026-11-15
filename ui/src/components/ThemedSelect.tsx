import { useId, useLayoutEffect, useRef, useState, type SelectHTMLAttributes } from "react";
import { createPortal } from "react-dom";
import "../styles/themedSelect.css";

type Item = { value: string; label: string; disabled: boolean; group: string | null };
type Box = { left: number; top: number; width: number; maxHeight: number; upwards: boolean };

/** Keep the native form control and change contract, but paint its popup with Gall's theme.
 * WebKitGTK's native popup uses the desktop palette even when option CSS says otherwise.
 */
export function ThemedSelect({ children, onMouseDown, onClick, onKeyDown, onBlur, ...props }: SelectHTMLAttributes<HTMLSelectElement>) {
  const control = useRef<HTMLSelectElement>(null), menu = useRef<HTMLDivElement>(null);
  const id = useId();
  const [box, setBox] = useState<Box | null>(null), [items, setItems] = useState<Item[]>([]), [active, setActive] = useState(-1);
  const search = useRef({ text: "", time: 0 });
  const read = () => Array.from(control.current?.options ?? []).filter((o) => !o.hidden).map((o): Item => {
    const group = o.parentElement instanceof HTMLOptGroupElement ? o.parentElement : null;
    return { value: o.value, label: o.label, disabled: o.disabled || !!group?.disabled, group: group?.label ?? null };
  });
  const close = () => { setBox(null); search.current = { text: "", time: 0 }; };
  const show = () => {
    const select = control.current;
    if (!select || select.disabled) return;
    select.focus();
    const next = read(), rect = select.getBoundingClientRect(), gap = 5, margin = 8;
    const below = window.innerHeight - rect.bottom - margin - gap, above = rect.top - margin - gap;
    const upwards = below < 180 && above > below;
    const maxHeight = Math.max(0, Math.min(320, upwards ? above : below));
    const width = Math.min(Math.max(rect.width, 160), window.innerWidth - margin * 2);
    setItems(next);
    const selected = next.findIndex((o) => o.value === select.value && !o.disabled);
    setActive(selected >= 0 ? selected : next.findIndex((o) => !o.disabled));
    setBox({ left: Math.max(margin, Math.min(rect.left, window.innerWidth - width - margin)), top: upwards ? rect.top - gap : rect.bottom + gap, width, maxHeight, upwards });
  };
  const choose = (index: number) => {
    const item = items[index], select = control.current;
    if (!item || item.disabled || !select) return;
    if (select.value !== item.value) {
      select.value = item.value;
      select.dispatchEvent(new Event("change", { bubbles: true }));
    }
    close();
  };
  useLayoutEffect(() => {
    if (!box) return;
    const next = read();
    const value = items[active]?.value ?? control.current?.value;
    setItems(next);
    const current = next.findIndex((item) => item.value === value && !item.disabled);
    setActive(current >= 0 ? current : next.findIndex((item) => !item.disabled));
  }, [children, !!box]);
  useLayoutEffect(() => {
    if (!box) return;
    const away = (event: MouseEvent) => {
      if (!control.current?.contains(event.target as Node) && !menu.current?.contains(event.target as Node)) close();
    };
    const scroll = (event: Event) => { if (!menu.current?.contains(event.target as Node)) close(); };
    document.addEventListener("mousedown", away, true);
    document.addEventListener("scroll", scroll, true);
    window.addEventListener("resize", close);
    window.addEventListener("blur", close);
    return () => {
      document.removeEventListener("mousedown", away, true);
      document.removeEventListener("scroll", scroll, true);
      window.removeEventListener("resize", close);
      window.removeEventListener("blur", close);
    };
  }, [!!box]);
  useLayoutEffect(() => {
    if (box && active >= 0) menu.current?.querySelector<HTMLElement>(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active, items, !!box]);
  useLayoutEffect(() => { if (props.disabled) close(); }, [props.disabled]);
  const key = (event: React.KeyboardEvent<HTMLSelectElement>) => {
    onKeyDown?.(event);
    if (event.defaultPrevented) return;
    if (event.key === "F4" && !event.altKey && !event.ctrlKey && !event.metaKey) {
      event.preventDefault(); event.stopPropagation(); if (box) close(); else show(); return;
    }
    if (event.key === "Tab") { close(); return; }
    if (event.key === "Escape") { if (box) { event.preventDefault(); event.stopPropagation(); close(); } return; }
    if (event.altKey && event.key === "ArrowUp") { event.preventDefault(); event.stopPropagation(); close(); return; }
    const move = event.key === "ArrowDown" || event.key === "ArrowUp" || event.key === "Home" || event.key === "End" || event.key === "PageDown" || event.key === "PageUp";
    const accept = event.key === "Enter" || event.key === " ";
    const typed = event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey;
    if (!move && !accept && !typed) return;
    event.preventDefault(); event.stopPropagation();
    if (!box) {
      show();
      if (!typed || event.key === " ") return;
    }
    const options = box ? items : read();
    if (accept && box) { choose(active); return; }
    if (move && box) {
      const step = event.key === "ArrowUp" || event.key === "End" || event.key === "PageUp" ? -1 : 1;
      const page = event.key === "PageUp" || event.key === "PageDown";
      const count = page ? Math.max(1, Math.floor((menu.current?.clientHeight ?? 200) / (menu.current?.querySelector<HTMLElement>("[role=option]")?.offsetHeight || 30)) - 1) : 1;
      let index = event.key === "Home" ? 0 : event.key === "End" ? options.length - 1 : Math.max(0, Math.min(options.length - 1, active + step * count));
      while (index >= 0 && index < options.length && options[index].disabled) index += step;
      if (index >= 0 && index < options.length) setActive(index);
    } else if (typed) {
      const now = Date.now(), previous = now - search.current.time < 700 ? search.current.text : "";
      const text = previous + event.key.toLocaleLowerCase();
      search.current = { text, time: now };
      const prefix = [...text].every((c) => c === text[0]) ? text[0] : text;
      const start = prefix.length === 1 ? active + 1 : Math.max(0, active);
      for (let offset = 0; offset < options.length; offset++) {
        const index = (start + offset + options.length) % options.length;
        if (!options[index].disabled && options[index].label.toLocaleLowerCase().startsWith(prefix)) { setActive(index); break; }
      }
    }
  };
  return <>
    <select {...props} ref={control} aria-haspopup="listbox" aria-expanded={!!box} aria-controls={box ? id : undefined} aria-activedescendant={box && active >= 0 ? `${id}-${active}` : undefined}
      onMouseDown={(event) => { onMouseDown?.(event); if (event.button === 0 && !event.defaultPrevented) { event.preventDefault(); if (box) close(); else show(); } }}
      onClick={(event) => { onClick?.(event); event.preventDefault(); if (event.detail === 0 && !box) show(); }}
      onKeyDown={key} onBlur={(event) => { onBlur?.(event); close(); }}>{children}</select>
    {box && createPortal(<div ref={menu} id={id} className="themed-select-menu" role="listbox" aria-label={props["aria-label"] ?? control.current?.labels?.[0]?.textContent ?? props.title ?? "Options"}
      style={{ left: box.left, top: box.upwards ? undefined : box.top, bottom: box.upwards ? window.innerHeight - box.top : undefined, width: box.width, maxHeight: box.maxHeight }}
      onMouseDown={(event) => event.preventDefault()}>
      {items.map((item, index) => <div key={index}>
        {item.group !== null && (index === 0 || items[index - 1].group !== item.group) && <div className="themed-select-group-label">{item.group}</div>}
        <div id={`${id}-${index}`} role="option" aria-selected={item.value === control.current?.value} aria-disabled={item.disabled} data-index={index}
          className={`themed-select-option${active === index ? " active" : ""}${item.group !== null ? " grouped" : ""}`}
          onMouseEnter={() => { if (!item.disabled) setActive(index); }} onClick={() => choose(index)}>{item.label}</div>
      </div>)}
    </div>, document.body)}
  </>;
}
