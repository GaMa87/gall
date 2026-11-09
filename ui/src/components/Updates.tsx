import { openUrl } from "@tauri-apps/plugin-opener";
import { useStore } from "../state/store";
import { checkUpdates, installUpdate, postponeUpdate, useUpdates } from "../state/updates";
import { Icon } from "./Icon";

export function Updates() {
  const update = useUpdates();
  const busy = useStore((s) => Object.keys(s.tasks).length > 0);
  const version = useStore((s) => s.info?.version);
  const working = update.phase === "checking" || update.phase === "installing";
  const status = update.phase === "checking" ? "Checking" : update.phase === "installing" ? "Installing" :
    update.phase === "current" ? "Up to date" : update.phase === "error" ? "Check unavailable" : update.version ? "Update available" : "Stable releases";
  const heading = update.phase === "checking" ? "Checking for updates…" : update.phase === "installing" ? "Installing your update…" :
    update.phase === "current" ? "You're up to date" : update.phase === "error" ? "Couldn't check for updates" :
      update.version ? `Gall ${update.version} is available` : "Keep Gall up to date";
  return <section className="updates-card" data-state={update.phase} aria-label="Application updates" aria-busy={working}>
    <header className="updates-header">
      <span className={`updates-icon${working ? " is-working" : ""}`}><Icon name={update.phase === "current" ? "check" : "refresh"} size={20} /></span>
      <div className="updates-heading"><h4>Software updates</h4><span>{version ? `Installed version ${version}` : "Gall for desktop"}</span></div>
      <span className="updates-badge">{status}</span>
    </header>
    <div className="updates-body" role="status" aria-live="polite">
      <h5>{heading}</h5>
      <p>{update.phase === "checking" ? "Looking for the latest stable release on GitHub." :
        update.phase === "installing" ? "Gall will restart when installation finishes. Keep the application open." :
        update.phase === "current" ? "You are using the latest stable version of Gall." :
        update.phase === "error" ? "Try again in a moment and check your internet connection if the problem continues." :
        update.version ? "Install when you're ready. You can keep working until then." : "Check for the latest improvements and fixes. Updates are installed with your approval."}</p>
    </div>
    {update.phase === "installing" && <div className="updates-progress">
      <progress aria-label="Installing update" />
      <span>{Math.round((update.bytes ?? 0) / 1024).toLocaleString()} KB downloaded</span>
    </div>}
    {update.version && update.notes && <details className="updates-notes"><summary>What's new in {update.version}</summary><div tabIndex={0} aria-label="Release notes">{update.notes}</div></details>}
    {update.version && update.reason && <p className="updates-hint"><Icon name="info" size={15} /><span>{update.reason}</span></p>}
    {update.version && busy && !update.reason && <p className="updates-hint"><Icon name="history" size={15} /><span>Wait for file operations to finish before installing an update.</span></p>}
    {update.error && <div className="updates-error" role="alert">
      <Icon name="alert" size={16} />
      <div><p>{update.phase === "available" ? "The update couldn't be installed. You can try again." : "Your current version is ready to use. Please try checking again later."}</p>
        <details><summary>Technical details</summary><pre tabIndex={0} aria-label="Technical error details">{update.error}</pre></details>
      </div>
    </div>}
    <footer className="updates-actions">
      {update.version && <>
        {update.reason ? <button className="btn primary" disabled={working} onClick={() => { void openUrl("https://github.com/GaMa87/gall/releases/latest"); }}><Icon name="download" size={15} />Download update</button> :
          <button className="btn primary" disabled={working || busy} onClick={() => { void installUpdate(); }}><Icon name="download" size={15} />Install update and restart</button>}
        <button className="btn" disabled={working} onClick={postponeUpdate}>Later</button>
      </>}
      <button className={`btn updates-check${update.version ? "" : " primary"}`} disabled={working} onClick={() => { void checkUpdates(); }}><Icon name="refresh" size={14} />Check for updates</button>
    </footer>
  </section>;
}
