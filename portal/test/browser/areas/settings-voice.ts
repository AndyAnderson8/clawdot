import assert from "node:assert/strict";
import type { Ctx } from "../harness.ts";

export async function settingsVoice(c: Ctx): Promise<void> {
  const { b, mock, open, T, eq, step } = c;
  await step(
    "Settings: the voice and language are chosen on the Voice page; the model comes from the provider",
    async () => {
      // Without a speech provider the language and the voice say so and cannot be changed.
      await open("open", "#/settings/voice");
      await b.until(`!!T.q('#f-speech_language')`, "the language box");
      await eq(`T.q('#f-speech_language').disabled`, true, "disabled without a speech provider");
      await eq(`T.text('#speechprovider').includes('No speech provider is set up')`, true, "and the page says why");
      await open("open", "#/settings/voice", { speechOn: true });
      await b.until(`!!T.q('#f-speech_language') && !T.q('#f-speech_language').disabled`, "the language box");
      await eq(`T.text('#speechprovider dd')`, "speech.example", "the speech service in use is shown");
      await eq(`T.q('#f-speech_language').tagName`, "SELECT", "a dropdown, not a code to type");
      await eq(
        `T.all('#f-speech_language option').some((o) => o.textContent === 'English (en)') && T.all('#f-speech_language option').some((o) => o.textContent === 'German (de)')`,
        true,
        "languages are named",
      );
      await eq(
        `T.q('#f-speech_language option').textContent`,
        "Detect it automatically",
        "automatic is the first choice",
      );
      await T(`set('#f-speech_language', 'de')`);
      await b.ev(`T.q('#f-speech_language').form.requestSubmit()`);
      await b.until(`T.text('#msg') === 'Saved.'`, "saved");
      assert.deepEqual(mock.log.filter((l) => l.method === "PUT" && l.path === "/api/settings").pop()!.body, {
        speech_language: "de",
      });
      // The voice, from the server's list, with a sample.
      await b.until(`T.q('#f-voice')?.tagName === 'SELECT'`, "the voice list");
      await eq(
        `T.all('#f-voice option').map((o) => o.textContent).filter((t) => t !== 'Choose a voice')`,
        ["alloy", "Nova (my voice)", "onyx"],
        "the voices on offer, a cloned one by the name it was given",
      );
      await eq(`T.q('#hearvoice').disabled`, true, "nothing to hear until a voice is chosen");
      await T(`set('#f-voice', 'nova')`);
      await eq(`T.q('#hearvoice').disabled`, false, "a choice can be heard at once, before it is saved");
      await T(`click('#hearvoice')`);
      await b.until(`T.text('#msg').startsWith('Playing a sample')`, "the sample is on its way");
      const heard = mock.log.filter((l) => l.path === "/api/speak").pop()!.body as { text: string; voice: string };
      assert.ok(heard.text.startsWith("Hello. This is how I sound."));
      assert.equal(heard.voice, "nova", "in the voice that was picked, not the saved one");
      assert.ok(
        !mock.log.some((l) => l.method === "PUT" && (l.body as { voice?: string }).voice === "nova"),
        "and nothing was saved by listening",
      );
      await b.ev(`T.q('#f-voice').form.requestSubmit()`);
      await b.until(`T.text('#msg') === 'Saved.'`, "saved");
      assert.deepEqual(mock.log.filter((l) => l.method === "PUT" && l.path === "/api/settings").pop()!.body, {
        voice: "nova",
      });
      await eq(`T.q('#sayform') !== null`, true, "the speaker test is on this page, not among the settings");
      // Saving reloads the settings and the list of voices with them, so wait for the list to be back.
      await b.until(`!!T.q('#f-voice option[value="nova"]')`, "the voices are listed again after the save");
      await eq(`T.q('#f-voice option[value="nova"]').textContent`, "Nova (my voice)", "its id is what is sent");

      // The speech service is a choice, and its key is pasted once and shown only as its ends.
      await eq(
        `T.all('#f-speech_provider option').map((o) => o.value)`,
        ["openai", "elevenlabs", "deepgram"],
        "three kinds of speech service",
      );
      await T(`set('#f-speech_provider', 'elevenlabs')`);
      await b.ev(`T.q('#f-speech_provider').form.requestSubmit()`);
      await b.until(`T.text('#msg').startsWith('Saved')`, "the choice is saved");
      assert.deepEqual(mock.log.filter((l) => l.method === "PUT" && l.path === "/api/settings").pop()!.body, {
        speech_provider: "elevenlabs",
      });
      await b.until(`!!T.q('#speechkey')`, "a box for the key");
      await T(`set('#speechkey', 'two words')`);
      await T(`click('#speechsave')`);
      await b.until(
        `T.text('#speechmsg').startsWith('A key is one line')`,
        "a key that cannot be one is refused, with why",
      );
      await T(`set('#speechkey', 'xi-abcdefghijklmnop9876')`);
      await T(`click('#speechsave')`);
      await b.until(`T.text('#speechhint') === 'xi-abc…9876'`, "kept, and shown only as its ends");
      await eq(`document.body.innerText.includes('defghijklmnop')`, false, "never the key itself");

      // Realtime voice: its settings, its key and a plain line about whether it can run.
      await eq(`T.text('#realtimestate').startsWith('Not ready')`, true, "says it is not ready, and what it needs");
      await T(`set('#realtimekey', 'sk-realtime-abcdefgh1234')`);
      await T(`click('#realtimesave')`);
      await b.until(`T.text('#realtimehint') === 'sk-rea…1234'`, "the realtime key is kept the same way");
      assert.ok(
        mock.log.some((l) => l.method === "PUT" && l.path === "/api/realtime/key"),
        "through its own call",
      );
      await T(`set('#f-realtime_enabled', 'true')`);
      await T(`set('#f-realtime_url', 'wss://rt.example/v1/realtime')`);
      await T(`set('#f-realtime_model', 'rt-model')`);
      await b.ev(`T.q('#f-realtime_enabled').form.requestSubmit()`);
      await b.until(`T.text('#msg').startsWith('Saved')`, "realtime settings saved");
      assert.deepEqual(mock.log.filter((l) => l.method === "PUT" && l.path === "/api/settings").pop()!.body, {
        realtime_enabled: true,
        realtime_url: "wss://rt.example/v1/realtime",
        realtime_model: "rt-model",
      });
      // A build without it says so and offers nothing that would do nothing.
      await open("open", "#/settings/voice", { speechOn: true, realtimeBuilt: false });
      await b.until(`!!T.q('#realtimestate')`, "the realtime card");
      await eq(`T.text('#realtimestate')`, "This build of ClawDot does not include realtime voice.", "says so");
      await eq(`T.q('#f-realtime_enabled').disabled && T.q('#realtimekey').disabled`, true, "and its controls are off");

      // Time zones show their offset, west to east.
      await open("open", "#/settings/general");
      await b.until(`!!T.q('#f-zone')`, "the zone list");
      const zones = await b.ev<string[]>(`T.all('#f-zone option').map((o) => o.textContent)`);
      assert.ok(zones.length > 100 && zones.every((z) => z.startsWith("(UTC")), "every zone starts with its offset");
      const offs = zones.map((z) => {
        const m = /^\(UTC(?:([+-])(\d\d):(\d\d))?\)/.exec(z)!;
        return m[1] ? (m[1] === "-" ? -1 : 1) * (Number(m[2]) * 60 + Number(m[3])) : 0;
      });
      assert.deepEqual(
        [...offs].sort((a, c) => a - c),
        offs,
        "ordered by offset",
      );
      assert.ok(
        zones.some((z) => z.includes("changes with daylight saving")),
        "zones that move say so",
      );

      // The assistant: provider, then its key, then the model as a list the provider supplied.
      await open("open", "#/settings/assistant");
      await b.until(`T.q('#f-llm_model')?.tagName === 'SELECT' && !T.q('#f-llm_model').disabled`, "the model list");
      await eq(
        `T.all('#f-llm_model option').map((o) => o.textContent).filter((t) => t !== 'Choose a model').sort()`,
        ["free/model-c", "model-a", "model-b"],
        "the models the provider offers",
      );
      const labels = await b.ev<string[]>(
        `T.all('#panel form.panel:first-of-type .slabel label').map((l) => l.textContent.replace('Optional', '').trim())`,
      );
      assert.deepEqual(
        labels.slice(0, 5),
        ["Provider", "API key", "Address", "Key variable", "Model"],
        "provider, then its key, then the rest",
      );
      await eq(`T.q('#loadmodels') === null`, true, "no button to press to see the models");
      await eq(
        `T.q('#f-llm_fallback_model') === null && T.q('#f-summary_model') === null`,
        true,
        "no backup or summary model to set",
      );
      // The key: shown only as its first and last few characters, replaced by pasting another, checked before it is kept.
      await T(`set('#provider', 'openrouter')`);
      await b.until(`!!T.q('#keyhint')`, "the saved key is shown");
      await eq(`T.text('#keyhint')`, "sk-or-…c9f2", "as its ends only");
      await b.ev(`window.__answer = () => true`);
      await T(`click('#keyremove')`);
      await b.until(`!!T.q('#apikey')`, "removed: a box to paste a key appears");
      await eq(`T.q('#f-llm_model').disabled`, true, "and the models wait for a key");
      await T(`set('#apikey', 'nope')`);
      await T(`click('#keycheck')`);
      await b.until(
        `T.text('#keymsg').startsWith('The service did not accept that key')`,
        "a key that is refused says so",
      );
      assert.ok(
        !mock.log.some(
          (l) => l.method === "PUT" && l.path === "/api/llm/key" && (l.body as { key: string }).key === "nope",
        ),
        "and is not kept",
      );
      await T(`set('#apikey', 'sk-or-v1-abcdefghijklmnop1234')`);
      await b.ev(
        `document.getElementById('apikey').dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }))`,
      );
      await b.until(`T.text('#keyhint') === 'sk-or-…1234'`, "Enter checks it, keeps it, and shows only its ends");
      await b.until(`!T.q('#f-llm_model').disabled`, "and the models can be chosen");
      await eq(`T.all('#f-llm_model option').length >= 3`, true, "from the list the provider gave");
      await eq(`document.body.innerText.includes('abcdefghijklmnop')`, false, "the page never shows the key itself");
      // The Settings tabs wrap instead of scrolling.
      await eq(`getComputedStyle(T.q('.tabs')).overflowX`, "visible", "no scrollbar on the tab strip");
    },
  );
}
