// Ready-made choices for the provider, so a person picks a name instead of typing an address. Every one speaks the
// OpenAI-style chat protocol that ClawDot uses. The key is pasted on the Assistant page and kept on the device.
export interface Provider {
  id: string;
  name: string;
  /** The address of the service; empty for "custom", where the person types it. */
  url: string;
  /** A sensible first model; empty when it depends on what the person has installed. */
  model: string;
  /** The usual name of the environment variable that holds the key; empty when none is needed. */
  keyEnv: string;
  note: string;
}

export const PROVIDERS: Provider[] = [
  {
    id: "openrouter",
    name: "OpenRouter",
    url: "https://openrouter.ai/api/v1",
    model: "openai/gpt-4o-mini",
    keyEnv: "OPENROUTER_API_KEY",
    note: "One account that reaches models from many makers.",
  },
  {
    id: "openai",
    name: "OpenAI",
    url: "https://api.openai.com/v1",
    model: "gpt-4o-mini",
    keyEnv: "OPENAI_API_KEY",
    note: "The makers of ChatGPT.",
  },
  {
    id: "anthropic",
    name: "Anthropic",
    url: "https://api.anthropic.com/v1",
    model: "claude-sonnet-5-5",
    keyEnv: "ANTHROPIC_API_KEY",
    note: "The makers of Claude, through their OpenAI-compatible address.",
  },
  {
    id: "custom",
    name: "Custom",
    url: "",
    model: "",
    keyEnv: "",
    note: "Any other service that speaks the OpenAI style of request, or a model you run yourself. You give its address.",
  },
];

/** Which provider an address belongs to; an empty address is "not chosen yet", anything unknown is "custom". */
export function providerFor(url: string): Provider | null {
  const trim = (x: string) => x.trim().replace(/\/+$/, "");
  const u = trim(url);
  if (!u) return null;
  for (const p of PROVIDERS) if (p.url !== "" && trim(p.url) === u) return p;
  return PROVIDERS.at(-1) ?? null;
}
