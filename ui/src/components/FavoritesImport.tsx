// Bring the places of another file manager along: a checklist per detected source (GTK bookmarks, KDE Places…).
// Used as a step of the Welcome setup and as a dialog (Settings › Window › Favourites, command palette). Nothing is
// read before this opens; every path and label is shown as text and never interpreted.
import { useEffect, useRef, useState } from "react";
import { api } from "../api/ipc";
import * as A from "../state/actions";
import { tt } from "../i18n";
import { choiceFor, importable, initialTicks, SOURCE_HINTS, skippedReason, tally, type ImportChoice, type ImportPlace, type ImportSourceInfo } from "../core/favoritesImport";

type List = { places: ImportPlace[]; ticked: Set<number> } | { error: string };

export function FavoritesImportPanel({ sources, onImported }: { sources: ImportSourceInfo[]; onImported?: () => void }) {
  const [lists, setLists] = useState<Record<string, List>>({});
  const [busy, setBusy] = useState(false);
  // Bumped after an import: the lists are read again, so what was just added shows as already a favorite.
  const [round, setRound] = useState(0);
  useEffect(() => {
    let alive = true;
    for (const source of sources) {
      api.favoritesImportRead(source.id)
        .then((places) => { if (alive) setLists((l) => ({ ...l, [source.id]: { places, ticked: initialTicks(places) } })); })
        .catch((e) => { if (alive) setLists((l) => ({ ...l, [source.id]: { error: String(e instanceof Error ? e.message : e) } })); });
    }
    return () => { alive = false; };
  }, [sources, round]);

  const setTicks = (id: string, next: (places: ImportPlace[], old: Set<number>) => Set<number>) =>
    setLists((l) => { const cur = l[id]; return cur && "places" in cur ? { ...l, [id]: { ...cur, ticked: next(cur.places, cur.ticked) } } : l; });
  const toggle = (id: string, i: number) => setTicks(id, (_, old) => { const n = new Set(old); if (!n.delete(i)) n.add(i); return n; });

  const choices: ImportChoice[] = sources.flatMap((source) => {
    const list = lists[source.id];
    const choice = list && "places" in list ? choiceFor(source.label, list.places, list.ticked) : null;
    return choice ? [choice] : [];
  });
  const total = sources.reduce((sum, source) => { const l = lists[source.id]; return l && "places" in l ? sum + tally(l.places, l.ticked).ticked : sum; }, 0);
  const usable = sources.reduce((sum, source) => { const l = lists[source.id]; return l && "places" in l ? sum + tally(l.places, l.ticked).usable : sum; }, 0);
  const run = async () => {
    if (busy || !choices.length) return;
    setBusy(true);
    const added = await A.importFavorites(choices);
    setBusy(false);
    if (added) { setRound((r) => r + 1); onImported?.(); }
  };

  return (
    <div className="fi">
      {sources.map((source) => {
        const list = lists[source.id];
        const hint = SOURCE_HINTS[source.id];
        return (
          <section key={source.id} className="fi-source" aria-label={tt(source.label)}>
            <h3>{tt(source.label)}{list && "places" in list && <span>{list.places.length}</span>}</h3>
            {hint && <p className="fi-hint">{tt(hint)}</p>}
            {!list && <p className="fi-note" role="status">{tt("Reading…")}</p>}
            {list && "error" in list && <p className="fi-note fi-error" role="alert">{list.error}</p>}
            {list && "places" in list && (list.places.length === 0
              ? <p className="fi-note">{tt("No places in this list.")}</p>
              : <>
                <div className="fi-tools">
                  <button type="button" className="link-btn" onClick={() => setTicks(source.id, (places) => new Set(places.flatMap((p, i) => (importable(p) ? [i] : []))))}>{tt("Select all")}</button>
                  <button type="button" className="link-btn" onClick={() => setTicks(source.id, () => new Set())}>{tt("Select none")}</button>
                </div>
                <ul className="fi-list">
                  {list.places.map((place, i) => {
                    const reason = skippedReason(place);
                    const usableRow = importable(place);
                    return (
                      <li key={i} className={`fi-row ${usableRow ? "" : "off"} ${place.missing ? "missing" : ""}`}>
                        <label>
                          <input type="checkbox" checked={usableRow && list.ticked.has(i)} disabled={!usableRow} onChange={() => toggle(source.id, i)} />
                          <span className="fi-text">
                            <span className="fi-label" title={place.label}>{place.label || place.path}</span>
                            <small className="fi-path" title={place.path}>{place.path}</small>
                          </span>
                          {place.duplicate && <em className="fi-tag">{tt("already in favorites")}</em>}
                          {place.missing && <em className="fi-tag warn">{tt("missing")}</em>}
                          {place.hidden && <em className="fi-tag">{tt("hidden there")}</em>}
                          {reason && <em className="fi-tag warn" title={reason}>{tt("Skipped")}: {reason}</em>}
                        </label>
                      </li>
                    );
                  })}
                </ul>
              </>)}
          </section>
        );
      })}
      <footer className="fi-foot">
        <span role="status">{tt("{ticked} of {usable} places selected", { ticked: total, usable })}</span>
        <span style={{ flex: 1 }} />
        <button type="button" className="btn primary" disabled={busy || choices.length === 0} onClick={() => void run()}>{tt("Import")}</button>
      </footer>
    </div>
  );
}

/** Settings › Window › Favourites and the command palette: detects what exists and shows the checklist. */
export function FavoritesImportDialog() {
  const [sources, setSources] = useState<ImportSourceInfo[] | null>(null);
  const [error, setError] = useState("");
  const modal = useRef<HTMLDivElement>(null);
  useEffect(() => {
    modal.current?.focus();
    let alive = true;
    api.favoritesImportDetect().then((s) => { if (alive) setSources(s); }).catch((e) => { if (alive) setError(String(e instanceof Error ? e.message : e)); });
    return () => { alive = false; };
  }, []);
  const close = () => A.openOverlay(null);
  return (
    <div className="backdrop center" onMouseDown={close}>
      <div className="modal fi-modal" tabIndex={-1} ref={modal} role="dialog" aria-label={tt("Import favorites")} onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Escape") close(); }}>
        <header className="fi-head">
          <b>{tt("Import favorites")}</b>
          <span>{tt("Takes over the places other file managers keep. Nothing in them is changed, and existing favorites stay as they are.")}</span>
        </header>
        <div className="fi-body">
          {error && <p className="fi-note fi-error" role="alert">{error}</p>}
          {!error && sources === null && <p className="fi-note" role="status">{tt("Looking for other file managers…")}</p>}
          {sources && sources.length === 0 && <p className="fi-note">{tt("No favorites of other file managers were found on this computer.")}</p>}
          {sources && sources.length > 0 && <FavoritesImportPanel sources={sources} onImported={close} />}
        </div>
        <footer className="fi-close"><button type="button" className="btn" onClick={close}>{tt("Close")}</button></footer>
      </div>
    </div>
  );
}
