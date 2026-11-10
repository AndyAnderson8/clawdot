// Ready-made choices for "who answers questions", so a person picks a name instead of typing an address.
// Every one speaks the OpenAI-style chat protocol that ClawDot uses. The key is never stored by ClawDot: only the
// NAME of the environment variable that holds it is.
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
    note: "One key for many models from many makers.",
  },
  {
    id: "openai",
    name: "OpenAI",
    url: "https://api.openai.com/v1",
    model: "gpt-4o-mini",
    keyEnv: "OPENAI_API_KEY",
    note: "ChatGPT models.",
  },
  {
    id: "anthropic",
    name: "Anthropic",
    url: "https://api.anthropic.com/v1",
    model: "claude-sonnet-5-5",
    keyEnv: "ANTHROPIC_API_KEY",
    note: "Claude models, through Anthropic's OpenAI-compatible address.",
  },
  {
    id: "local",
    name: "A model on your own network",
    url: "http://localhost:11434/v1",
    model: "",
    keyEnv: "",
    note: "Ollama, LM Studio or similar. Change the address if it runs on another machine.",
  },
  {
    id: "custom",
    name: "Something else",
    url: "",
    model: "",
    keyEnv: "",
    note: "Any service with an OpenAI-style address.",
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
