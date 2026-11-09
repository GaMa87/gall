import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import "./styles/app.css";
import { api } from "./api/ipc";

// Forward UI errors to the backend log file for diagnosis.
window.addEventListener("error", (e) => api.uiLog("error", `${e.message} @ ${e.filename}:${e.lineno}\n${e.error?.stack ?? ""}`).catch(() => {}));
window.addEventListener("unhandledrejection", (e) => api.uiLog("error", `unhandled: ${(e.reason instanceof Error ? e.reason.stack ?? e.reason.message : String(e.reason))}`).catch(() => {}));

// While an input method composes text (Japanese, Chinese, Korean, Vietnamese …), Enter,
// Esc, the arrows and letters belong to it: Enter picks the characters and must not also
// confirm a rename or run a shortcut. Registered first, so no other handler sees them.
// (Only isComposing: WebKitGTK with IBus reports keyCode 229 for ordinary keys too.)
window.addEventListener("keydown", (e) => {
  if (e.isComposing) e.stopImmediatePropagation();
}, true);

document.documentElement.dataset.theme = window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
