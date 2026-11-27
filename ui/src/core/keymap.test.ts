import { describe, expect, it } from "vitest";
import { buildKeymap, DEFAULT_KEYMAP, KEYBOARD_PROFILES, type KeyboardProfile } from "./keymap";

describe("new folder vs new window", () => {
  const profiles = Object.keys(KEYBOARD_PROFILES) as KeyboardProfile[];

  it("keeps Ctrl+Shift+N as New folder in every profile", () => {
    for (const profile of profiles) {
      const km = buildKeymap({}, profile);
      expect(km.get("Ctrl+Shift+N"), profile).toBe("newFolder");
    }
  });

  it("opens a new window with Ctrl+Alt+N, which no other action uses", () => {
    for (const profile of profiles) {
      const km = buildKeymap({}, profile);
      expect(km.get("Ctrl+Alt+N"), profile).toBe("newWindow");
    }
    const users = Object.entries(DEFAULT_KEYMAP).filter(([, chords]) => chords.includes("Ctrl+Alt+N")).map(([a]) => a);
    expect(users).toEqual(["newWindow"]);
  });

  it("lets the user rebind the new-window chord", () => {
    const km = buildKeymap({ newWindow: "Ctrl+Shift+W" }, "explorer");
    expect(km.get("Ctrl+Shift+W")).toBe("newWindow");
    expect(km.has("Ctrl+Alt+N")).toBe(false);
  });
});
