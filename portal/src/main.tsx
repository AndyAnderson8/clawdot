import { render } from "preact";
import { App } from "./App.tsx";
import { get, load, start } from "./store.ts";

render(<App />, document.getElementById("root")!);
void start();
// Keep what is on screen fresh while the page is open and visible.
setInterval(() => {
  if (get().view === "app" && !document.hidden) void load();
}, 10000);
