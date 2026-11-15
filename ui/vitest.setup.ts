// The unit tests assume a non-Mac platform (Ctrl labels, Alt is not a character key).
// Node reports the host's platform in `navigator.platform`, so a run on a Mac would
// otherwise behave differently from CI. Tests of the macOS behavior pass `mac` explicitly.
if (typeof globalThis.navigator === "undefined") Object.defineProperty(globalThis, "navigator", { value: {}, configurable: true });
Object.defineProperty(globalThis.navigator, "platform", { value: "Linux x86_64", configurable: true });
