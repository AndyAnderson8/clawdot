import { render } from "preact";
import { App } from "./App.tsx";
import { get, load, start } from "./store.ts";

const root = document.getElementById("root");
if (!root) throw new Error("index.html lost its #root element");
render(<App />, root);
void start();
// Keep what is on screen fresh while the page is open and visible.
setInterval(() => {
  if (get().view === "app" && !document.hidden) void load();
}, 10000);
