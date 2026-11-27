// Pure presentation helpers for the cross-platform "Default file manager" step
// (Welcome setup and Settings › Open with).
import type { DefaultFileManagerStatus, DefaultFileManagerUnavailableReason } from "../api/types";

export type TranslateFn = (key: string, params?: Record<string, string | number>) => string;

/** Fallback system file manager label when the backend does not report a specific name. */
export function fallbackSystemManager(platform: string, tt: TranslateFn): string {
  if (platform === "macos") return "Finder";
  if (platform === "windows") return "File Explorer";
  return tt("System default");
}

/** Title of the "keep / restore non-Gall" card in the Welcome setup. */
export function keepOrRestoreTitle(status: DefaultFileManagerStatus | null, platform: string, tt: TranslateFn): string {
  if (status?.isDefault) {
    return restoreActionLabel(status, tt);
  }
  const current = status?.currentHandler?.trim();
  if (current) {
    return tt("Keep {name}", { name: current });
  }
  if (platform === "macos") return tt("Keep {name}", { name: "Finder" });
  if (platform === "windows") return tt("Keep {name}", { name: "File Explorer" });
  return tt("Keep current default");
}

/** Label of the Restore button in Settings › Open with and Welcome setup. */
export function restoreActionLabel(status: DefaultFileManagerStatus, tt: TranslateFn): string {
  const target = status.restoreTarget?.trim();
  if (target) return tt("Restore {name}", { name: target });
  return tt("Restore previous default");
}

/** Platform-specific explanation of what changing the default file manager modifies. */
export function platformScopeNote(platform: string, tt: TranslateFn): string {
  if (platform === "macos") {
    return tt("Sets macOS NSFileViewer so apps that reveal or open folders use Gall. Finder still handles the Desktop; running apps may need a restart.");
  }
  if (platform === "windows") {
    return tt("Registers Gall for opening folders and drives for your user account (no administrator rights needed). System views such as Control Panel stay in File Explorer.");
  }
  return tt("Updates the Freedesktop inode/directory default (mimeapps.list) for your user account.");
}

/** Localized explanation when setting Gall as the default file manager is unavailable. */
export function unavailableExplanation(reason: DefaultFileManagerUnavailableReason | null, tt: TranslateFn): string | null {
  switch (reason) {
    case "macosAppBundleRequired":
      return tt("Install Gall.app in Applications to set it as the system default file manager.");
    case "executableNotFound":
      return tt("Gall could not locate its executable path in this environment.");
    case "unsupportedPlatform":
      return tt("Setting the default file manager is not supported on this operating system.");
    default:
      return null;
  }
}
