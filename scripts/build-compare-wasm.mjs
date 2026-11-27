#!/usr/bin/env node
// Builds the browser module of the comparison (`gall-compare` for wasm32) into ui/public/compare-wasm, where the
// comparison worker loads it from the app's own assets (`/compare-wasm/gall_compare.js`, no network).
//
//   node scripts/build-compare-wasm.mjs            build, or fail with the reason
//   node scripts/build-compare-wasm.mjs --optional build when the tools are installed, else say why and exit 0
//
// `npm --prefix ui run build` runs it with --optional, so a machine without the wasm target or the wasm-bindgen CLI
// still builds the application (the browser path of the window then reports that the module is not installed).
// Node rather than a shell script, so that it runs the same on Linux, macOS and Windows.
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const optional = process.argv.includes("--optional");
const out = join(root, "ui", "public", "compare-wasm");

function stop(message) {
  if (optional) {
    console.log(`compare-wasm: skipped (${message})`);
    process.exit(0);
  }
  console.error(`compare-wasm: ${message}`);
  process.exit(1);
}

function run(command, args) {
  const result = spawnSync(command, args, { cwd: root, encoding: "utf8" });
  return result.error ? null : result;
}

// The wasm32 target of the Rust toolchain.
const targets = run("rustup", ["target", "list", "--installed"]);
if (!targets || targets.status !== 0 || !targets.stdout.split(/\r?\n/).includes("wasm32-unknown-unknown")) {
  stop("the Rust target wasm32-unknown-unknown is not installed: rustup target add wasm32-unknown-unknown");
}

// The wasm-bindgen CLI must be the version of the crate that the module is built with (the two must agree).
const lock = readFileSync(join(root, "Cargo.lock"), "utf8");
const wanted = /name = "wasm-bindgen"\nversion = "([^"]+)"/.exec(lock)?.[1];
if (!wanted) stop("wasm-bindgen is not in Cargo.lock");
const cli = run("wasm-bindgen", ["--version"]);
const have = cli && cli.status === 0 ? /wasm-bindgen (\S+)/.exec(cli.stdout)?.[1] : null;
if (have !== wanted) {
  stop(`the wasm-bindgen CLI is ${have ?? "not installed"}, the crate is ${wanted}: cargo install wasm-bindgen-cli --version ${wanted} --locked`);
}

execFileSync("cargo", ["build", "-p", "gall-compare", "--target", "wasm32-unknown-unknown", "--release", "--features", "wasm"], { cwd: root, stdio: "inherit" });
const wasm = join(root, "target", "wasm32-unknown-unknown", "release", "gall_compare.wasm");
if (!existsSync(wasm)) stop(`the build did not produce ${wasm}`);
mkdirSync(out, { recursive: true });
execFileSync("wasm-bindgen", ["--target", "web", "--out-dir", out, wasm], { cwd: root, stdio: "inherit" });
for (const file of ["gall_compare.js", "gall_compare_bg.wasm"]) {
  if (!existsSync(join(out, file))) stop(`wasm-bindgen did not produce ${file}`);
}
console.log(`compare-wasm: installed in ${out}`);
