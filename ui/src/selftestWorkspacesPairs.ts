// Real-application graphical self-test for Workspaces + Drop Portal (WN-179),
// Live Tab Pairs (WN-180, Variant A), and Pair Gravity ghost tabs (WN-181).
import type { gitSelftest } from "./selftest";

export async function runWorkspacesPairsSelfTest(t: typeof gitSelftest) {
  const { $, $$, click, focusPane, until, sleep, shot, setWindowSize, log, updateSettings, finish } = t;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const { api } = await import("./api/ipc");
  const { getState } = await import("./state/store");
  const { controller } = await import("./state/controllers");
  const A = await import("./state/actions");
  const { startDrag } = await import("./state/dnd");

  const errors: string[] = [];
  window.addEventListener("error", (e) => errors.push(e.message));
  window.addEventListener("unhandledrejection", (e) => errors.push(String(e.reason)));

  try {
    await getCurrentWindow().unmaximize();
    await setWindowSize(1280, 800);
    await updateSettings({ keyboardProfile: "explorer", shortcuts: {}, language: "en", theme: "dark", showNavColumns: false });

    const baseRoot = A.activePath(0);
    // Prepare clean fixture directories inside the disposable selftest root.
    const dirA = await api.ensureDir(`${baseRoot}/ws-repo-a/src/ui`);
    const dirB = await api.ensureDir(`${baseRoot}/ws-repo-b/src/ui`);
    const dirDrop = await api.ensureDir(`${baseRoot}/ws-drop-dest`);
    await api.ensureDir(`${baseRoot}/ws-repo-a/src/only-in-a`);
    const sampleFile = await api.createFile(dirA, "bridge-note.txt");
    log(!!dirA && !!dirB && !!dirDrop && !!sampleFile, "fixture directories created");

    // 1. Workspaces (WN-179): create, rename, tint, switch, verify zero state leak.
    await A.sessionCmd({ op: "navigate", b: 0, path: `${baseRoot}/ws-repo-a` });
    await A.sessionCmd({ op: "navigate", b: 1, path: `${baseRoot}/ws-repo-b` });
    await A.workspaceCmd({ op: "rename", idx: 0, name: "Dev" });
    await A.workspaceCmd({ op: "accent", idx: 0, accent: "#3d8bfd" });
    await Promise.all([
      A.workspaceCmd({ op: "create", name: "Queued one", cloneCurrent: false }),
      A.workspaceCmd({ op: "create", name: "Queued two", cloneCurrent: false }),
    ]);
    log(getState().workspaces.length === 3, "consecutive queued workspace creations both succeed");
    await A.workspaceCmd({ op: "close", idx: 2 });
    await A.workspaceCmd({ op: "close", idx: 1 });
    await A.workspaceCmd({ op: "create", name: "Inbox", accent: "#3fb27f", cloneCurrent: false });
    await A.sessionCmd({ op: "navigate", b: 0, path: dirDrop });
    await A.sessionCmd({ op: "navigate", b: 1, path: baseRoot });
    await A.workspaceCmd({ op: "switch", idx: 0 });
    log(
      !!(await until(() => getState().workspaces.length === 2 && getState().activeWorkspace === 0)),
      "workspaces created, renamed, tinted, and switched cleanly",
      `count=${getState().workspaces.length} active=${getState().activeWorkspace}`,
    );
    const firstWorkspacePath = A.activePath(0);
    const secondWorkspacePath = getState().workspaces[1]?.leftPath;
    const racePath = await api.ensureDir(`${baseRoot}/ws-race-target`);
    const switchRequest = A.workspaceCmd({ op: "switch", idx: 1 });
    const navigationRequest = A.sessionCmd({ op: "navigate", b: 0, path: racePath });
    await Promise.all([switchRequest, navigationRequest]);
    const switchedWithOldWorkspaceNavigation = await until(
      () => getState().activeWorkspace === 1 && A.activePath(0) === secondWorkspacePath,
    );
    await A.workspaceCmd({ op: "switch", idx: 0 });
    const navigationStayedWithOrigin = await until(() => A.activePath(0) === racePath);
    await A.sessionCmd({ op: "navigate", b: 0, path: firstWorkspacePath });
    await A.workspaceCmd({ op: "switch", idx: 1 });
    const secondWorkspaceStayedUnchanged = await until(() => A.activePath(0) === secondWorkspacePath);
    await A.workspaceCmd({ op: "switch", idx: 0 });
    log(
      !!switchedWithOldWorkspaceNavigation && !!navigationStayedWithOrigin && !!secondWorkspaceStayedUnchanged,
      "queued workspace switch and navigation keep the navigation in its originating workspace",
      `origin=${racePath} other=${secondWorkspacePath}`,
    );
    log(
      !!(await until(() => $$(".ws-pill").length === 2 && $(".frame.ws-tinted") !== null)),
      "workspace switcher pills and active accent hairline rendered",
    );
    const leftRect = $(".tb-left")?.getBoundingClientRect();
    const pmRect = $(".panel-map")?.getBoundingClientRect();
    const layRect = $(".tb-layouts")?.getBoundingClientRect();
    log(
      !!leftRect && !!pmRect && !!layRect && leftRect.right <= pmRect.left + 1 && pmRect.right <= layRect.left + 1,
      "titlebar left strip, PanelMap bridge, and LayoutSwitch do not overlap",
      leftRect && pmRect && layRect ? `left=${Math.round(leftRect.right)} pm=${Math.round(pmRect.left)}..${Math.round(pmRect.right)} lay=${Math.round(layRect.left)}` : "missing",
    );

    // 2. Drop Portal (WN-179): drag file over inactive workspace 2 pill -> portal opens -> drop copies/moves without switching.
    const pill2 = $$<HTMLButtonElement>(".ws-pill")[1];
    log(!!pill2, "inactive workspace 2 pill present");
    if (pill2) {
      const dt = new DataTransfer();
      startDrag({ dataTransfer: dt, isTrusted: false, preventDefault() {} } as unknown as React.DragEvent, [sampleFile]);
      pill2.dispatchEvent(new DragEvent("dragover", { bubbles: true, cancelable: true, dataTransfer: dt }));
      const portalOpened = await until(() => $(".ws-drop-portal") !== null, 1500);
      log(!!portalOpened, "Drop Portal opens on hover over inactive workspace pill");
      await shot("wp-01-drop-portal");
      t.note("SCREENSHOT wp-01-drop-portal.png: expected TitleBar with Workspace 1 (Dev, blue) active, Workspace 2 (Inbox, green) armed, and Drop Portal popover showing Left (ws-drop-dest) and Right drop targets.");

      const leftTarget = $<HTMLElement>('.ws-portal-target[data-portal-side="0"]');
      if (leftTarget) {
        leftTarget.dispatchEvent(new DragEvent("dragover", { bubbles: true, cancelable: true, dataTransfer: dt, ctrlKey: true }));
        leftTarget.dispatchEvent(new DragEvent("drop", { bubbles: true, cancelable: true, dataTransfer: dt, ctrlKey: true }));
      }
      await sleep(300);
      log(getState().activeWorkspace === 0, "Drop Portal drop keeps current workspace active", `active=${getState().activeWorkspace}`);
      const copiedInDrop = await until(async () => {
        const list = await api.listDir(dirDrop, { showHidden: false, hideTemp: false, dirsOnly: false }).catch(() => []);
        return list.some((e) => e.name === "bridge-note.txt");
      }, 3000);
      log(!!copiedInDrop, "Drop Portal copied dragged file into inactive workspace folder");
    }

    // 3. Live Tab Pairs (WN-180, Variant A): Tandem walk + missing folder ghost card + Create folder + Flow bridge.
    await A.sessionCmd({ op: "navigate", b: 0, path: `${baseRoot}/ws-repo-a` });
    await A.sessionCmd({ op: "navigate", b: 1, path: `${baseRoot}/ws-repo-b` });
    A.pairActiveTabs("tandem");
    log(
      !!(await until(() => $(".pm-bridge.paired.mode-tandem") !== null && $$(".tab.paired").length === 2)),
      "Tandem pair badge visible on both TabBars and PanelMap bridge",
    );
    await A.sessionCmd({ op: "navigate", b: 0, path: `${baseRoot}/ws-repo-a/src/ui` });
    log(
      !!(await until(() => A.activePath(1).endsWith("/ws-repo-b/src/ui"))),
      "Tandem walk mirrored left navigation into matching right subfolder",
      `right=${A.activePath(1)}`,
    );
    await shot("wp-02-tandem-live");
    t.note("SCREENSHOT wp-02-tandem-live.png: expected matched paired tabs in both TabBars with Tandem (🔗) badges and PanelMap bridge pill in sync on src/ui.");

    // Walk into a subfolder missing on right -> non-destructive Tandem ghost card!
    await A.sessionCmd({ op: "navigate", b: 0, path: `${baseRoot}/ws-repo-a/src/only-in-a` });
    const ghostCard = await until(() => $(".tandem-ghost") !== null, 2500);
    log(!!ghostCard, "Tandem missing-folder ghost card shown non-destructively without losing pair");
    log((getState().session?.pairs?.length ?? 0) === 1, "pair stays intact on missing folder");
    await shot("wp-03-tandem-missing");
    t.note("SCREENSHOT wp-03-tandem-missing.png: expected right pane displaying non-destructive Tandem ghost card with Create folder, Open parent, and Unlink pair buttons.");

    // Click "Create folder" on the ghost card and confirm the folder is created & loaded.
    const createBtn = $<HTMLButtonElement>(".tandem-ghost .btn.primary");
    createBtn?.click();
    log(
      !!(await until(() => $(".tandem-ghost") === null && A.activePath(1).endsWith("/ws-repo-b/src/only-in-a"), 3000)),
      "Create folder on Tandem ghost card creates mirrored directory and clears ghost state",
    );

    // Switch to Flow mode and confirm Send button appears in PanelMap bridge.
    const pairId = getState().session?.pairs?.[0]?.id;
    if (pairId != null) {
      A.setPairMode(pairId, "flow", "leftToRight", "copy");
      log(
        !!(await until(() => $(".pm-bridge.mode-flow") !== null && $(".pm-flow-send") !== null)),
        "Flow mode displays directional arrow and Send button in PanelMap",
      );
      await shot("wp-04-flow-bridge");
      t.note("SCREENSHOT wp-04-flow-bridge.png: expected Flow (→) badges in TabBars and PanelMap bridge with one-click Send pill and one-click × unlink button.");
      // Every button in the TitleBar and TabBars must carry a non-empty hover description (`title`).
      const missingTitles = $$<HTMLButtonElement>(".titlebar button, .tabbar button").filter((b) => !b.getAttribute("title")?.trim());
      log(
        missingTitles.length === 0,
        "every button in TitleBar and TabBar has a hover tooltip description",
        missingTitles.map((b) => b.className).join(", "),
      );
      const activeSourcePath = A.activePath(0);
      const misroutedFile = await api.createFile(activeSourcePath, "flow-routing-check.txt");
      const inactiveSourcePath = await api.ensureDir(`${baseRoot}/ws-flow-inactive-source`);
      const inactiveDestinationPath = await api.ensureDir(`${baseRoot}/ws-flow-inactive-destination`);
      const beforeInactivePair = getState().session!;
      const activeLeftIdx = beforeInactivePair.browsers[0].active;
      await A.sessionCmd({ op: "newTab", b: 0, path: inactiveSourcePath, activate: false });
      await A.sessionCmd({ op: "newTab", b: 1, path: inactiveDestinationPath, activate: false });
      const afterTabs = getState().session!;
      const inactiveLeftIdx = afterTabs.browsers[0].tabs.findIndex((tab) => tab.path === inactiveSourcePath);
      const inactiveRightIdx = afterTabs.browsers[1].tabs.findIndex((tab) => tab.path === inactiveDestinationPath);
      A.pairTabs(inactiveLeftIdx, inactiveRightIdx, "flow", "leftToRight", "copy");
      const inactivePair = await until(() => getState().session?.pairs?.find((p) =>
        p.mode === "flow"
        && p.leftTabId === getState().session?.browsers[0].tabs[inactiveLeftIdx]?.id
        && p.rightTabId === getState().session?.browsers[1].tabs[inactiveRightIdx]?.id,
      ));
      if (inactivePair) await A.sessionCmd({ op: "activate", b: 0, idx: activeLeftIdx });
      await until(() => A.activePath(0) === activeSourcePath);
      const sourceRow = await until(() => $$(".fp-list [data-path]").find((el) => el.getAttribute("data-path") === misroutedFile));
      if (sourceRow) click(sourceRow);
      await focusPane(0);
      const selectedFromActivePane = !!controller(0)?.selection().some((entry) => entry.path === misroutedFile);
      log(selectedFromActivePane, "Flow routing fixture selected in the active source pane");
      if (inactivePair) await A.sendAcrossFlow(inactivePair.id);
      const inactiveDestinationEntries = await api.listDir(inactiveDestinationPath, {
        showHidden: false,
        hideTemp: false,
        dirsOnly: false,
      });
      log(
        !!inactivePair && !inactiveDestinationEntries.some((entry) => entry.name === "flow-routing-check.txt"),
        "sending from an inactive Flow tab pair cannot copy the active pane's unrelated selection",
      );
      if (inactivePair) A.unpairTabs(inactivePair.id);
      await until(() => (getState().session?.pairs?.length ?? 0) === 1);
      // One-click × button on PanelMap bridge unpairs cleanly back to independent panels.
      const unlinkBtn = $<HTMLButtonElement>(".pm-bridge-unlink");
      log(!!unlinkBtn, "PanelMap bridge shows one-click × unlink button");
      unlinkBtn?.click();
      log(!!(await until(() => (getState().session?.pairs?.length ?? 0) === 0)), "One-click × on PanelMap bridge unpairs tabs immediately");
    }

    // 4. Pair Gravity (WN-181): learned affinity surfaces translucent 🧲 ghost tab on opposite TabBar!
    await A.sessionCmd({ op: "navigate", b: 0, path: dirA });
    await A.sessionCmd({ op: "navigate", b: 1, path: baseRoot });
    await api.gravityRecord(dirA, dirB, 3.5, true);
    A.scheduleGravityRefresh();
    const ghostTab = await until(() => $<HTMLElement>(".tab.tab-ghost"), 3000);
    log(!!ghostTab, "Pair Gravity ghost tab (🧲) appears on opposite TabBar", ghostTab?.textContent ?? "none");
    await shot("wp-05-pair-gravity");
    t.note("SCREENSHOT wp-05-pair-gravity.png: expected translucent dashed ghost tab (🧲 ui) at the end of the right TabBar.");

    // Click ghost tab -> opens partner in opposite TabBar and pairs automatically!
    ghostTab?.click();
    log(
      !!(await until(() => A.activePath(1) === dirB && (getState().session?.pairs?.length ?? 0) === 1, 3000)),
      "Clicking Pair Gravity ghost tab opens and pairs the partner folder",
      `right=${A.activePath(1)} pairs=${getState().session?.pairs?.length}`,
    );

    // 5. Settings opt-out toggles: turning off Workspaces, Live Tab Pairs, and Pair Gravity restores clean classic dual-pane chrome.
    await updateSettings({ showWorkspaces: "never", liveTabPairs: false, pairGravity: false });
    log(
      !!(await until(() => !$(".ws-bar") && !$(".pm-bridge") && !$(".tab.tab-ghost") && (getState().session?.pairs?.length ?? 0) === 0, 2500)),
      "Disabling showWorkspaces, liveTabPairs, and pairGravity cleanly restores classic two-panel UI and unpairs active tabs",
    );
    await updateSettings({ showWorkspaces: "always", liveTabPairs: true, pairGravity: true });

    log(errors.length === 0, "no uncaught errors", errors.join(" | "));
  } finally {
    await finish();
  }
}
