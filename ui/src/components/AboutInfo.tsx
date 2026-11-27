// What Gall is, who made it and where it keeps its files. Settings › About shows it in
// full with every tip; the About window shows it compact with one tip at a time.
import * as A from "../state/actions";
import { tt } from "../i18n";
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
        <span className="about-support-label">{tt("Support Gall")}</span>
        <b>{tt("Enjoying Gall?")}</b>
        <p>{tt("Gall is free and built in spare time. If it saves you time, you can buy me a coffee.")}</p>
        <button className="about-support-link" onClick={() => openUrl(SUPPORT_URL)}>
          {tt("Buy me a coffee")} <span aria-hidden="true">·</span> {tt("Revolut")}
        </button>
        <span className="about-support-tag">revtag <code>@gama87</code></span>
        <p className="about-support-thanks">{tt("Stars, bug reports and pull requests help just as much. Thank you!")}</p>
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
  const reportButton = <button className="btn about-report-button" onClick={() => A.openOverlay({ type: "bugReport" })}>
    <span className="about-report-icon"><Icon name="activity" size={15} /></span><span>{tt("Report a bug")}</span>
  </button>;
  return (
    <div className={`about ${full ? "full" : "compact"}`}>
      <header className="about-hero">
        <Logo size={full ? 56 : 44} />
        <div className="about-title">
          <div><b>{tt("Gall")}</b>{info?.version && <span className="about-ver">v{info.version}</span>}</div>
          <span className="about-sub">{tt("Open-source Miller-column, dual-pane file manager")}</span>
          <div className="about-links">
            {!full && reportButton}
            <button className="link-btn" onClick={() => openUrl(SOURCE_URL)}><Icon name="code" size={13} />{tt("Source code")}</button>
            <span className="about-dot">·</span>
            <span>{tt("MIT licence")}</span>
            <span className="about-dot">·</span>
            <span>by Matej Gajšek</span>
            <span className="about-dot">·</span>
            <button className="link-btn" onClick={() => A.openOverlay({ type: "welcome" })}><Icon name="wand" size={13} />{tt("Welcome setup")}</button>
          </div>
        </div>
        {full && reportButton}
      </header>


      <dl className="about-facts">
        <dt>{tt("Version")}</dt>
        <dd>
          {info?.version}
          {info?.runningOverlay && info.baseVersion ? <span className="about-fact-note">(updated over {info.baseVersion})</span> : null}
        </dd>
        <dt>{tt("Platform")}</dt><dd>{PLATFORM[info?.platform ?? ""] ?? info?.platform}</dd>
        <dt>{tt("Settings")}</dt><dd>{info && <FolderLink path={info.configDir} />}</dd>
        <dt>{tt("Data")}</dt><dd>{info && <FolderLink path={info.dataDir} />}</dd>
        <dt>{tt("Install")}</dt>
        <dd>
          {info && (
            <span className="about-fact-stack">
              <FolderLink path={info.installDir} />
              {info.runningOverlay && info.baseExePath ? <small>Base installation ({info.baseExePath}) stays untouched</small> : null}
            </span>
          )}
        </dd>
        <dt>{tt("Updates")}</dt>
        <dd>
          {info && (
            info.updateStrategy === "overlay" ? (
              <span className="about-fact-stack">
                <FolderLink path={info.updatesDir} />
                <small>{tt("In-app updates install here (system install stays untouched)")}</small>
              </span>
            ) : info.updateStrategy === "inPlace" ? (
              <span className="about-fact-stack">
                <FolderLink path={info.installDir} />
                <small>{tt("Updated in place")}</small>
              </span>
            ) : info.updateStrategy === "storeManaged" ? (
              <span>{tt("Managed by package manager / software store")}</span>
            ) : (
              <span>{tt("Not configured in this build")}</span>
            )
          )}
        </dd>
      </dl>

      {full ? (
        <>
          <h4 className="about-section">{tt("Tips & tricks")}</h4>
          <FeaturedTips />
        </>
      ) : (
        <SupportCard />
      )}

      {full ? (
        <>
          <SupportCard quiet />
          <h4 className="about-section">{tt("More tips & tricks")}</h4>
          <TipsList />
        </>
      ) : (
        <TipCard />
      )}

      <p className="about-note">
        {tt("Independent implementation inspired by publicly documented workflows of OneCommander; not affiliated with or endorsed by its authors.")}
      </p>
    </div>
  );
}
