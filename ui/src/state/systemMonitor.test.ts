import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: () => Promise.resolve(null), convertFileSrc: (p: string) => p }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));
vi.stubGlobal("window", { setTimeout, clearTimeout });

import * as A from "./actions";
import { getState, monitorShown, setState } from "./store";
import type { Settings } from "../api/types";

const wish = () => getState().settings!.showSystemMonitor;
const crowd = (monitorCrowded: boolean) => setState({ monitorCrowded });

beforeEach(() => {
  setState({ settings: { showSystemMonitor: true } as Settings, monitorCrowded: false, monitorOverride: false });
});

describe("system monitor visibility", () => {
  it("follows the saved wish while there is room", () => {
    expect(monitorShown(getState())).toBe(true);
    setState({ settings: { ...getState().settings!, showSystemMonitor: false } });
    expect(monitorShown(getState())).toBe(false);
  });

  it("steps aside in a crowded sidebar without changing the saved wish", () => {
    crowd(true);
    expect(monitorShown(getState())).toBe(false);
    expect(wish()).toBe(true);
    crowd(false);
    expect(monitorShown(getState())).toBe(true);
  });

  it("the toolbar button shows a crowded monitor anyway, and hides it again", () => {
    crowd(true);
    A.toggleSystemMonitor();
    expect(monitorShown(getState())).toBe(true);
    expect(wish()).toBe(true);
    A.toggleSystemMonitor();
    expect(monitorShown(getState())).toBe(false);
    expect(wish()).toBe(false);
  });

  it("a monitor hidden by the user stays hidden when the window grows", () => {
    A.toggleSystemMonitor(false);
    crowd(true);
    crowd(false);
    expect(monitorShown(getState())).toBe(false);
  });

  it("turning the monitor on in a short window shows it right away", () => {
    A.toggleSystemMonitor(false);
    crowd(true);
    A.toggleSystemMonitor();
    expect(wish()).toBe(true);
    expect(monitorShown(getState())).toBe(true);
  });
});
