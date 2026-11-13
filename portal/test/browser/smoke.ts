// Browser smoke test: drives the built page (crates/core/src/ui.html) in headless Chrome against a mock of the
// device API, under the real Content-Security-Policy. Run with `npm run test:browser` after `npm run build`.
//   CHROME=/path/to/chrome  choose the browser (otherwise the usual install paths are tried)
//   SHOTS=dir               also write phone-width screenshots of every screen to dir
// Without a browser the test is skipped locally and fails when CI is set.
// The flows share one browser and one mock device and run in this order; each lives in ./areas/<area>.ts and
// receives the shared harness (./harness.ts) as a context object.
import { findChrome } from "./chrome.ts";
import { createContext } from "./harness.ts";
import * as activity from "./areas/activity.ts";
import * as clock from "./areas/clock.ts";
import * as home from "./areas/home.ts";
import * as layout from "./areas/layout.ts";
import * as media from "./areas/media.ts";
import * as settingsAbout from "./areas/settings-about.ts";
import * as settingsFeatures from "./areas/settings-features.ts";
import * as settingsForm from "./areas/settings-form.ts";
import * as settingsGeneral from "./areas/settings-general.ts";
import * as settingsMemory from "./areas/settings-memory.ts";
import * as settingsNumbers from "./areas/settings-numbers.ts";
import * as settingsTheme from "./areas/settings-theme.ts";
import * as settingsVoice from "./areas/settings-voice.ts";
import * as signin from "./areas/signin.ts";
import * as tools from "./areas/tools.ts";
import * as updates from "./areas/updates.ts";
import * as webhooks from "./areas/webhooks.ts";

const chrome = findChrome();
if (!chrome) {
  const msg = "portal browser test: no Chrome found (set CHROME=/path/to/chrome)";
  if (process.env["CI"]) {
    console.error(msg);
    process.exit(1);
  }
  console.log(msg + "; skipped");
  process.exit(0);
}

const c = await createContext(chrome);

await signin.signIn(c);
await home.shellAndHome(c);
await clock.clock(c);
await media.media(c);
await activity.webhookMessages(c);
await settingsFeatures.features(c);
await settingsMemory.memory(c);
await activity.activityLog(c);
await tools.toolsSourcesSkills(c);
await webhooks.webhooks(c);
await settingsForm.settingsForm(c);
await settingsVoice.settingsVoice(c);
await settingsGeneral.settingsGeneral(c);
await settingsTheme.settingsTheme(c);
await layout.layout(c);
await settingsNumbers.settingsNumbers(c);
await settingsAbout.settingsAbout(c);
await activity.logs(c);
await updates.updates(c);
await layout.crossCutting(c);
await layout.lightScreenshots(c);

process.exit(await c.report());
