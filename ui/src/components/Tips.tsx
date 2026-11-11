// Tips & tricks (Settings › About, and one at a time in the About window). Keys are shown
// as they are now: vim keys when those are on, the user's own chords when rebound.
import { useState, type ReactNode } from "react";
import { ACTION_LABELS, type Action } from "../core/keymap";
import * as A from "../state/actions";
import { useKeyLabels } from "../state/keyLabels";
import { useStore } from "../state/store";
import { Icon } from "./Icon";

/** Renders an action's keys, or its name where it has none in the current mode. */
type K = (a: Action) => ReactNode;

interface Tip {
  title: string;
  body: (k: K, leader: string) => ReactNode;
}

const TIP_GROUPS: { title: string; icon: string; tips: Tip[] }[] = [
  {
    title: "Getting around", icon: "forward", tips: [
      { title: "Go to any file", body: (k) => <>{k("quickOpen")} searches the open folder and below; <kbd>Tab</kbd> switches to everywhere in the index. Unindexed folders are read on demand. Inside archives, only index search is available. End the query with <code>/</code> to list only folders; <code>ext:md</code>, <code>^start</code>, <code>end$</code>, <code>'exact</code> and <code>!skip</code> narrow it down.</> },
      { title: "Find text in files", body: (k) => <>{k("grepFolder")} searches inside the files of this folder, {k("grepRoot")} the whole system. <kbd>Tab</kbd> switches between the folder, Home and the drive; <kbd>Alt+C</kbd>, <kbd>Alt+W</kbd> and <kbd>Alt+R</kbd> toggle case, whole word and regex.</> },
      { title: "Folder marks", body: () => <>With Vim keys on, <kbd>m</kbd> and a letter marks the current folder; <kbd>'</kbd> and the letter goes back to it, <kbd>`</kbd> and the letter also selects the item you were on. <kbd>''</kbd> returns to where you jumped from. Marks are kept between sessions.</> },
      { title: "Hide search noise", body: () => <>Right-click › Hide from search adds an item to <code>.gallignore</code>; Show in search again brings it back. Rules also apply to subfolders, find in files and name search, without changing Git's rules.</> },
      { title: "Find ignored files", body: () => <><kbd>Alt+I</kbd> includes ignored files in find in files and the name search. In Quick Open it opens the name search in the current folder, including ignored files (not available inside archives).</> },
      { title: "Browse WSL on Windows", body: (k) => <>Open a distribution from Drives › Not mounted to start it. {k("terminal")} opens its Linux shell here. Delete uses its Linux trash where available; Gall's Trash window only shows the Windows Recycle Bin. Stop terminates every program in the distribution.</> },
      { title: "Type to filter", body: () => <>Start typing in a folder to filter it (with vim keys on, press <kbd>/</kbd> first). <kbd>Esc</kbd> clears the filter.</> },
      { title: "Every action by name", body: (k) => <>{k("commandPalette")} opens the command palette: search any action and see its keys.</> },
      { title: "A menu instead of shortcuts", body: (_k, leader) => leader ? <>Press <kbd>{leader}</kbd> (the leader key) for a menu of grouped actions; each letter leads one level deeper.</> : <>Set a leader key in Settings › Keyboard for a menu of grouped actions instead of shortcuts.</> },
      { title: "Type a path", body: (k) => <>{k("editPath")} turns the folder path into a text field; <code>~</code> and <code>$VARIABLES</code> work. It suggests favourites, tabs, history and a path on the clipboard, and <kbd>Tab</kbd> completes. Right-click it to copy the path in another format.</> },
      { title: "Recent files", body: (k) => <>{k("quickOpen")} with an empty query lists recent files and folders. <kbd>Ctrl+Enter</kbd> selects a result in its folder, <kbd>Shift+Enter</kbd> opens it in the other browser.</> },
      { title: "q closes windows", body: () => <><kbd>q</kbd> closes Quick Open, compare views, the preview and other windows like <kbd>Esc</kbd>, in every profile (not dialogs or notes, and not while typing).</> },
    ],
  },
  {
    title: "Two browsers", icon: "columns", tips: [
      { title: "Work across both", body: (k) => <>{k("focusOther")} jumps to the other browser; {k("copyToOther")} and {k("moveToOther")} copy or move the selection there.</> },
      { title: "Swap the folders", body: (k) => <>{k("swapBrowsers")} swaps the folders of the two browsers; each keeps its own panes. Double-click the divider to centre it.</> },
      { title: "Preview on one side only", body: (k) => <>{k("togglePreviewPane")} shows or hides the preview & details pane in the focused browser only.</> },
      { title: "Columns or breadcrumbs", body: (k) => <>{k("toggleColumns")} switches between Miller columns and a breadcrumb bar; {k("layoutNext")} cycles the layouts.</> },
      { title: "Title bar panel controls", body: () => <>The layout buttons in the title bar switch layouts. Above the divider, ‹ and › hide one browser and ⇄ swaps their folders; right-click there to show or hide each browser's details pane.</> },
      { title: "Pin a folder", body: (k) => <>{k("togglePin")} (or the star in the folder header) pins the current folder in the sidebar. With two browsers, hover a favourite to open it in the left or right one.</> },
      { title: "Favourites groups", body: () => <>Organise favourites in groups: the New group button at the bottom of the sidebar, + on a group adds the current folder, drop a folder on a group title, drag to reorder, give items an alias. Middle-click any sidebar item to open it in a background tab.</> },
      { title: "Tabs", body: (k) => <>{k("newTab")} opens a tab and {k("reopenTab")} brings back a closed one. Middle-click closes a tab; <kbd>Alt</kbd>+middle-click moves it to the other browser.</> },
    ],
  },
  {
    title: "Files", icon: "copy", tips: [
      { title: "Drag with a modifier", body: () => <>While dragging, hold <kbd>Ctrl</kbd> to copy, <kbd>Shift</kbd> to move or <kbd>Alt</kbd> to create a link.</> },
      { title: "Undo almost anything", body: (k) => <>{k("undo")} undoes copies, moves, renames (also batch renames), folder merges, deletes to the trash and extractions.</> },
      { title: "Rename many at once", body: (k) => <>Select several items and press {k("rename")}: a name template with counter and date, find & replace (also regex), case changes or edit all names as text. The preview flags clashes; one undo reverts it all.</> },
      { title: "Merge folders", body: () => <>When a copied or moved folder already exists, Merge puts both together; then choose for files in both: keep both, replace, replace older or skip.</> },
      { title: "Folder sizes", body: (k) => <>{k("folderSizes")} counts the selected folders, or all folders here, and shows the totals in the Size column. Calculate automatically in the Folder sizes menu keeps them up to date.</> },
      { title: "Hold Ctrl in menus", body: () => <>Hold <kbd>Ctrl</kbd> while a context menu is open to see the alternative items: Edit with…, Terminal here with…, and extract and move the archive to the trash.</> },
      { title: "Faster dialogs", body: (k) => <>In {k("newFolder")}, <kbd>Ctrl+Enter</kbd> creates and opens the folder and <kbd>Shift+Enter</kbd> moves the selection into it. While renaming, <kbd>F2</kbd> again selects the name, the extension or everything.</> },
      { title: "Trash window", body: () => <>Click Trash in the sidebar to restore items, show their original folder, delete for good or empty the trash. Dropping files on Trash moves them there.</> },
      { title: "Folder colours and header", body: () => <>Right-click › Folder colour, or the ⋯ menu in the folder header, colours a folder. The header's undo button undoes the last operation in that folder.</> },
      { title: "Notes", body: (k) => <>{k("newNote")} creates a <code>.2do</code> note with a done checkbox, right next to your files.</> },
      { title: "Always open with…", body: (k) => <>{k("openWith")} lists programs for the file. Pick one and press <kbd>Ctrl ↵</kbd> to always use it for that type; <kbd>Tab</kbd> narrows the rule to the project, the folder or only this file.</> },
      { title: "New from a template", body: (k) => <>{k("newFile")} creates a file from your templates; "Create file template" in the context menu adds any file to them.</> },
      { title: "Colour tags", body: (k) => <>{k("tag")} tags the selection with a colour; {k("search")} finds files by tag.</> },
    ],
  },
  {
    title: "Preview & compare", icon: "compare", tips: [
      { title: "Peek inside", body: (k) => <>{k("preview")} previews the file: Markdown is rendered, source code coloured, video and PDF played and paged, STL 3D models turned with the mouse.</> },
      { title: "Compare two files", body: (k) => <>Select two files and press {k("compare")}; the context menu also compares with the clipboard, with Git HEAD or with any commit ({k("compareGitCommit")}).</> },
      { title: "Sync two folders", body: (k) => <>{k("compareFolders")} compares the folders of both browsers and copies what differs: <kbd>&gt;</kbd> / <kbd>&lt;</kbd> copy the selection, <kbd>c</kbd> compares contents.</> },
      { title: "Keys in compare views", body: () => <><kbd>n</kbd> / <kbd>N</kbd> jump between changes, <kbd>u</kbd> switches split and unified, <kbd>Alt+→</kbd> / <kbd>Alt+←</kbd> copy a change across and <kbd>Ctrl+S</kbd> saves. Binary files open in a hex compare; three or more files give a similarity matrix.</> },
    ],
  },
  {
    title: "Archives", icon: "archive", tips: [
      { title: "Archives are folders", body: (k) => <>{k("open")} on a zip, 7z or tar opens it like a folder: preview, copy and open what is inside. Encrypted ones ask for the password.</> },
      { title: "Extract and clean up", body: () => <>Hold <kbd>Ctrl</kbd> in the context menu: the extract items also move the archive to the trash. One undo brings it back.</> },
    ],
  },
  {
    title: "Git", icon: "history", tips: [
      { title: "Git workspace", body: (k) => <>{k("gitGraph")} (or a click on the Git status at the bottom) opens the repository: history graph, single-line staging, commits, branches, stashes, rebase and a conflict editor. With Vim keys on, <kbd>?</kbd> there lists the keys of the focused panel; otherwise use its ? button.</> },
      { title: "Browse history safely", body: () => <>Selecting commits never changes your files. Checkout is an explicit action that offers to stash, commit or discard local changes, and rewritten history keeps a recovery reference.</> },
    ],
  },
  {
    title: "Make it yours", icon: "wand", tips: [
      { title: "Keyboard profiles", body: () => <>Choose Windows Explorer, Total Commander or Vim in Settings › Keyboard. Commander uses F5 copy, F6 move, F7 folder and F8 trash. Vim uses <kbd>j</kbd>/<kbd>k</kbd>, <kbd>gg</kbd>/<kbd>G</kbd> and <kbd>v</kbd> for visual mode. <kbd>?</kbd> shows the active profile's keys.</> },
      { title: "Your own shortcuts", body: () => <>Rebind any action in Settings › Shortcuts; menus and these tips show your keys.</> },
      { title: "Themes", body: (k) => <>Put a CSS file in the themes folder (Settings › Appearance) and pick it; {k("reloadTheme")} reloads it while you edit.</> },
      { title: "Portable", body: () => <>An empty file named <code>portable</code> next to the program keeps settings and data in a <code>UserData</code> folder beside it.</> },
      { title: "Report a problem", body: () => <>Report a bug in About adds screenshots, annotations and short clips of Gall, then fills in the GitHub form. Nothing is uploaded automatically.</> },
    ],
  },
];

const ALL_TIPS = TIP_GROUPS.flatMap((g) => g.tips);
const FEATURED_TIP_TITLES = new Set(["Go to any file", "Work across both", "Peek inside"]);

function useTipKeys(): { k: K; leader: string } {
  const keys = useKeyLabels();
  const s = useStore((st) => st.settings);
  const leaderSetting = s?.vimKeys ? s.leaderKey || "Space" : s?.leaderKeyBasic ?? "Ctrl Ctrl";
  const k: K = (a) => {
    const label = keys.action(a);
    return label ? <kbd>{label}</kbd> : <em>{ACTION_LABELS[a]}</em>;
  };
  return { k, leader: leaderSetting };
}

/** A few standout tips shown before the support card in Settings › About. */
export function FeaturedTips() {
  const { k, leader } = useTipKeys();
  const tips = ALL_TIPS.filter((tip) => FEATURED_TIP_TITLES.has(tip.title));
  return (
    <div className="tip-grid">
      {tips.map((tip) => (
        <div key={tip.title} className="tip">
          <b>{tip.title}</b>
          <p>{tip.body(k, leader)}</p>
        </div>
      ))}
    </div>
  );
}

/** Remaining tips, grouped. */
export function TipsList() {
  const { k, leader } = useTipKeys();
  return (
    <div className="tips">
      {TIP_GROUPS.map((g) => {
        const tips = g.tips.filter((tip) => !FEATURED_TIP_TITLES.has(tip.title));
        return tips.length > 0 && (
        <section key={g.title} className="tip-group">
          <h5><Icon name={g.icon} size={14} />{g.title}</h5>
          <div className="tip-grid">
            {tips.map((tip) => (
              <div key={tip.title} className="tip">
                <b>{tip.title}</b>
                <p>{tip.body(k, leader)}</p>
              </div>
            ))}
          </div>
        </section>
        );
      })}
    </div>
  );
}

/** One tip at a time, with the next one a click away (About window). */
export function TipCard() {
  const { k, leader } = useTipKeys();
  const [i, setI] = useState(() => Math.floor(Math.random() * ALL_TIPS.length));
  const t = ALL_TIPS[i];
  return (
    <div className="tip-card">
      <div className="tip-card-head">
        <span className="tip-badge"><Icon name="star" size={12} />Tip</span>
        <b>{t.title}</b>
        <span className="tip-count">{i + 1} / {ALL_TIPS.length}</span>
      </div>
      <p>{t.body(k, leader)}</p>
      <div className="tip-card-actions">
        <button className="link-btn" onClick={() => A.openOverlay({ type: "settings", section: "about" })}>All tips</button>
        <button className="btn" onClick={() => setI((i + 1) % ALL_TIPS.length)}>Next tip<Icon name="chevronRight" size={13} /></button>
      </div>
    </div>
  );
}
