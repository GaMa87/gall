// Themes shipped with Gall. Each is a plain CSS file in this folder (the same format as user
// themes in <settings>/themes, see docs/themes/README.md); a user file with the same name wins.
import gruvbox from "./gruvbox.css?raw";
import nightowl from "./nightowl.css?raw";

export const BUILTIN_THEMES: Record<string, { label: string; css: string }> = {
  gruvbox: { label: "Gruvbox", css: gruvbox },
  nightowl: { label: "Night Owl", css: nightowl },
};
