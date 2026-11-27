import { afterEach, describe, expect, it, vi } from "vitest";
import { keyCap } from "./ShortcutKeys";

describe("key caps", () => {
  it("name Ctrl and Alt as Cmd and Option on macOS only", () => {
    expect(keyCap("Ctrl", true)).toBe("Cmd");
    expect(keyCap("Alt", true)).toBe("Option");
    expect(keyCap("Ctrl", false)).toBe("Ctrl");
    expect(keyCap("Alt", false)).toBe("Alt");
  });

  it("keep the other keys, with arrows and short names", () => {
    for (const mac of [true, false]) {
      expect(keyCap("Shift", mac)).toBe("Shift");
      expect(keyCap("Enter", mac)).toBe("Enter");
      expect(keyCap("ArrowLeft", mac)).toBe("←");
      expect(keyCap("PageDown", mac)).toBe("PgDn");
      expect(keyCap("p", mac)).toBe("p");
    }
  });
});

describe("shortcut names in translated UI texts", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.resetModules();
  });

  it("are written for the platform: Ctrl and Alt off macOS", async () => {
    vi.stubGlobal("navigator", { platform: "Linux x86_64", userAgent: "Linux", languages: ["en"] });
    vi.resetModules();
    const { tt, t } = await import("../i18n");
    expect(tt("Cut (Ctrl+X) and Alt+Enter")).toBe("Cut (Ctrl+X) and Alt+Enter");
    expect(tt("Hold Ctrl: {label}", { label: "Edit with…" })).toBe("Hold Ctrl: Edit with…");
    expect(t("sidebar.settings", { keys: "Ctrl+," })).toContain("Ctrl+,");
  });

  it("are written for the platform: Cmd and Option on macOS, also after {name} is filled in", async () => {
    vi.stubGlobal("navigator", { platform: "MacIntel", userAgent: "Macintosh", languages: ["en"] });
    vi.resetModules();
    const { tt, t } = await import("../i18n");
    expect(tt("Cut (Ctrl+X) and Alt+Enter")).toBe("Cut (Cmd+X) and Option+Enter");
    expect(tt("Hold Ctrl: {label}", { label: "Edit with…" })).toBe("Hold Cmd: Edit with…");
    expect(t("sidebar.settings", { keys: "Ctrl+," })).toContain("Cmd+,");
    expect(t("sidebar.settings", { keys: "Ctrl+," })).not.toContain("Ctrl");
  });
});
