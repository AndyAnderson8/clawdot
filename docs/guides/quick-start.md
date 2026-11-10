# Quick start

From nothing to a speaker that answers, on a computer. A Raspberry Pi and an Echo Dot follow the same steps once ClawDot runs on them; for the Dot see the [trial guide](b0-techo5-trial-plan.md).

## What you need

- ClawDot (the `clawdot` program for your system).
- An account with a language-model service (OpenRouter, OpenAI, Anthropic, or a model on your own network) and its key.
- For voice, a speech-to-text and text-to-speech service. [Local speech](local-speech.md) shows a free one that runs on your own machine.

## 1. Start it

Keep the key out of files: put it in an environment variable, and tell ClawDot the variable's name later.

```
OPENROUTER_API_KEY=your-key clawdot
```

It prints the address of the portal, by default `http://127.0.0.1:3335/`. To reach it from your phone, start it with `--listen 0.0.0.0:3335` and use the computer's network address.

## 2. Choose a password

Open the portal. The first time, it asks you to choose an admin password (at least 8 characters). Until you do, anyone on your private network who opens the page could, so do it straight away. There is no recovery code: if you forget the password, hold the speaker's action button for 10 seconds, or run `clawdot --reset-admin` on a computer ([forgotten password](forgot-password.md)).

## 3. Choose who answers

Settings, Assistant. Under "Who answers", choose your service. ClawDot fills in the address, a first model and the usual name of the key variable; change the variable's name if you used a different one, then Save. Home stops showing "Language model not set" when it works.

## 4. Hear it

Voice needs a speech service in the configuration file ([local speech](local-speech.md)). Once it is set, Settings, Assistant, "Test the speaker" says a sentence out loud, and tells you plainly if something is missing and where to fix it.

## 5. Make it yours

- **Features** switches abilities on or off: remember things, say the plan first, say what it is doing, check for updates, practice mode.
- **Clock** sets timers, alarms and reminders, or you can just ask.
- **Settings, Inputs** lets other programs send events the speaker reads out; **Tools** and **Skills** teach it new things.

## Keeping it up to date

ClawDot looks for a newer version about once a day and shows it on Home. Settings, About, Updates shows what is new and installs it when you confirm ([updates](updates.md)). Settings, About also saves your settings as a file to move to another device.
