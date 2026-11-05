// What Gall is, who made it and where it keeps its files. Settings › About shows it in
// full with every tip; the About window shows it compact with one tip at a time.
import * as A from "../state/actions";
import { useStore } from "../state/store";
import { FolderLink } from "./FolderLink";
import { Icon } from "./Icon";
import { FeaturedTips, TipCard, TipsList } from "./Tips";
import { Logo } from "./TitleBar";
import supportQr from "../../../docs/support/revtag.svg?url";

const SOURCE_URL = "https://github.com/GaMa87/gall";
const SUPPORT_URL = "https://revolut.me/gama87";

function openUrl(url: string) {
  import("@tauri-apps/plugin-opener").then(({ openUrl }) => openUrl(url)).catch(() => {});
}

const PLATFORM: Record<string, string> = { linux: "Linux", windows: "Windows", macos: "macOS" };

function SupportCard({ quiet = false }: { quiet?: boolean }) {
  return (
    <section className={`about-support${quiet ? " settings-support" : ""}`}>
      <div className="about-support-copy">
        <span className="about-support-label">Support Gall</span>
        <b>Enjoying Gall?</b>
        <p>Gall is free and built in spare time. If it saves you time, you can buy me a coffee.</p>
        <button className="about-support-link" onClick={() => openUrl(SUPPORT_URL)}>
          Buy me a coffee <span aria-hidden="true">·</span> Revolut
        </button>
        <span className="about-support-tag">revtag <code>@gama87</code></span>
        <p className="about-support-thanks">Stars, bug reports and pull requests help just as much. Thank you!</p>
      </div>
      <button
        className="about-support-qr"
        onClick={() => openUrl(SUPPORT_URL)}
        style={{ backgroundImage: `url("${supportQr}")` }}
        title="Open Revolut @gama87"
        aria-label="Open Revolut @gama87"
      />
    </section>
  );
}

export function AboutInfo({ full = false }: { full?: boolean }) {
  const info = useStore((s) => s.info);
  return (
    <div className={`about ${full ? "full" : "compact"}`}>
      <header className="about-hero">
        <Logo size={full ? 56 : 44} />
        <div className="about-title">
          <div><b>Gall</b>{info?.version && <span className="about-ver">v{info.version}</span>}</div>
          <span className="about-sub">Open-source Miller-column, dual-pane file manager</span>
          <div className="about-links">
            <button className="link-btn" onClick={() => openUrl(SOURCE_URL)}><Icon name="code" size={13} />Source code</button>
            <span className="about-dot">·</span>
            <span>MIT licence</span>
            <span className="about-dot">·</span>
            <span>by Matej Gajšek</span>
            <span className="about-dot">·</span>
            <button className="link-btn" onClick={() => A.openOverlay({ type: "welcome" })}><Icon name="wand" size={13} />Welcome setup</button>
          </div>
        </div>
      </header>

      <dl className="about-facts">
        <dt>Version</dt><dd>{info?.version}</dd>
        <dt>Platform</dt><dd>{PLATFORM[info?.platform ?? ""] ?? info?.platform}</dd>
        <dt>Settings</dt><dd>{info && <FolderLink path={info.configDir} />}</dd>
        <dt>Data</dt><dd>{info && <FolderLink path={info.dataDir} />}</dd>
      </dl>

      {full ? (
        <>
          <h4 className="about-section">Tips & tricks</h4>
          <FeaturedTips />
        </>
      ) : (
        <SupportCard />
      )}

      {full ? (
        <>
          <SupportCard quiet />
          <h4 className="about-section">More tips & tricks</h4>
          <TipsList />
        </>
      ) : (
        <TipCard />
      )}

      <p className="about-note">
        Independent implementation inspired by publicly documented workflows of OneCommander; not affiliated with or endorsed by its authors.
      </p>
    </div>
  );
}
