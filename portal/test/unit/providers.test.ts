import assert from "node:assert/strict";
import { test } from "node:test";
import { PROVIDERS, providerFor } from "../../src/providers.ts";

test("a known address is its provider, with or without a trailing slash", () => {
  assert.equal(providerFor("https://openrouter.ai/api/v1")?.id, "openrouter");
  assert.equal(providerFor("https://api.openai.com/v1/")?.id, "openai");
  assert.equal(providerFor("  https://api.anthropic.com/v1  ")?.id, "anthropic");
});

test("nothing chosen is not a provider, and an unknown address is something else", () => {
  assert.equal(providerFor(""), null);
  assert.equal(providerFor("   "), null);
  assert.equal(providerFor("https://llm.example/v1")?.id, "custom");
  assert.equal(providerFor("https://")?.id, "custom");
});

test("every preset says where its key lives by name, never a key", () => {
  for (const p of PROVIDERS) {
    assert.ok(!/sk-|[A-Za-z0-9]{32,}/.test(p.keyEnv), p.id + " keyEnv is a variable name");
    if (p.url) assert.ok(p.url.startsWith("https://") || p.id === "local", p.id + " uses a secure address");
  }
  assert.equal(PROVIDERS.at(-1)?.id, "custom", "something else comes last");
});
