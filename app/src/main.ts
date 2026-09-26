import { mount } from "svelte";
import { invoke } from "@tauri-apps/api/core";
import App from "./App.svelte";
import "./app.css";

// Forward uncaught UI errors to the terminal running the app.
if ("__TAURI_INTERNALS__" in window) {
  const report = (message: string) => void invoke("ui_log", { level: "error", message }).catch(() => {});
  window.addEventListener("error", (e) => report(`${e.message} at ${e.filename}:${e.lineno}`));
  window.addEventListener("unhandledrejection", (e) => report(`unhandled rejection: ${JSON.stringify(e.reason) ?? e.reason}`));
}

export default mount(App, { target: document.getElementById("app")! });
