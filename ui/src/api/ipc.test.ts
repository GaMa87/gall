import { describe, expect, it } from "vitest";
import { shouldRecordSuccessCommand } from "./ipc";

describe("shouldRecordSuccessCommand", () => {
  it("skips routine background polling and cursor preview commands", () => {
    for (const cmd of [
      "system_stats",
      "list_drives",
      "other_volumes",
      "gvfs_network_mounts",
      "list_cloud_folders",
      "selection_publish",
      "grep_context",
      "bug_report_prepare",
      "selftest_done",
      "ui_log",
    ]) {
      expect(shouldRecordSuccessCommand(cmd)).toBe(false);
    }
  });

  it("records real user commands", () => {
    for (const cmd of ["list_dir", "session_cmd", "grep_start", "set_prefs", "set_tag"]) {
      expect(shouldRecordSuccessCommand(cmd)).toBe(true);
    }
  });
});
