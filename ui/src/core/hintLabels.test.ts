import { expect, it } from "vitest";
import { HINT_ALPHABET, HINT_TARGETS, hintLabels, matchHint, maxHintLabels, onScreen } from "./hintLabels";
import { gitCommandForKey, gitCommands } from "./gitCommands";

const prefixFree = (labels: string[]) => labels.every((a, i) => labels.every((b, j) => i === j || !b.startsWith(a)));

it("makes unique labels of one or two letters, none the beginning of another, for any count", () => {
  for (const count of [0, 1, 5, 26, 27, 40, 100, 300, 676]) {
    const labels = hintLabels(count);
    expect(labels).toHaveLength(count);
    expect(new Set(labels).size).toBe(count);
    expect(prefixFree(labels)).toBe(true);
    expect(labels.every((label) => /^[a-z]{1,2}$/.test(label))).toBe(true);
  }
});

it("uses single letters while they are enough and the most single letters that the count allows after that", () => {
  expect(hintLabels(10).every((label) => label.length === 1)).toBe(true);
  expect(hintLabels(26)).toEqual([...HINT_ALPHABET]);
  const some = hintLabels(40), singles = some.filter((label) => label.length === 1).length;
  expect(singles).toBeGreaterThan(0);
  expect(singles).toBeLessThan(26);
  // One more single letter would not leave room for the two-letter labels the count needs.
  expect(singles + (26 - singles) * 26).toBeGreaterThanOrEqual(40);
  expect(singles + 1 + (26 - singles - 1) * 26).toBeLessThan(40);
  // Home row letters come first.
  expect(hintLabels(3)).toEqual(["a", "s", "d"]);
});

it("makes no more than two letters give, and none for a negative count", () => {
  expect(maxHintLabels()).toBe(676);
  expect(hintLabels(5000)).toHaveLength(676);
  expect(hintLabels(-3)).toEqual([]);
  expect(hintLabels(5, "ab")).toHaveLength(4);
  expect(hintLabels(3, "ab")).toEqual(["a", "ba", "bb"]);
  expect(prefixFree(hintLabels(4, "ab"))).toBe(true);
});

it("tells what typed letters name, ignoring case", () => {
  const labels = hintLabels(40);
  const two = labels.findIndex((label) => label.length === 2);
  expect(matchHint(labels[two][0], labels)).toMatchObject({ partial: expect.arrayContaining([two]) });
  expect(matchHint(labels[two].toUpperCase(), labels)).toEqual({ exact: two });
  expect(matchHint(labels[0], labels)).toEqual({ exact: 0 });
  expect(matchHint("", labels)).toHaveProperty("partial");
  expect(matchHint("1", labels)).toEqual({ none: true });
  // A label never waits for a second letter: a one-letter label is exact at once.
  expect(matchHint("a", labels)).toEqual({ exact: 0 });
});

it("labels only what is on screen and has a size", () => {
  const view = { left: 0, top: 0, right: 800, bottom: 600 };
  expect(onScreen({ left: 10, top: 10, right: 50, bottom: 30 }, view)).toBe(true);
  expect(onScreen({ left: 790, top: 590, right: 900, bottom: 700 }, view)).toBe(true);
  expect(onScreen({ left: 801, top: 10, right: 900, bottom: 30 }, view)).toBe(false);
  expect(onScreen({ left: -50, top: 10, right: -1, bottom: 30 }, view)).toBe(false);
  expect(onScreen({ left: 10, top: 700, right: 50, bottom: 720 }, view)).toBe(false);
  expect(onScreen({ left: 10, top: 10, right: 10, bottom: 30 }, view)).toBe(false);
});

it("aims at buttons, rows, tabs and fields, not at every line of a diff", () => {
  for (const part of ["button", ".gg-row", ".gg-side-row", ".gc-file", "[role=tab]", "input:not([type=hidden])", "select"]) expect(HINT_TARGETS).toContain(part);
  expect(HINT_TARGETS).not.toContain(".gc-diff-line");
});

it("has a Git command for it on `;` and the chord Ctrl+;, which works without Vim keys and hides no other key", () => {
  for (const scope of ["history", "branches", "tags", "stashes", "files", "worktrees"] as const) {
    const command = gitCommandForKey(scope, ";");
    expect(command?.action, scope).toBe("hints");
    expect(gitCommandForKey(scope, "Ctrl+;")?.action, scope).toBe("hints");
    expect(command?.always).toBe(true);
  }
  expect(gitCommandForKey("history", "f")?.action).toBe("quick:fixup");
  expect(gitCommandForKey("branches", "f")?.action).toBe("fetch");
  expect(gitCommands("history").filter((command) => command.keys.includes(";"))).toHaveLength(1);
});
