// The browser comparison worker: loads the WebAssembly module of `gall-compare` (built with the `wasm` feature and
// installed next to the app, see docs/compare-integrations.md) and answers requests of `compareTransport.ts`.
// The module comes from the app's own assets, so no network is needed once they are installed; if it is missing the
// worker says so in answer to the probe and the page keeps using the desktop comparison.
import type { WorkerReply, WorkerRequest } from "./compareTransport";

interface Module { default: (url?: string) => Promise<unknown>; compare: (a: Uint8Array, b: Uint8Array, options: string) => string; merge: (base: Uint8Array, left: Uint8Array, right: Uint8Array, request: string) => string }

const MODULE_URL = "/compare-wasm/gall_compare.js";
const BINARY_URL = "/compare-wasm/gall_compare_bg.wasm";

let loading: Promise<Module> | null = null;
const load = (): Promise<Module> => (loading ??= (async () => {
  // The path is resolved at run time, never bundled: the file exists only where the module was installed. The Vite
  // development server rewrites every dynamic import into `…?import`, which it refuses for files of the public
  // directory, so there the script is fetched and imported from a blob (the module is told its binary explicitly).
  const url = new URL(MODULE_URL, self.location.href).href;
  let source = url, revoke = "";
  if (import.meta.env.DEV) {
    const response = await fetch(url);
    if (!response.ok) throw new Error(`${url}: ${response.status}`);
    source = revoke = URL.createObjectURL(new Blob([await response.text()], { type: "text/javascript" }));
  }
  const m = (await import(/* @vite-ignore */ source)) as Module;
  if (revoke) URL.revokeObjectURL(revoke);
  await m.default(new URL(BINARY_URL, self.location.href).href);
  return m;
})());

const post = (reply: WorkerReply) => (self as unknown as { postMessage: (m: WorkerReply) => void }).postMessage(reply);

self.onmessage = async (event: MessageEvent<WorkerRequest | { type: "probe" }>) => {
  const message = event.data;
  let module: Module;
  try {
    module = await load();
  } catch (e) {
    if (message.type === "probe") post({ type: "failed", reason: `the comparison module is not installed: ${String(e)}` });
    return;
  }
  if (message.type === "probe") return post({ type: "ready" });
  let json: string;
  try {
    json = message.type === "merge"
      ? module.merge(new Uint8Array(message.base ?? new ArrayBuffer(0)), new Uint8Array(message.a), new Uint8Array(message.b), message.options)
      : module.compare(new Uint8Array(message.a), new Uint8Array(message.b), message.options);
  } catch (e) {
    // A trap or a failed allocation inside the module: say so instead of leaving the page waiting.
    json = JSON.stringify({ kind: "error", message: `the comparison failed: ${String(e)}` });
  }
  post({ type: "answer", id: message.id, json });
};
