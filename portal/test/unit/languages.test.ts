import assert from "node:assert/strict";
import { test } from "node:test";
import { languageName, languageOptions } from "../../src/languages.ts";

test("languages are named, in alphabetical order, with their code", () => {
  assert.equal(languageName("en"), "English");
  assert.equal(languageName("de"), "German");
  const opts = languageOptions();
  assert.ok(opts.some((o) => o.code === "en" && o.label === "English (en)"));
  const names = opts.map((o) => o.label);
  assert.deepEqual(
    [...names].sort((a, b) => a.localeCompare(b)),
    names,
  );
});

test("a language that is saved but not on the list is still shown", () => {
  assert.ok(languageOptions("pt-BR").some((o) => o.code === "pt-BR"));
  assert.equal(languageOptions("en").filter((o) => o.code === "en").length, 1);
});
