import { mount } from "svelte";
import { invoke } from "@tauri-apps/api/core";
import "./app.css";
import App from "./App.svelte";

const app = mount(App, { target: document.getElementById("app")! });

// Show the window only after the first frame is painted, so there is no white flash.
requestAnimationFrame(() => requestAnimationFrame(() => void invoke("app_ready")));

export default app;
