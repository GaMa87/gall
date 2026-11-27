#!/usr/bin/env node
// Runs the browser comparison in a real (headless) Chromium against the installed WebAssembly module, once under the
// Vite development server and once from a production build, and checks the answers, the cancel, the frame budget of
// the main thread at 200,000 lines and that nothing but the page's own origin was ever asked for (no network needed).
//
//   node scripts/build-compare-wasm.mjs && node scripts/compare-browser-check.mjs [--dev-only | --build-only]
//
// Needs `chromium` (or CHROMIUM=/path) and `npm --prefix ui ci`. Exit 0 only when every check passes.
import { spawn } from "node:child_process";
import { createServer as httpServer } from "node:http";
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const ui = join(root, "ui");
const chromium = process.env.CHROMIUM || "chromium";
/** The 60 Hz frame budget (ms) with the slack of a headless frame clock; a longer gap means the page was blocked. */
const FRAME_P95_MS = 20, FRAME_MAX_MS = 100;
const failures = [];
const check = (ok, what) => { console.log(`${ok ? "ok  " : "FAIL"} ${what}`); if (!ok) failures.push(what); };

if (!existsSync(join(ui, "public", "compare-wasm", "gall_compare_bg.wasm"))) {
  console.error("compare-browser-check: the module is not installed; run node scripts/build-compare-wasm.mjs first");
  process.exit(2);
}
const vite = await import(pathToFileURL(join(ui, "node_modules", "vite", "dist", "node", "index.js")).href);

const MIME = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".wasm": "application/wasm", ".css": "text/css", ".map": "application/json" };
function serveDirectory(dir) {
  const server = httpServer((req, res) => {
    const path = join(dir, decodeURIComponent(new URL(req.url, "http://x").pathname));
    if (!path.startsWith(dir) || !existsSync(path) || !statSync(path).isFile()) { res.writeHead(404).end(); return; }
    res.writeHead(200, { "content-type": MIME[extname(path)] ?? "application/octet-stream" }).end(readFileSync(path));
  });
  return new Promise((ok) => server.listen(0, "127.0.0.1", () => ok({ url: `http://127.0.0.1:${server.address().port}/compare-check.html`, close: () => server.close() })));
}

/** Minimal DevTools-protocol client over the WebSocket of Node. */
async function launch(port) {
  const dir = mkdtempSync(join(tmpdir(), "gall-compare-check-"));
  const child = spawn(chromium, ["--headless=new", "--no-sandbox", "--disable-gpu", `--remote-debugging-port=${port}`, `--user-data-dir=${dir}`,
    // Everything but the loopback host cannot be resolved: a request to the network fails instead of being answered.
    "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1", "about:blank"], { stdio: "ignore" });
  let version;
  for (let i = 0; i < 100 && !version; i++) {
    try { version = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json(); } catch { await new Promise((r) => setTimeout(r, 100)); }
  }
  if (!version) { child.kill(); throw new Error("chromium did not start"); }
  const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
  const ws = new WebSocket(targets.find((t) => t.type === "page").webSocketDebuggerUrl);
  await new Promise((ok) => ws.addEventListener("open", ok, { once: true }));
  let next = 0;
  const waiting = new Map(), listeners = [];
  ws.addEventListener("message", (e) => {
    const m = JSON.parse(e.data);
    if (m.id && waiting.has(m.id)) { const { ok, fail } = waiting.get(m.id); waiting.delete(m.id); m.error ? fail(new Error(m.error.message)) : ok(m.result); }
    else listeners.forEach((l) => l(m));
  });
  const send = (method, params = {}) => new Promise((ok, fail) => { const id = ++next; waiting.set(id, { ok, fail }); ws.send(JSON.stringify({ id, method, params })); });
  return { send, on: (l) => listeners.push(l), close: () => { ws.close(); child.kill(); rmSync(dir, { recursive: true, force: true }); } };
}

async function runIn(label, url, port) {
  console.log(`\n== ${label} (${url})`);
  const browser = await launch(port);
  const requested = [], problems = [];
  browser.on((m) => {
    if (m.method === "Network.requestWillBeSent") requested.push(m.params.request.url);
    if (m.method === "Runtime.exceptionThrown") problems.push(m.params.exceptionDetails.exception?.description ?? m.params.exceptionDetails.text);
    if (m.method === "Runtime.consoleAPICalled" && m.params.type === "error") problems.push(m.params.args.map((a) => a.value ?? a.description).join(" "));
    if (m.method === "Log.entryAdded" && m.params.entry.level === "error") problems.push(`${m.params.entry.text} ${m.params.entry.url ?? ""}`);
  });
  await browser.send("Network.enable");
  await browser.send("Runtime.enable");
  await browser.send("Log.enable");
  await browser.send("Page.navigate", { url });
  // The development server compiles the page on first load: wait until the check is defined.
  for (let i = 0; i < 100; i++) {
    const ready = await browser.send("Runtime.evaluate", { expression: "typeof window.__compareCheck === 'function'", returnByValue: true });
    if (ready.result.value) break;
    await new Promise((r) => setTimeout(r, 200));
  }
  const evaluated = await browser.send("Runtime.evaluate", { expression: "window.__compareCheck().then((r) => JSON.stringify(r))", awaitPromise: true, returnByValue: true, timeout: 120000 });
  browser.close();
  if (evaluated.exceptionDetails) { check(false, `${label}: the page failed: ${evaluated.exceptionDetails.exception?.description ?? evaluated.exceptionDetails.text}`); return; }
  const r = JSON.parse(evaluated.result.value);
  console.log(JSON.stringify(r));
  check(r.probe === null, `${label}: the worker starts with the installed module`);
  check(r.text.added === 2 && r.text.removed === 1 && r.text.hunks === 2, `${label}: text comparison answer`);
  check(r.binary?.identical === false && r.binary.firstDifference === 2, `${label}: binary classification`);
  check(r.identical?.added === 0 && r.identical.removed === 0, `${label}: identical text`);
  check(r.merge.open === "text" && r.merge.conflicts === 1, `${label}: merge finds the conflict`);
  check(r.merge.resolved === "resolved" && r.merge.text === "a\nours\ntheirs\nc\n", `${label}: merge resolves with both`);
  check(r.merge.unresolved === "unresolved" && r.merge.withoutBase === "text", `${label}: unresolved and missing-base merge answers`);
  check(r.cancel.first === "cancelled" && r.cancel.next === "text", `${label}: cancel ends the worker and the next comparison works`);
  check(r.scale.kind === "text" && r.scale.p95 <= FRAME_P95_MS && r.scale.max <= FRAME_MAX_MS, `${label}: frame budget at 200,000 lines (p95 ${r.scale.p95.toFixed?.(1)} ms, max ${r.scale.max.toFixed?.(1)} ms over ${r.scale.frames} frames, ${r.scale.ms} ms total)`);
  check(r.limit === "tooLarge", `${label}: an input over the limit is refused`);
  const origin = new URL(url).origin;
  const outside = requested.filter((u) => !u.startsWith(origin) && !u.startsWith("blob:") && !u.startsWith("data:"));
  check(outside.length === 0, `${label}: no request left the page's own origin${outside.length ? ` (${outside.slice(0, 3)})` : ""}`);
  check(problems.length === 0, `${label}: no page errors${problems.length ? ` (${problems.slice(0, 3)})` : ""}`);
}

const args = process.argv.slice(2);
if (!args.includes("--build-only")) {
  const server = await vite.createServer({ root: ui, configFile: join(ui, "vite.config.ts"), server: { host: "127.0.0.1", port: 5199, strictPort: true }, logLevel: "error" });
  await server.listen();
  await runIn("development server", "http://127.0.0.1:5199/compare-check.html", 9333);
  await server.close();
}
if (!args.includes("--dev-only")) {
  const out = mkdtempSync(join(tmpdir(), "gall-compare-dist-"));
  await vite.build({ root: ui, configFile: join(ui, "vite.config.ts"), logLevel: "error", build: { outDir: out, emptyOutDir: true, sourcemap: false, rollupOptions: { input: join(ui, "compare-check.html") } } });
  const served = await serveDirectory(out);
  await runIn("production build", served.url, 9334);
  served.close();
  rmSync(out, { recursive: true, force: true });
}
console.log(failures.length ? `\n${failures.length} check(s) failed` : "\nall checks passed");
process.exit(failures.length ? 1 : 0);
