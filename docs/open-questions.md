# Open questions

Questions an agent hit while working that the maintainer should confirm. Each row records the default that was chosen so work could continue. Change the default by telling the next agent, or by amending the spec. Status is `open` until the maintainer answers.

| Date | Area | Question | Default chosen | Status |
|---|---|---|---|---|
| 2026-11-09 | Portal copy | Is "A voice assistant that runs on your own speaker." an acceptable tagline? The old one ("Your speaker's agent inbox...") was disliked. | Plain description of the product, shown only on the sign-in and setup screens; the signed-in header shows the device name instead of any tagline. | open |
| 2026-11-09 | Portal naming | Are the tab names Home, Reminders, Agents, Tools, Connections, Skills, Webhooks, Settings right? "Connections" stands for MCP servers; "Agents" replaces "inbox". | Use these names (REQ-CFG-026), with a one-sentence explanation of MCP at the top of Connections. | open |
| 2026-11-09 | Device name | The header needs a device name but no setting exists. | Use the host name (HOSTNAME, COMPUTERNAME, or /etc/hostname), else "ClawDot". A `name` key in the configuration file can come with editable settings. | open |
| 2026-11-09 | Settings | The Settings tab is read-only because only the admin password is persisted from the UI today; every other setting comes from flags and the hand-written config file. Should the portal write settings (time zone, model, persona, voice) to a separate file in the data directory that overrides the config file? | Read-only for now with an honest "edit the file and restart" note; no override file. | open |
| 2026-11-09 | Test coverage | The page has no automated browser test; behaviour was checked by hand in headless Chrome and through the JSON API tests. Add a headless-browser smoke test to CI? | No, not yet (a separate decision, see ui-stack.md). | open |
| 2026-11-09 | Page size | The portal HTML grew from 16 KB to about 49 KB uncompressed. Pre-compress the page (about 70 percent smaller) or accept it? | Accept; the page is served from memory and the browser does the work. Compression is independent and can follow. | open |
