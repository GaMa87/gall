import { expect, it } from "vitest";
import { commitUrl, copyChoices, splitMessage } from "./gitCopy";

it("splits a commit message into subject and body", () => {
  expect(splitMessage("Only subject\n")).toEqual({ subject: "Only subject", body: "" });
  expect(splitMessage("Subject\n\nBody line 1\nBody line 2\n")).toEqual({ subject: "Subject", body: "Body line 1\nBody line 2" });
  expect(splitMessage("Subject\r\n\r\nBody\r\n")).toEqual({ subject: "Subject", body: "Body" });
});

it("builds commit URLs only for known hosts", () => {
  const id = "abc123";
  expect(commitUrl("https://github.com/owner/repo.git", id)).toBe("https://github.com/owner/repo/commit/abc123");
  expect(commitUrl("git@github.com:owner/repo.git", id)).toBe("https://github.com/owner/repo/commit/abc123");
  expect(commitUrl("ssh://git@github.com/owner/repo", id)).toBe("https://github.com/owner/repo/commit/abc123");
  expect(commitUrl("https://user:token@github.com/owner/repo.git", id)).toBe("https://github.com/owner/repo/commit/abc123");
  expect(commitUrl("https://gitlab.com/group/sub/repo.git", id)).toBe("https://gitlab.com/group/sub/repo/-/commit/abc123");
  expect(commitUrl("https://bitbucket.org/team/repo.git", id)).toBe("https://bitbucket.org/team/repo/commits/abc123");
  expect(commitUrl("https://codeberg.org/o/r", id)).toBe("https://codeberg.org/o/r/commit/abc123");
  expect(commitUrl("https://git.example.com/o/r.git", id)).toBeNull();
  expect(commitUrl("/home/me/repo", id)).toBeNull();
  expect(commitUrl("file:///tmp/repo.git", id)).toBeNull();
});

it("never puts credentials into a copied URL", () => {
  expect(commitUrl("https://user:secret@github.com/owner/repo.git", "abc")).not.toContain("secret");
});

it("offers lazygit's attributes and skips empty ones", () => {
  const info = { id: "0123456789abcdef", author: "Ana", email: "ana@example.com", date: "2026-11-09", message: "Subject\n\nBody\n", tags: [], url: null };
  const labels = copyChoices(info, async () => "diff").map((choice) => choice.label);
  expect(labels).toEqual(["Commit hash", "Abbreviated hash", "Commit subject", "Commit message body", "Full commit message", "Author", "Date", "Commit diff"]);
  const full = copyChoices({ ...info, message: "Only subject", tags: ["v1", "v1.1"], url: "https://github.com/o/r/commit/x" }, async () => "diff");
  expect(full.map((choice) => choice.label)).toContain("Tags");
  expect(full.map((choice) => choice.label)).toContain("Commit URL");
  expect(full.map((choice) => choice.label)).not.toContain("Commit message body");
  expect(full.find((choice) => choice.label === "Abbreviated hash")?.text).toBe("01234567");
  expect(full.find((choice) => choice.label === "Author")?.text).toBe("Ana <ana@example.com>");
  expect(full.find((choice) => choice.label === "Tags")?.text).toBe("v1\nv1.1");
});
