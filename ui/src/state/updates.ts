import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { listen } from "@tauri-apps/api/event";
import { getState } from "./store";
import * as A from "./actions";

type UpdateState = { phase: "idle" | "checking" | "available" | "current" | "installing" | "error"; version?: string; notes?: string; error?: string; reason?: string; bytes?: number };
let state: UpdateState = { phase: "idle" };
let pending: Update | null = null;
const listeners = new Set<() => void>();
function set(next: UpdateState) { state = next; listeners.forEach((listener) => listener()); }
export function useUpdates() { return useSyncExternalStore((listener) => { listeners.add(listener); return () => { listeners.delete(listener); }; }, () => state); }

export async function checkUpdates(manual = true) {
  if (state.phase === "checking" || state.phase === "installing") return;
  set({ phase: "checking" });
  try {
    await pending?.close();
    pending = null;
    const reason = await invoke<string>("update_support");
    pending = await check({ timeout: 15000 });
    if (pending && !pending.version.split("+")[0].includes("-")) {
      set({ phase: "available", version: pending.version, notes: pending.body, reason });
      if (!manual) A.notify(`Gall ${pending.version} is available. Open Settings → Updates to update.`, "info");
    } else {
      await pending?.close();
      pending = null;
      set({ phase: "current" });
    }
  } catch (error) { set({ phase: "error", error: String(error) }); }
}

export async function installUpdate() {
  if (!pending || state.phase !== "available" || state.reason) return;
  const available = state;
  set({ ...available, phase: "installing", bytes: 0 });
  try {
    if (Object.keys(getState().tasks).length) throw new Error("Wait for file operations to finish before installing an update.");
    const unlisten = await listen<number>("update-progress", (event) => set({ ...available, phase: "installing", bytes: event.payload }));
    try { await invoke("install_update"); } finally { unlisten(); }
  } catch (error) { set({ ...available, phase: "available", error: String(error) }); }
}

export function postponeUpdate() {
  if (state.phase !== "available") return;
  const update = pending;
  pending = null;
  set({ phase: "idle" });
  void update?.close().catch(() => {});
}
