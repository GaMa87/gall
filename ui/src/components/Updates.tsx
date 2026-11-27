import { openUrl } from "@tauri-apps/plugin-opener";
import { tt } from "../i18n";
import * as A from "../state/actions";
import { useStore } from "../state/store";
import { applyUpdate, checkUpdates, installUpdate, postponeUpdate, revertOverlayUpdate, useUpdates } from "../state/updates";
import { FolderLink } from "./FolderLink";
import { Icon } from "./Icon";

function formatBytesMb(bytes: number): string {
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

const STAGE_LABEL = {
  downloading: "Downloading update…",
  verifying: "Verifying signature…",
  extracting: "Extracting update…",
  probing: "Checking compatibility…",
  applying: "Applying update…",
  restarting: "Restarting Gall…",
} as const;

export function Updates() {
  const update = useUpdates();
  const busy = useStore((s) => Object.keys(s.tasks).length > 0);
  const info = useStore((s) => s.info);
  const autoDownload = useStore((s) => s.settings?.autoDownloadUpdates ?? true);
  const version = info?.version;
  const working = update.phase === "checking" || update.phase === "downloading" || update.phase === "applying";

  const status = update.phase === "checking" ? "Checking"
    : update.phase === "downloading" ? "Downloading"
      : update.phase === "applying" ? "Restarting"
        : update.phase === "ready" ? "Ready to restart"
          : update.phase === "current" ? "Up to date"
            : update.phase === "error" ? "Check unavailable"
              : update.version ? "Update available" : "Stable releases";

  const heading = update.phase === "checking" ? "Checking for updates…"
    : update.phase === "downloading" ? "Downloading update…"
      : update.phase === "applying" ? "Restarting into the update…"
        : update.phase === "ready" && update.version ? `Gall ${update.version} is ready to install`
          : update.phase === "current" ? "You're up to date"
            : update.phase === "error" ? "Couldn't check for updates"
              : update.version ? `Gall ${update.version} is available` : "Keep Gall up to date";

  const bodyText = update.phase === "checking" ? "Looking for the latest stable release on GitHub."
    : update.phase === "downloading" ? "Downloading, verifying and staging in the background. You can keep working until you restart."
      : update.phase === "applying" ? "Gall will restart automatically once the update is applied."
        : update.phase === "ready" ? "The update is verified and staged. Restart when you're ready to switch to the new version."
          : update.phase === "current" ? "You are using the latest stable version of Gall."
            : update.phase === "error" ? "Try again in a moment and check your internet connection if the problem continues."
              : update.version ? "Install when you're ready. You can keep working until then."
                : "Check for the latest improvements and fixes. Updates are verified with Gall's signing key.";

  const strategy = update.strategy ?? info?.updateStrategy;
  // Installed by the distribution's package manager (pacman, `omarchy update`): Gall never downloads
  // or replaces itself, so no install / download actions are offered.
  const distro = (update.installKind ?? info?.installKind) === "distro-package";
  const installDir = update.installDir ?? info?.installDir;
  const updatesDir = update.updatesDir ?? info?.updatesDir;
  const baseVersion = update.baseVersion ?? info?.baseVersion;
  const baseExePath = update.baseExePath ?? info?.baseExePath;
  const runningOverlay = update.runningOverlay ?? info?.runningOverlay ?? false;

  const downloaded = update.downloaded ?? 0;
  const total = update.total ?? null;
  const hasTotal = update.stage === "downloading" && total !== null && total > 0;
  const pct = hasTotal ? Math.min(100, Math.round((downloaded / total) * 100)) : null;
  const stageLabel = STAGE_LABEL[update.stage ?? "downloading"];

  const heading2 = distro ? (update.version ? `Gall ${update.version} is available` : "Updates are managed by your system") : heading;
  const bodyText2 = distro
    ? update.version
      ? "Update Gall with your system package manager (for example `omarchy update` or `sudo pacman -Syu`)."
      : "Gall was installed by your distribution's package manager. Update it together with the rest of the system (for example `omarchy update` or `sudo pacman -Syu`)."
    : bodyText;

  return (
    <div className="updates-stack">
      <section className="updates-card" data-state={update.phase} aria-label={tt("Application updates")} aria-busy={working}>
        <header className="updates-header">
          <span className={`updates-icon${working ? " is-working" : ""}`}><Icon name={update.phase === "current" || update.phase === "ready" ? "check" : "refresh"} size={20} /></span>
          <div className="updates-heading">
            <h4>{tt("Software updates")}</h4>
            <span>
              {version ? `Installed version ${version}` : "Gall for desktop"}
              {runningOverlay && baseVersion ? ` (updated over ${baseVersion})` : ""}
            </span>
          </div>
          <span className="updates-badge">{status}</span>
        </header>
        <div className="updates-body" role="status" aria-live="polite">
          <h5>{heading2}</h5>
          <p>{bodyText2}</p>
        </div>
        {(update.phase === "downloading" || update.phase === "applying") && (
          <div className="updates-progress">
            {hasTotal ? <progress aria-label={tt("Preparing update")} value={downloaded} max={total} /> : <progress aria-label={tt("Preparing update")} />}
            <span>
              {stageLabel}
              {update.stage === "downloading" && downloaded > 0 && (hasTotal ? ` ${formatBytesMb(downloaded)} / ${formatBytesMb(total)} (${pct}%)` : ` ${Math.round(downloaded / 1024).toLocaleString()} KB downloaded`)}
            </span>
          </div>
        )}
        {update.version && update.notes && <details className="updates-notes"><summary>What's new in {update.version}</summary><div tabIndex={0} aria-label={tt("Release notes")}>{update.notes}</div></details>}
        {update.version && update.reason && <p className="updates-hint"><Icon name="info" size={15} /><span>{update.reason}</span></p>}
        {(update.version || runningOverlay) && busy && !update.reason && <p className="updates-hint"><Icon name="history" size={15} /><span>{tt("Wait for file operations to finish before applying an update.")}</span></p>}
        {update.error && (
          <div className="updates-error" role="alert">
            <Icon name="alert" size={16} />
            <div>
              <p>{update.phase === "available" || update.phase === "ready" ? "The update couldn't be completed. You can try again." : "Your current version is ready to use. Please try checking again later."}</p>
              <details><summary>{tt("Technical details")}</summary><pre tabIndex={0} aria-label={tt("Technical error details")}>{update.error}</pre></details>
            </div>
          </div>
        )}
        <footer className="updates-actions">
          {!distro && update.phase === "ready" && !update.reason && (
            <>
              <button className="btn primary" disabled={working || busy} onClick={() => { void applyUpdate(); }}><Icon name="refresh" size={15} />{tt("Restart to update")}</button>
              <button className="btn" disabled={working} onClick={postponeUpdate}>{tt("Later")}</button>
            </>
          )}
          {!distro && update.version && update.phase !== "ready" && (
            <>
              {update.reason ? (
                <button className="btn primary" disabled={working} onClick={() => { void openUrl("https://github.com/GaMa87/gall/releases/latest"); }}><Icon name="download" size={15} />{tt("Download update")}</button>
              ) : (
                <button className="btn primary" disabled={working || busy} onClick={() => { void installUpdate(); }}><Icon name="download" size={15} />{tt("Install update and restart")}</button>
              )}
              <button className="btn" disabled={working} onClick={postponeUpdate}>{tt("Later")}</button>
            </>
          )}
          <button className={`btn updates-check${update.version ? "" : " primary"}`} disabled={working} onClick={() => { void checkUpdates(); }}><Icon name="refresh" size={14} />{tt("Check for updates")}</button>
        </footer>
      </section>

      {!distro && <section className="sgroup">
        <h4>{tt("Automatic updates")}</h4>
        <div className="scard">
          <div className="opt toggle">
            <label htmlFor="opt-autoDownloadUpdates" className="opt-text">
              <span className="opt-label">{tt("Download updates automatically")}</span>
              <small>{tt("Download and verify new releases in the background, then prompt to restart when ready. Turn off to only show a notification.")}</small>
            </label>
            <div className="opt-ctl">
              <input
                id="opt-autoDownloadUpdates"
                type="checkbox"
                className="switch"
                role="switch"
                title={tt("Download updates automatically")}
                checked={autoDownload}
                onChange={(e) => { void A.updateSettings({ autoDownloadUpdates: e.target.checked }); }}
              />
            </div>
          </div>
        </div>
      </section>}

      {(installDir || updatesDir) && (
        <section className="sgroup updates-location">
          <h4>{tt("Where updates are stored")}</h4>
          <div className="scard">
            {strategy === "overlay" && updatesDir ? (
              <div className="opt">
                <div className="opt-text">
                  <span className="opt-label">{tt("User-space update folder")}</span>
                  <small>
                    {tt("Updates are verified and installed into")} <FolderLink path={updatesDir} />. The base installation
                    {baseExePath ? <> (<code>{baseExePath}</code>)</> : installDir ? <> in <FolderLink path={installDir} /></> : null} stays
                    untouched for your system package manager and launches the updated version automatically.
                  </small>
                </div>
                {runningOverlay && baseVersion && (
                  <div className="opt-ctl">
                    <button className="btn" disabled={working || busy} onClick={() => { void revertOverlayUpdate(); }}>
                      Revert to {baseVersion}
                    </button>
                  </div>
                )}
              </div>
            ) : strategy === "inPlace" && installDir ? (
              <div className="opt">
                <div className="opt-text">
                  <span className="opt-label">{tt("In-place application folder")}</span>
                  <small>{tt("Updates are verified and replace Gall directly inside")} <FolderLink path={installDir} /> {tt("while preserving your settings.")}</small>
                </div>
              </div>
            ) : (
              <div className="opt">
                <div className="opt-text">
                  <span className="opt-label">{tt("Managed externally")}</span>
                  <small>{distro
                    ? "Updates are installed by your system package manager (pacman / omarchy update), not by Gall."
                    : "Updates for this build are delivered through your package manager or software store."}</small>
                </div>
              </div>
            )}
          </div>
        </section>
      )}
    </div>
  );
}
