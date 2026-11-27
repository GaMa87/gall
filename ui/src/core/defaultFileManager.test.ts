import { describe, expect, it } from "vitest";
import type { DefaultFileManagerStatus } from "../api/types";
import { fallbackSystemManager, keepOrRestoreTitle, platformScopeNote, restoreActionLabel, unavailableExplanation } from "./defaultFileManager";

const identity = (key: string, params?: Record<string, string | number>) =>
  params ? key.replace(/\{(\w+)\}/g, (_, k: string) => String(params[k] ?? `{${k}}`)) : key;

const status = (patch: Partial<DefaultFileManagerStatus>): DefaultFileManagerStatus => ({
  platform: "linux",
  isDefault: false,
  currentHandler: null,
  currentId: null,
  canSet: true,
  unavailableReason: null,
  canRestore: false,
  restoreTarget: null,
  ...patch,
});

describe("defaultFileManager helpers", () => {
  it("picks platform fallback labels when handler name is absent", () => {
    expect(fallbackSystemManager("macos", identity)).toBe("Finder");
    expect(fallbackSystemManager("windows", identity)).toBe("File Explorer");
    expect(fallbackSystemManager("linux", identity)).toBe("System default");
  });

  it("formats keep and restore card titles cleanly across states and platforms", () => {
    expect(keepOrRestoreTitle(null, "macos", identity)).toBe("Keep Finder");
    expect(keepOrRestoreTitle(null, "windows", identity)).toBe("Keep File Explorer");
    expect(keepOrRestoreTitle(null, "linux", identity)).toBe("Keep current default");
    expect(keepOrRestoreTitle(status({ currentHandler: "Dolphin" }), "linux", identity)).toBe("Keep Dolphin");
    expect(keepOrRestoreTitle(status({ isDefault: true, restoreTarget: "Finder" }), "macos", identity)).toBe("Restore Finder");
    expect(keepOrRestoreTitle(status({ isDefault: true, restoreTarget: null }), "linux", identity)).toBe("Restore previous default");
  });

  it("formats restore button label and platform scope notes", () => {
    expect(restoreActionLabel(status({ isDefault: true, restoreTarget: "File Explorer" }), identity)).toBe("Restore File Explorer");
    expect(restoreActionLabel(status({ isDefault: true, restoreTarget: null }), identity)).toBe("Restore previous default");
    expect(platformScopeNote("linux", identity)).toContain("mimeapps.list");
    expect(platformScopeNote("windows", identity)).toContain("File Explorer");
    expect(platformScopeNote("macos", identity)).toContain("NSFileViewer");
  });

  it("maps unavailable reasons to clear user explanations", () => {
    expect(unavailableExplanation("macosAppBundleRequired", identity)).toContain("Gall.app");
    expect(unavailableExplanation("executableNotFound", identity)).toContain("executable");
    expect(unavailableExplanation("unsupportedPlatform", identity)).toContain("not supported");
    expect(unavailableExplanation(null, identity)).toBeNull();
  });
});
