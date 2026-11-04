// File/folder icons from the Catppuccin icon theme (MIT, https://github.com/catppuccin/zed-icons),
// vendored under public/icons/catppuccin/{mocha,latte}. Lookup mirrors Zed's icon themes:
// exact file name, then dotted suffixes from longest to shortest; folders by name.
import map from "./catppuccin-icons.json";

const M = map as unknown as {
  suffixes: Record<string, string>;
  stems: Record<string, string>;
  dirs: Record<string, [string, string]>;
  dir: [string, string];
};

/** Our own icons (public/icons/gall) that replace Catppuccin ids, so headers read as "h". */
const OWN: Record<string, string> = { "c-header": "@h", "cpp-header": "@hpp" };

const cache = new Map<string, string>();
// Own keys only: a file named "constructor" must not hit Object.prototype.
const get = <T,>(rec: Record<string, T>, k: string): T | undefined => (Object.hasOwn(rec, k) ? rec[k] : undefined);

export function fileIconId(name: string): string {
  let hit = cache.get(name);
  if (hit) return hit;
  hit = get(M.stems, name) ?? get(M.suffixes, name) ?? get(M.suffixes, name.toLowerCase());
  if (!hit) {
    // "foo.test.ts" → try "test.ts", then "ts"
    for (let i = name.indexOf("."); i >= 0 && !hit; i = name.indexOf(".", i + 1)) {
      const suf = name.slice(i + 1);
      if (suf) hit = get(M.suffixes, suf) ?? get(M.suffixes, suf.toLowerCase());
    }
  }
  hit ??= "_file";
  hit = get(OWN, hit) ?? hit;
  if (cache.size > 5000) cache.clear();
  cache.set(name, hit);
  return hit;
}

export function folderIconId(name: string, open = false): string {
  const d = get(M.dirs, name) ?? get(M.dirs, name.toLowerCase()) ?? M.dir;
  return d[open ? 1 : 0];
}

/** The plain folder (no emblem). */
export const isPlainFolder = (id: string) => id === M.dir[0] || id === M.dir[1];

/** Catppuccin's neutral outline colour per flavour: the folder shape in folder icons. */
const NEUTRAL = ["#cdd6f4", "#4c4f69"];

/**
 * The emblem of a Catppuccin folder icon (src, .git, node_modules…) as SVG markup in the
 * icon's 16×16 space, without the folder outline, so it can sit on a filled folder.
 * The outline is the first neutral-coloured path; in a few icons it also carries
 * emblem strokes, so only its subpaths that start on the folder's left side
 * (x < 7) go.
 */
export function stripFolderOutline(svg: string): string {
  const doc = new DOMParser().parseFromString(svg, "image/svg+xml");
  const root = doc.documentElement;
  const outline = [...root.querySelectorAll("path")].find((p) => NEUTRAL.includes((p.getAttribute("stroke") ?? "").toLowerCase()));
  if (outline) {
    const rest = (outline.getAttribute("d") ?? "")
      .split(/(?=M)/)
      .filter((sp) => {
        const m = sp.match(/^[Mm]\s*(-?[\d.]+)/);
        return m ? Number(m[1]) >= 7 : false;
      })
      .join("");
    if (rest.trim()) outline.setAttribute("d", rest);
    else outline.remove();
  }
  return root.innerHTML.trim();
}

const emblems = new Map<string, Promise<string>>();

/** [`stripFolderOutline`] of the icon at `url`, fetched once. */
export function folderEmblem(url: string): Promise<string> {
  let p = emblems.get(url);
  if (!p) {
    p = fetch(url).then((r) => (r.ok ? r.text() : "")).then((t) => (t ? stripFolderOutline(t) : "")).catch(() => "");
    emblems.set(url, p);
  }
  return p;
}

export function iconUrl(id: string, light: boolean): string {
  const flavour = light ? "latte" : "mocha";
  return id.startsWith("@") ? `/icons/gall/${flavour}/${id.slice(1)}.svg` : `/icons/catppuccin/${flavour}/${id}.svg`;
}
