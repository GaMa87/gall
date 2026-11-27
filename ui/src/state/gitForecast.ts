import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { forecastBranches, forecastFiles, ForecastQueue, type Forecast } from "../core/gitForecast";
import type { GitReference } from "../core/gitRepository";
import { tt } from "../i18n";

const queue = new ForecastQueue();
let tickets = Date.now();
const key = (path: string, ours: string, theirs: string) => `${path}\0${ours}\0${theirs}`;

/** Forecasts of merging each branch into HEAD, filled in after the sidebar is on screen; closing it stops them. */
export function useForecasts(path: string, refs: GitReference[] | undefined, head: string, shown: boolean, limit: number): ReadonlyMap<string, Forecast> {
  const branches = useMemo(() => shown && refs ? forecastBranches(refs, head, limit) : [], [refs, head, shown, limit]);
  const [found, setFound] = useState<{ head: string; map: ReadonlyMap<string, Forecast> }>({ head: "", map: new Map() });
  const ids = branches.map((ref) => ref.id).join(" ");
  useEffect(() => {
    if (!ids) return;
    let alive = true;
    const asked: number[] = [];
    const timer = setTimeout(() => {
      for (const id of new Set(ids.split(" "))) queue.ask(key(path, head, id), () => {
        const ticket = ++tickets;
        asked.push(ticket);
        return invoke<Forecast>("git_forecast", { path, ours: head, theirs: id, ticket });
      }, (value) => { if (alive && value.known) setFound((old) => ({ head, map: new Map(old.head === head ? old.map : []).set(id, value) })); });
    }, 400);
    return () => {
      alive = false;
      clearTimeout(timer);
      queue.clear();
      for (const ticket of asked) invoke("git_forecast_cancel", { ticket }).catch(() => {});
    };
  }, [path, head, ids]);
  return found.head === head ? found.map : new Map();
}

/** For a confirmation: which files would conflict, or "" when that is not known soon enough. */
export async function forecastText(path: string, ours: string, theirs: string): Promise<string> {
  const known = queue.get(key(path, ours, theirs));
  const ticket = ++tickets;
  // The confirmation does not wait longer than this.
  const value = known ?? await Promise.race([
    invoke<Forecast>("git_forecast", { path, ours, theirs, ticket }).catch(() => null),
    new Promise<null>((resolve) => setTimeout(() => { invoke("git_forecast_cancel", { ticket }).catch(() => {}); resolve(null); }, 700)),
  ]);
  if (!value?.known) return "";
  if (!value.files.length) return tt("Forecast: no conflicts.");
  const { names, more } = forecastFiles(value);
  return `${tt(value.files.length === 1 ? "Forecast: {count} file in conflict" : "Forecast: {count} files in conflict", { count: value.files.length })}\n${names.join("\n")}${more ? `\n${tt("and {count} more", { count: more })}` : ""}`;
}
