import { expect, it, vi } from "vitest";
import { ForecastQueue, forecastBranches, forecastFiles, type Forecast } from "./gitForecast";
import type { GitReference } from "./gitRepository";

const value = (known = true, ...files: string[]): Forecast => ({ known, files });
const settle = async () => { for (let i = 0; i < 10; i++) await Promise.resolve(); };

it("asks a few at a time, once for each pair, and keeps the answers", async () => {
  const queue = new ForecastQueue(2), open: ((v: Forecast) => void)[] = [], seen: string[] = [];
  const run = vi.fn(() => new Promise<Forecast>((resolve) => { open.push(resolve); }));
  for (const key of ["a", "b", "c", "a"]) queue.ask(key, run, () => seen.push(key));
  expect(run).toHaveBeenCalledTimes(2);
  open[0](value()); await settle();
  expect(seen).toEqual(["a", "a"]);
  expect(run).toHaveBeenCalledTimes(3);
  queue.ask("a", run, () => seen.push("again"));
  expect(seen).toEqual(["a", "a", "again"]);
  expect(run).toHaveBeenCalledTimes(3);
});
it("what has not started can be forgotten and an unknown answer is asked again", async () => {
  const queue = new ForecastQueue(1), open: ((v: Forecast) => void)[] = [];
  const run = vi.fn(() => new Promise<Forecast>((resolve) => { open.push(resolve); }));
  queue.ask("a", run, () => {}); queue.ask("b", run, () => {});
  queue.clear();
  open[0](value(false)); await settle();
  expect(run).toHaveBeenCalledTimes(1);
  expect(queue.get("a")).toBeUndefined();
  queue.ask("a", run, () => {});
  expect(run).toHaveBeenCalledTimes(2);
});
it("a failure does not block the queue", async () => {
  const queue = new ForecastQueue(1), seen: string[] = [];
  queue.ask("a", () => Promise.reject(new Error("no")), () => seen.push("a"));
  queue.ask("b", () => Promise.resolve(value()), () => seen.push("b"));
  await settle(); await settle();
  expect(seen).toEqual(["b"]);
});
it("leaves out HEAD, other references and repositories with too many branches", () => {
  const ref = (name: string, id: string, kind = "branch") => ({ name, id, kind, fullName: name }) as GitReference;
  const refs = [ref("main", "1"), ref("same", "1"), ref("x", "2"), ref("origin/x", "3", "remote")];
  expect(forecastBranches(refs, "1", 50).map((r) => r.name)).toEqual(["x"]);
  expect(forecastBranches(refs, "1", 2)).toEqual([]);
  expect(forecastBranches(refs, "1", 0)).toEqual([]);
  expect(forecastBranches(refs, "", 50)).toEqual([]);
  expect(forecastFiles(value(true, "a", "b", "c"), 2)).toEqual({ names: ["a", "b"], more: 1 });
});
