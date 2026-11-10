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

export function snippets(base: string, name: string, adapter: string, tok: string | undefined): [string, string][] {
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
  if (adapter === "claude-code") {
    const hook = (ev: string) =>
      `    "${ev}": [{ "hooks": [{ "type": "http", "url": "${url}", "headers": { "Authorization": "Bearer ${t}" }, "timeout": 5 }] }]`;
    out.push([
      "Coding agent hooks: add to ~/.claude/settings.json",
      '{\n  "hooks": {\n' + hook("Stop") + ",\n" + hook("Notification") + "\n  }\n}",
    ]);
    out.push([
      "Let your voice replies reach the agent (run on the computer where it works)",
      `CLAWDOT_TOKEN=${t} clawdot-bridge --url ${base} --endpoint ${name}`,
    ]);
  }
  return out;
}
