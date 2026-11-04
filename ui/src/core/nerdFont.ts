// Detects whether a Nerd Font is installed, so glyph-based UI can fall back to SVG icons.
// System fonts can't be queried directly: draw a Nerd glyph with the Nerd font first and
// with plain monospace; if no Nerd font exists both fall back to the same font and match.
let cached: boolean | null = null;

export const NERD_FAMILIES = `"JetBrainsMono Nerd Font", "JetBrainsMono NF", "Symbols Nerd Font", "Symbols Nerd Font Mono", "FiraCode Nerd Font", "Hack Nerd Font"`;

export function hasNerdFont(): boolean {
  if (cached !== null) return cached;
  try {
    const c = document.createElement("canvas");
    c.width = c.height = 32;
    const g = c.getContext("2d", { willReadFrequently: true });
    if (!g) return (cached = false);
    const draw = (font: string) => {
      g.clearRect(0, 0, 32, 32);
      g.font = `24px ${font}`;
      g.fillText("\uF07B", 2, 26); // nf-fa-folder
      return g.getImageData(0, 0, 32, 32).data.join(",");
    };
    cached = draw(`${NERD_FAMILIES}, monospace`) !== draw("monospace");
  } catch {
    cached = false;
  }
  return cached;
}
