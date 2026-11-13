// The longest realistic content for every list and field, so a layout that only fits short text shows itself.
// Merged into the mock device's state (objects one level deep, arrays replaced).
import { endpoint, settingMeta } from "./mock-api.ts";

const t = Math.floor(Date.now() / 1000);
const longName = "extremely_long_tool_name_for_the_hallway_light_controller_v2";
const longUrl =
  "https://home-assistant.example-very-long-hostname.local:8123/api/services/light/turn_on/living_room_ceiling_lamp";
const inputName = "a-very-long-input-name-of-32-chars";

export const STRESS: Record<string, unknown> = {
  settings: {
    public_url: { ...settingMeta("public_url"), value: longUrl, source: "saved", saved: true },
    persona: {
      ...settingMeta("persona"),
      value: "Averyveryverylongwordwithoutanybreaks".repeat(4),
      source: "saved",
      saved: true,
    },
    llm_url: { ...settingMeta("llm_url"), value: longUrl, source: "file" },
    llm_key_env: {
      ...settingMeta("llm_key_env"),
      value: "A_VERY_LONG_ENVIRONMENT_VARIABLE_NAME_FOR_THE_KEY",
      source: "file",
    },
    device_name: {
      ...settingMeta("device_name"),
      value: "A device name that is as long as the box allows it to be",
      source: "saved",
    },
  },
  schedule: [
    {
      id: 1,
      kind: "timer",
      label: "a timer with a very long name that goes on and on and on and on and on",
      fire_at: t + 600,
      seconds_left: 600,
      repeat: null,
    },
    {
      id: 2,
      kind: "reminder",
      label: "Unbrokenreminderlabel".repeat(6),
      fire_at: t + 9000,
      seconds_left: 9000,
      repeat: null,
    },
  ],
  endpoints: [endpoint(inputName, "claude-code", t - 100)],
  events: [
    {
      id: 1,
      endpoint: inputName,
      source: "a-build-server-with-a-long-name-as-source",
      title: "A very long event title that keeps going and going so that it has to wrap on a phone ".repeat(2),
      text: "https://example.com/" + "segment/".repeat(30),
      state: "new",
      received_at: t - 120,
    },
  ],
  tools: [
    {
      name: longName,
      description:
        "Turn the light on or off in any room of the house, by name, with a brightness between zero and one hundred",
      enabled: true,
      confirm: true,
      source: "http",
      origin: "portal",
      deletable: true,
      tier: "costly",
      tier_default: "costly",
      tier_set: true,
      tier_options: ["read_only", "reversible", "costly"],
    },
    {
      name: "mcp_" + longName,
      description: "Toggle",
      enabled: false,
      confirm: true,
      source: "mcp",
      server: "home-assistant-with-a-long-connection-name",
      deletable: false,
    },
  ],
  mcp: [
    {
      name: "home-assistant-with-a-long-connection-name",
      url: longUrl,
      enabled: true,
      connected: false,
      error: "connection refused by " + longUrl,
      deletable: true,
      origin: "portal",
      token_env: "A_VERY_LONG_ENVIRONMENT_VARIABLE_NAME_FOR_THE_TOKEN",
      tools: [
        {
          name: "mcp_" + longName,
          description: "Toggle",
          enabled: false,
          confirm: true,
          source: "mcp",
          server: "home",
        },
      ],
    },
  ],
  notes: [
    { id: 1, text: "Unbrokennotetext".repeat(14), ts: t - 3600 },
    { id: 2, text: "A long note about where everything is kept in the house, ".repeat(4), ts: t - 7200 },
  ],
  notesOn: true,
  skills: [
    {
      name: "A skill with a very long name that has to wrap on a narrow screen",
      instructions: "When I say good night, turn off every light listed here, one by one. ".repeat(3),
      tools: [longName, "another_long_tool_name_for_a_second_light"],
      enabled: true,
    },
  ],
  audit: [
    {
      v: 1,
      id: 1,
      ts: t - 60,
      day: 1,
      turn: 1,
      trigger: "event:" + inputName,
      tool: longName,
      tier: "costly",
      decision: "ran",
      status: "ok",
      args: { url: longUrl, token: "[redacted]" },
      result: longUrl + "?" + "q=1&".repeat(40),
    },
  ],
  logs: [{ ts: t - 30, level: "error", text: "could not reach " + longUrl + " " + "x".repeat(120) }],
};
