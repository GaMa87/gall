// Executes keyboard / palette / leader-menu actions against the active browser.
import { api } from "../api/ipc";
import type { Action } from "../core/keymap";
import { archiveOf } from "../core/archive";
import { ancestors } from "../core/path";
import { isPatchFile } from "../core/patchImport";
import * as A from "./actions";
import { controller, refreshAll } from "./controllers";
import { getState, setState } from "./store";

/** Runs a keyboard action against the active browser or the app. */
export function run(a: Action) {
  const b = A.activeBrowser();
  const c = controller(b);
  if (c?.handle(a)) return;
  const path = A.activePath(b);
  const session = getState().session!;
  const br = session.browsers[b];
  switch (a) {
    case "parent": {
      A.sessionCmd({ op: "up", b });
      setTimeout(() => controller(b)?.select([path], path), 150);
      break;
    }
    case "back": A.sessionCmd({ op: "back", b }); break;
    case "forward": A.sessionCmd({ op: "forward", b }); break;
    case "editPath": A.openOverlay({ type: "pathEdit", b }); break;
    case "copyFolderPath": A.copyPaths([], path); break;
    case "togglePin": A.togglePin(path); break;
    case "compare": A.compareSelection(b); break;
    case "gitGraph": void A.openGitWorkspace(path); break;
    case "worktrees": void A.openWorktrees(path); break;
    case "compareFolders": A.comparePanelFolders(b); break;
    case "mergeFiles": A.mergeSelection(b); break;
    case "applyPatch": {
      // With a patch file focused the dialog opens on it.
      const f = archiveOf(path) ? null : controller(b)?.focused();
      A.openOverlay({ type: "applyPatch", root: A.activePath(b), file: f && isPatchFile(f) ? f.path : undefined });
      break;
    }
    case "favoritesImport": A.openOverlay({ type: "favoritesImport" }); break;
    case "gitFileHistory":
    case "compareClipboard":
    case "compareGitCommit":
    case "compareGitLastChange":
    case "compareGitHead": {
      const f = controller(b)?.focused();
      if (!f || f.isDir) { A.notify("Focus a file first", "warn"); break; }
      if (a === "compareClipboard") A.compareWithClipboard(f.path);
      else if (a === "gitFileHistory" || a === "compareGitCommit") {
        A.openOverlay({ type: "gitCommits", path: f.path, onlyFile: a === "gitFileHistory" });
      } else A.compareGitFile(f.path, a === "compareGitLastChange" ? "lastChange" : "head");
      break;
    }
    case "longNames": setState((s) => ({ longNames: !s.longNames })); break;
    case "newFile": if (!A.refuseInArchive(path, "create files")) A.openOverlay({ type: "templates", dir: path, b }); break;
    case "newFolder": A.newFolder(b); break;
    case "newNote": if (!A.refuseInArchive(path, "create notes")) A.openOverlay({ type: "note", dir: path }); break;
    case "newTab": A.sessionCmd({ op: "newTab", b }); break;
    case "reopenTab": A.sessionCmd({ op: "reopen", b }); break;
    case "tabFromClipboard": A.tabFromClipboard(b); break;
    case "closeTab": A.sessionCmd({ op: "close", b, idx: br.active }); break;
    case "nextTab": A.sessionCmd({ op: "activate", b, idx: (br.active + 1) % br.tabs.length }); break;
    case "prevTab": A.sessionCmd({ op: "activate", b, idx: (br.active - 1 + br.tabs.length) % br.tabs.length }); break;
    case "tab1": case "tab2": case "tab3": case "tab4": case "tab5":
    case "tab6": case "tab7": case "tab8": case "tab9": {
      const idx = Number(a.slice(3)) - 1;
      if (idx < br.tabs.length) A.sessionCmd({ op: "activate", b, idx });
      break;
    }
    case "workspace1": case "workspace2": case "workspace3": case "workspace4": case "workspace5":
    case "workspace6": case "workspace7": case "workspace8": case "workspace9": {
      A.switchWorkspace(Number(a.slice(9)) - 1);
      break;
    }
    case "workspaceNew": A.createWorkspace(); break;
    case "workspaceNext": A.cycleWorkspace(1); break;
    case "workspacePrev": A.cycleWorkspace(-1); break;
    case "workspaceClose": A.closeWorkspace(getState().activeWorkspace); break;
    case "workspaceRename": void A.promptRenameWorkspace(getState().activeWorkspace); break;
    case "pairToggle": A.toggleActivePair(); break;
    case "pairCycleMode": A.cycleActivePairMode(); break;
    case "pairFlowSend": void A.sendAcrossFlow(); break;
    case "pairFlowFlip": A.flipActiveFlowDirection(); break;
    case "gravityAccept": void A.acceptGravityGhost(); break;
    case "gravityDismiss": void A.dismissGravityGhost(); break;
    case "focusLeft":
    case "focusRight": {
      const t = a === "focusLeft" ? 0 : 1;
      if (t === b || session.layout === "classic" || session.browsers[t].collapsed) break;
      A.focusBrowser(t);
      setTimeout(() => controller(t)?.focus(), 0);
      break;
    }
    case "focusOther": {
      if (session.layout === "classic" || session.browsers[1 - b].collapsed) break;
      A.focusBrowser(1 - b);
      setTimeout(() => controller(1 - b)?.focus(), 0);
      break;
    }
    case "paste": A.paste(b); break;
    case "pasteAsLinks": A.pasteAsLinks(b); break;
    case "refresh": refreshAll(); A.refreshDrives(); A.invalidateAllFolderSizes(); break;
    case "toggleHidden": A.toggleHidden(b); break;
    case "toggleHiddenAll": A.toggleHiddenAll(); break;
    case "toggleColumns": A.toggleColumns(b); break;
    case "toggleSidebar": A.toggleSidebar(); break;
    case "newWindow": api.newWindow({ paths: [path] }).catch(A.fail); break;
    case "togglePreviewPane": A.toggleDetails(b); break;
    case "swapBrowsers": A.swapBrowsers(); break;
    case "notifications": {
      const showNotificationLog = !getState().showNotificationLog;
      setState({ showNotificationLog });
      if (showNotificationLog) {
        const actions = document.querySelectorAll<HTMLButtonElement>(".toasts .toast:not(.task) > button:first-of-type");
        actions[actions.length - 1]?.focus();
      }
      break;
    }
    case "search": A.openOverlay({ type: "search", root: path }); break;
    case "quickOpen": A.openOverlay({ type: "quickOpen" }); break;
    case "commandPalette": A.openOverlay({ type: "quickOpen", initial: ">" }); break;
    case "grepFolder": A.openOverlay({ type: "grep", root: path, scope: "folder" }); break;
    case "grepRoot": A.openOverlay({ type: "grep", root: path, scope: "root" }); break;
    case "keyHelp": A.openOverlay({ type: "keys" }); break;
    case "goHome": A.navigate(b, getState().info!.home); break;
    case "goRoot": A.navigate(b, ancestors(path)[0] ?? "/"); break;
    case "terminal": api.openTerminal(path).catch(A.fail); break;
    case "terminalWith": A.terminalWith(path); break;
    case "openWithSettings": A.openOverlay({ type: "settings", section: "openWith" }); break;
    case "undo": A.undo(); break;
    case "reloadTheme": setState((s) => ({ themeNonce: s.themeNonce + 1 })); A.notify("Theme reloaded"); break;
    case "settings": A.openOverlay({ type: "settings" }); break;
    case "layoutNext": A.nextLayout(); break;
    default: break;
  }
}
