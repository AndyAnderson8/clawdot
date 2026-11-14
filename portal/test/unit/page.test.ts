import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

test("the built page keeps the licence notice that Preact's MIT licence asks for", () => {
  const page = readFileSync(new URL("../../../crates/core/src/ui.html", import.meta.url), "utf8");
  assert.match(page, /Preact[^*]*MIT License/);
});
