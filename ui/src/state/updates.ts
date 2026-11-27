import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { StartupUpdateNotice, UpdateCheckResult, UpdateProgressEvent, UpdateStage, UpdateStrategy } from "../api/types";
import { getState } from "./store";
import * as A from "./actions";

export type UpdatePhase = "idle" | "checking" | "available" | "downloading" | "ready" | "applying" | "current" | "error";

export type UpdateState = {
  phase: UpdatePhase;
  version?: string;
  notes?: string;
  error?: string;
  reason?: string;
  strategy?: UpdateStrategy;
  installKind?: string;
  stage?: UpdateStage;
  downloaded?: number;
  total?: number | null;
  installDir?: string;
  updatesDir?: string;
  baseExePath?: string;
  baseVersion?: string;
  runningOverlay?: boolean;
};

let state: UpdateState = { phase: "idle" };
let activeOp = false;
const listeners = new Set<() => void>();
function set(next: UpdateState) { state = next; listeners.forEach((listener) => listener()); }
export function useUpdates() { return useSyncExternalStore((listener) => { listeners.add(listener); return () => { listeners.delete(listener); }; }, () => state); }

const UPDATED_KEY = "gall.updatedTo";

function preserveMeta(s: UpdateState) {
  return {
    strategy: s.strategy,
    installKind: s.installKind,
    installDir: s.installDir,
    updatesDir: s.updatesDir,
    baseExePath: s.baseExePath,
    baseVersion: s.baseVersion,
    runningOverlay: s.runningOverlay,
  };
}

function metaFromResult(res: UpdateCheckResult) {
  return {
    reason: res.reason,
    strategy: res.strategy,
    installKind: res.installKind,
    installDir: res.installDir,
    updatesDir: res.updatesDir,
    baseExePath: res.baseExePath,
    baseVersion: res.baseVersion,
    runningOverlay: res.runningOverlay,
  };
}

/** Legacy localStorage fallback for transitions from 0.2.x. */
export function takeUpdatedVersion(): string | null {
  try {
    const version = localStorage.getItem(UPDATED_KEY);
    localStorage.removeItem(UPDATED_KEY);
    return version;
  } catch { return null; }
}

/** Reads and clears the one-shot startup notice written by the updater or bootstrapper. */
export async function takeStartupUpdateNotice(): Promise<StartupUpdateNotice | null> {
  const legacy = takeUpdatedVersion();
  try {
    const notice = await invoke<StartupUpdateNotice | null>("update_startup_notice");
    if (notice) return notice;
  } catch { /* fallback below */ }
  return legacy !== null ? { kind: "updated", version: legacy, fromVersion: null } : null;
}

async function runWithProgress(fn: () => Promise<void>, onProgress: (ev: UpdateProgressEvent) => void) {
  const unlisten = await listen<UpdateProgressEvent>("update-progress", (event) => onProgress(event.payload));
  try { await fn(); } finally { unlisten(); }
}

export async function checkUpdates(manual = true) {
  if (activeOp || state.phase === "checking" || state.phase === "downloading" || state.phase === "applying") return;
  activeOp = true;
  const baseMeta = preserveMeta(state);
  set({ ...baseMeta, phase: "checking" });
  try {
    const res = await invoke<UpdateCheckResult>("check_update");
    const meta = metaFromResult(res);
    if (!res.available || !res.version) {
      set({ ...meta, phase: "current" });
      return;
    }
    const availableBase: UpdateState = { ...meta, phase: "available", version: res.version, notes: res.notes ?? undefined };
    if (res.staged) {
      set({ ...availableBase, phase: "ready" });
      if (!manual) {
        A.notify(`Gall ${res.version} is ready to install.`, "info", { label: "Restart to update", run: () => { void applyUpdate(); } });
      }
      return;
    }
    if (res.reason) {
      set(availableBase);
      if (!manual) {
        A.notify(`Gall ${res.version} is available. Open Settings → Updates for details.`, "info", { label: "Details…", run: () => A.openOverlay({ type: "settings", section: "updates" }) });
      }
      return;
    }

    const autoDownload = getState().settings?.autoDownloadUpdates ?? true;
    if (!autoDownload) {
      set(availableBase);
      if (!manual) {
        A.notify(`Gall ${res.version} is available.`, "info", { label: "Update…", run: () => A.openOverlay({ type: "settings", section: "updates" }) });
      }
      return;
    }

    set({ ...availableBase, phase: "downloading", stage: "downloading", downloaded: 0, total: null });
    try {
      await runWithProgress(
        () => invoke("download_update"),
        (ev) => {
          if (state.phase === "downloading") {
            set({ ...state, stage: ev.stage, downloaded: ev.downloaded, total: ev.total });
          }
        },
      );
      set({ ...availableBase, phase: "ready" });
      if (!manual) {
        A.notify(`Gall ${res.version} is ready to install.`, "info", { label: "Restart to update", run: () => { void applyUpdate(); } });
      }
    } catch (downloadError) {
      set({ ...availableBase, error: String(downloadError) });
      if (!manual) {
        A.notify(`Gall ${res.version} is available.`, "info", { label: "Update…", run: () => A.openOverlay({ type: "settings", section: "updates" }) });
      }
    }
  } catch (error) {
    set({ ...baseMeta, phase: "error", error: String(error) });
  } finally {
    activeOp = false;
  }
}

export async function applyUpdate() {
  if (activeOp || state.phase !== "ready" || state.reason) return;
  const readyState = state;
  activeOp = true;
  set({ ...readyState, phase: "applying", stage: "applying", error: undefined });
  try {
    if (Object.keys(getState().tasks).length) throw new Error("Wait for file operations to finish before installing an update.");
    try { localStorage.setItem(UPDATED_KEY, readyState.version ?? ""); } catch { /* storage unavailable */ }
    await runWithProgress(
      () => invoke("apply_update"),
      (ev) => {
        if (state.phase === "applying") {
          set({ ...state, stage: ev.stage, downloaded: ev.downloaded, total: ev.total });
        }
      },
    );
  } catch (error) {
    try { localStorage.removeItem(UPDATED_KEY); } catch { /* storage unavailable */ }
    const msg = error instanceof Error ? error.message : String(error);
    set({ ...readyState, phase: "ready", error: msg });
    if (getState().overlay?.type !== "settings") {
      A.notify(msg, "error");
    }
  } finally {
    activeOp = false;
  }
}

export async function installUpdate() {
  if (state.phase === "ready") return applyUpdate();
  if (activeOp || !state.version || state.phase !== "available" || state.reason) return;
  const available = state;
  activeOp = true;
  set({ ...available, phase: "downloading", stage: "downloading", downloaded: 0, total: null, error: undefined });
  try {
    if (Object.keys(getState().tasks).length) throw new Error("Wait for file operations to finish before installing an update.");
    try { localStorage.setItem(UPDATED_KEY, available.version ?? ""); } catch { /* storage unavailable */ }
    await runWithProgress(
      () => invoke("install_update"),
      (ev) => {
        const nextPhase: UpdatePhase = ev.stage === "applying" || ev.stage === "restarting" ? "applying" : "downloading";
        set({ ...available, phase: nextPhase, stage: ev.stage, downloaded: ev.downloaded, total: ev.total });
      },
    );
  } catch (error) {
    try { localStorage.removeItem(UPDATED_KEY); } catch { /* storage unavailable */ }
    set({ ...available, phase: "available", error: String(error) });
  } finally {
    activeOp = false;
  }
}

export async function revertOverlayUpdate() {
  if (activeOp) return;
  activeOp = true;
  try {
    if (Object.keys(getState().tasks).length) throw new Error("Wait for file operations to finish before reverting.");
    await invoke("revert_overlay_update");
  } catch (error) {
    set({ ...state, error: String(error) });
  } finally {
    activeOp = false;
  }
}

export function postponeUpdate() {
  if (state.phase !== "available" && state.phase !== "ready") return;
  set({ ...preserveMeta(state), phase: "idle" });
}
