// Graphical self-test for WN-152: the revision scrubber of the commit details (scripts/selftest.sh --git-scrubber) on a
// file with a rename and a merge: metadata bar, parent choice of a merge, root commit, keys `[` `]` `D`, focus after the toggle.
import * as A from "./state/actions";
import type { gitSelftest } from "./selftest";

export async function runGitScrubberSelfTest(h: typeof gitSelftest, repoPath: string) {
  const { $, $$, key, click, until, sleep, shot, log, setWindowSize } = h;
  const text = (sel: string) => $(sel)?.textContent ?? "";
  const details = () => $<HTMLElement>(".git-commit-details");
  const patch = () => text(".gcd-patch");
  const target = () => (details()?.contains(document.activeElement) ? document.activeElement : details()) as HTMLElement;
  const press = async (k: string) => { key(k, {}, target()); await sleep(250); };
  const count = () => text(".gcd-scrub > small:last-child");
  const settled = () => until(() => !$(".gcd-scrub")?.getAttribute("aria-busy")?.includes("true") && !patch().includes("Loading the change"), 6000);
  try {
    await h.updateSettings({ theme: "dark", keyboardProfile: "vim" });
    await setWindowSize(1320, 900);
    A.openOverlay({ type: "gitGraph", path: repoPath });
    const row = await until(() => $$(".git-graph-lines [data-commit]").find((r) => r.textContent?.includes("merge side")), 8000);
    log(!!row, "git-scrubber: the history shows the merge commit");
    if (!row) return;
    click(row);
    await until(() => $(".gcd-file.selected"), 6000);
    await press("v");
    await until(() => $(".gcd-scrub"), 8000);
    await press("l");
    await settled();
    log(!!$(".gcd-scrub input[type=range]"), "git-scrubber: the file view shows the scrubber bar");
    log(/^\d+ \/ \d+$/.test(count().trim()) && text(".gcd-scrub-meta").includes("Test"), "git-scrubber: the bar shows the position and the metadata", `${count()} | ${text(".gcd-scrub-meta")}`);
    await shot("gs-01-scrubber-file");

    // The change against the parent: the merge has two, chosen without another request for the commit.
    await press("D");
    await until(() => patch().includes("Merge: compared with parent 1 of 2"), 6000);
    log(patch().includes("Merge: compared with parent 1 of 2"), "git-scrubber: the merge is compared with parent 1 of 2");
    const select = await until(() => $<HTMLSelectElement>(".gcd-scrub select"), 3000);
    log(!!select && select.options.length === 2, "git-scrubber: the parent selector of a merge has two parents", `${select?.options.length}`);
    log(details()?.contains(document.activeElement) ?? false, "git-scrubber: the focus stays in the details after the toggle", document.activeElement?.className ?? "");
    await shot("gs-02-merge-parent-1");
    const first = patch();
    if (select) { select.value = "1"; select.dispatchEvent(new Event("change", { bubbles: true })); }
    await until(() => patch().includes("parent 2 of 2"), 6000);
    log(patch().includes("parent 2 of 2") && patch() !== first, "git-scrubber: choosing parent 2 shows the change against it", patch().slice(0, 120));
    await shot("gs-03-merge-parent-2");

    // Keys step older and newer revisions; the last answer wins when stepping fast.
    await press("[");
    await settled();
    log(count().trim().startsWith("2 /"), "git-scrubber: [ steps to the older revision", count());
    key("]", {}, target()); key("[", {}, target()); key("[", {}, target());
    await sleep(700); await settled();
    log(count().trim().startsWith("3 /") && !patch().includes("Loading the change"), "git-scrubber: fast stepping shows only the last revision", `${count()} | ${patch().slice(0, 80)}`);
    for (let i = 0; i < 8; i++) await press("[");
    await settled();
    log(patch().includes("First commit: compared with an empty tree") && !$(".gcd-scrub select"), "git-scrubber: the root commit is compared with an empty tree", patch().slice(0, 100));
    const meta = text(".gcd-scrub-at");
    log(meta.includes("add old.txt"), "git-scrubber: the oldest revision uses the name before the rename", meta);
    await shot("gs-04-root-commit");
    await press("D");
    await until(() => !patch().includes("First commit"), 6000);
    log(patch().includes("one") && patch().includes("five"), "git-scrubber: D shows the file as of the revision again", patch().slice(0, 80));
    await shot("gs-05-file-again");
  } finally {
    A.openOverlay(null);
  }
}
