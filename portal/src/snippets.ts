// Ready-to-paste setup text for one input (a webhook). Pure functions, unit-tested.
// The token is the real one only right after it was made; otherwise the text says YOUR_TOKEN.

/** What an adapter is called in the page. Coding-agent hooks are one adapter in a neutral list. */
export const ADAPTERS: [value: string, label: string][] = [
  ["generic", "Plain JSON (works with anything)"],
  ["claude-code", "Coding agent hooks (Claude Code format)"],
];

export function adapterName(adapter: string): string {
  return adapter === "claude-code" ? "Coding agent hooks" : "Plain JSON";
}

const BODY = '{"title":"Backup finished","summary":"12 GB copied"}';

/** `setup` is the endpoint's `setup` from the device: its adapter's set-up lines, with the placeholders unfilled. */
export function snippets(
  base: string,
  name: string,
  adapter: string,
  tok: string | undefined,
  setup: string[],
): [string, string][] {
  const url = base + "/hook/" + name;
  const t = tok || "YOUR_TOKEN";
  const out: [string, string][] = [
    ["curl, from a terminal", `curl -X POST ${url} \\\n  -H 'Authorization: Bearer ${t}' \\\n  -d '${BODY}'`],
    [
      "PowerShell",
      `Invoke-RestMethod -Method Post -Uri ${url} \`\n  -Headers @{ Authorization = "Bearer ${t}" } \`\n  -ContentType "application/json" \`\n  -Body '${BODY}'`,
    ],
    [
      "GitHub Actions: add as the last step of a job",
      "- name: Tell the speaker\n  if: always()\n  run: |\n    curl -fsS -X POST " +
        url +
        ' \\\n      -H "Authorization: Bearer ${{ secrets.CLAWDOT_TOKEN }}" \\\n      -d \'{"title":"Build ${{ job.status }}","summary":"${{ github.repository }}"}\'',
    ],
    [
      "Home Assistant: add to configuration.yaml",
      `rest_command:\n  tell_speaker:\n    url: "${url}"\n    method: post\n    headers:\n      Authorization: "Bearer ${t}"\n    content_type: "application/json"\n    payload: '{{ {"title": title, "summary": summary} | to_json }}'`,
    ],
  ];
  // The plain adapter's own lines are the curl example above. Any other adapter brings its set-up text from the
  // device (the same lines `clawdot --create-endpoint` prints), which is filled in here and not written again.
  if (adapter !== GENERIC_ADAPTER) out.push(...adapterSnippets(setup, { public: base, endpoint: name, token: t }));
  return out;
}

/** The adapter that needs no set-up beyond the snippets above. */
const GENERIC_ADAPTER = "generic";

/** `{public}`, `{endpoint}` and `{token}` replaced in one pass, so a value that holds a placeholder is not expanded. */
export function fillSetup(text: string, values: { public: string; endpoint: string; token: string }): string {
  return text.replace(/\{(public|endpoint|token)\}/g, (_, k: "public" | "endpoint" | "token") => values[k]);
}

/**
 * An adapter's set-up lines as titled snippets. Blank lines separate the parts; the first line of a part (without its
 * closing colon) is the title and the lines after it are the text, without the indent they are printed with.
 */
export function adapterSnippets(
  setup: string[],
  values: { public: string; endpoint: string; token: string },
): [string, string][] {
  const parts: string[][] = [];
  let part: string[] = [];
  for (const line of setup) {
    if (line.trim() === "") {
      parts.push(part);
      part = [];
    } else part.push(line);
  }
  parts.push(part);
  const out: [string, string][] = [];
  for (const [head, ...body] of parts) {
    if (head === undefined || !body.length) continue;
    const indent = Math.min(...body.map((l) => l.length - l.trimStart().length));
    out.push([
      fillSetup(head.replace(/:$/, ""), values),
      fillSetup(body.map((l) => l.slice(indent)).join("\n"), values),
    ]);
  }
  return out;
}
