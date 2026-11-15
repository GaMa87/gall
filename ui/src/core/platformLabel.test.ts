import { describe, expect, it } from "vitest";
import { platformLabel } from "./keymap";

describe("platformLabel", () => {
  it("writes Cmd and Option on macOS and keeps Ctrl and Alt elsewhere", () => {
    expect(platformLabel("Ctrl+P", true)).toBe("Cmd+P");
    expect(platformLabel("Ctrl+Alt+K", true)).toBe("Cmd+Option+K");
    expect(platformLabel("Find (Ctrl+F, Ctrl+Shift+F)", true)).toBe("Find (Cmd+F, Cmd+Shift+F)");
    expect(platformLabel("Ctrl+P", false)).toBe("Ctrl+P");
    expect(platformLabel("Controls", true)).toBe("Controls");
  });
});
