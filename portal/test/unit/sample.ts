// A sample of what `GET /api/settings` returns, for the unit tests. What each setting is (its kind, limits, choices,
// group and live status) comes from test/fixtures/settings-meta.json, which crates/core/src/settings/tests.rs keeps equal
// to the table the device validates with; a test states only the values.
import { readFileSync } from "node:fs";
import type { SettingField, Settings } from "../../src/types.ts";

export type SettingMeta = Pick<SettingField, "kind" | "min" | "max" | "max_length" | "choices" | "live" | "group">;

export const META: Record<string, SettingMeta> = JSON.parse(
  readFileSync(new URL("../fixtures/settings-meta.json", import.meta.url), "utf8"),
) as Record<string, SettingMeta>;

/** A listing with the given settings: `[value, source]` for each key. Every key must be a real setting. */
export function sampleSettings(values: Record<string, [SettingField["value"], string]>): Settings {
  const fields: Record<string, SettingField> = {};
  for (const [key, [value, source]] of Object.entries(values)) {
    const meta = META[key];
    if (!meta) throw new Error(`${key} is not in the settings fixture`);
    fields[key] = { ...meta, value, source };
  }
  return { fields };
}
