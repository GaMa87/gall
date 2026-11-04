// Markdown → safe HTML for the preview: markdown-it (CommonMark + GFM tables and
// strikethrough, the parser behind VS Code's Markdown preview), sanitised with DOMPurify.
// Both load on first use, like pdf.js.
import { join, parent } from "./path";

type MarkdownIt = InstanceType<typeof import("markdown-it").default>;
let mdP: Promise<MarkdownIt> | null = null;
function markdownIt(): Promise<MarkdownIt> {
  return (mdP ??= import("markdown-it").then(({ default: M }) => new M({ html: true, linkify: true })));
}

/** GitHub-style heading anchor: lower case, punctuation dropped, spaces → "-". */
export function slugify(text: string): string {
  return text.trim().toLowerCase().replace(/[^\p{L}\p{N}\s_-]/gu, "").replace(/\s/g, "-");
}

/** Files that run code when opened with the system default: a link to one in a Markdown
 *  file (a downloaded README) asks before it is opened. */
const RUNNABLE = new Set([
  "exe", "com", "bat", "cmd", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "msi", "msp", "scr", "pif",
  "cpl", "hta", "jar", "lnk", "reg", "sh", "bash", "zsh", "fish", "csh", "run", "bin", "desktop", "appimage", "command",
  "app", "py", "pyw", "pl", "rb", "php", "action", "workflow",
]);
export function isRunnable(path: string): boolean {
  const name = path.split(/[\\/]/).pop() ?? "";
  const dot = name.lastIndexOf(".");
  return dot > 0 && RUNNABLE.has(name.slice(dot + 1).toLowerCase());
}

/** Resolves a relative link or image (`./img/a.png`, `../b.md`) against the file's folder; null for URLs and anchors. */
export function resolveRel(baseDir: string, href: string): string | null {
  if (!href || href.startsWith("#") || /^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith("//")) return null;
  let rel = href.split("#")[0].split("?")[0];
  try { rel = decodeURIComponent(rel); } catch { /* keep as written */ }
  if (!rel || rel.startsWith("/") || rel.startsWith("\\")) return null; // repo-root links: no repo here
  let dir = baseDir;
  for (const seg of rel.split(/[\\/]/)) {
    if (!seg || seg === ".") continue;
    dir = seg === ".." ? parent(dir) ?? dir : join(dir, seg);
  }
  return dir;
}

/**
 * Renders `text` (the content of the file at `path`) to a sanitised DOM fragment: heading
 * anchors, GitHub task-list checkboxes, relative images pointed at the local files via
 * `fileUrl`. The fragment is inserted as is (never re-serialised to an HTML string).
 */
export async function renderMarkdown(text: string, path: string, fileUrl: (p: string) => string): Promise<DocumentFragment> {
  const [md, { default: DOMPurify }] = await Promise.all([markdownIt(), import("dompurify")]);
  const frag = DOMPurify.sanitize(md.render(text), { RETURN_DOM_FRAGMENT: true, FORBID_TAGS: ["style", "form", "input", "button", "textarea", "select"] });
  const baseDir = parent(path) ?? path;

  const seen = new Map<string, number>();
  frag.querySelectorAll<HTMLElement>("h1, h2, h3, h4, h5, h6").forEach((h) => {
    const base = slugify(h.textContent ?? "") || "section";
    const n = seen.get(base) ?? 0;
    seen.set(base, n + 1);
    h.id = n ? `${base}-${n}` : base;
  });

  frag.querySelectorAll<HTMLImageElement>("img[src]").forEach((img) => {
    const local = resolveRel(baseDir, img.getAttribute("src")!);
    if (local) img.setAttribute("src", fileUrl(local));
    img.setAttribute("loading", "lazy");
  });

  // "- [ ] todo" / "- [x] done" → a disabled checkbox, as on GitHub.
  frag.querySelectorAll<HTMLLIElement>("li").forEach((li) => {
    const first = li.firstChild?.nodeName === "P" ? li.firstChild.firstChild : li.firstChild;
    const m = first?.nodeType === Node.TEXT_NODE ? /^\[([ xX])\]\s/.exec(first.textContent ?? "") : null;
    if (!m || !first) return;
    first.textContent = (first.textContent ?? "").slice(m[0].length);
    const box = document.createElement("input");
    box.type = "checkbox";
    box.disabled = true;
    box.checked = m[1] !== " ";
    first.parentNode!.insertBefore(box, first);
    li.classList.add("task");
    li.parentElement?.classList.add("tasks");
  });

  return frag;
}
