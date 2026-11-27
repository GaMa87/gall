// Headless `--e2e-*` self-test modes driven by the opaque-box suite (tests/e2e, scripts/e2e-suite.sh).
// Each mode runs inside the real app with GALL_SELFTEST=<mode>, performs one focused check, writes
// PASS/FAIL lines to the report (`log`) and ends with `finish()` (report.txt + "N failure(s)").
import type { gitSelftest } from "./selftest";

export type SelftestTools = typeof gitSelftest;
/** A group handler: returns true when it owns `mode` (and has finished the report). */
export type ModeGroup = (mode: string, t: SelftestTools) => Promise<boolean>;

export async function runE2eSelfTest(mode: string, t: SelftestTools) {
  const groups: (() => Promise<ModeGroup>)[] = [
    async () => (await import("./selftestE2eCore")).runCoreMode, // cli-state, dbus-reveal, clip-*
    async () => (await import("./selftestE2eLayout")).runLayoutMode, // omarchy-*, adaptive-*, tiling, window-buttons, vim-*
    async () => (await import("./selftestE2eTools")).runToolsMode, // editor-*, terminal-*, openwith, gvfs-*, actions-*, zoxide
  ];
  for (const load of groups) {
    if (await (await load())(mode, t)) return;
  }
  t.log(false, `unknown selftest mode ${mode}`);
  await t.finish();
}
