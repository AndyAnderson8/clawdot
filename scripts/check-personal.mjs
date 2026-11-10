#!/usr/bin/env node
// Personal-data guard: fail when a tracked text file contains a word from a denylist of personal strings.
// The repository is public, so the denylist itself must not spell the words out. See docs/process/personal-data.md.
//
// Sources of the denylist:
//   1. scripts/personal-denylist.sha256  (tracked): one sha256 per line (hex) of a lowercase token. A trailing
//      "# comment" may say what kind of word it is (never the word itself).
//   2. .personal-denylist                (untracked, git-ignored): plain words, one per line. Matched as a
//      case-insensitive substring, so it also catches the word inside a longer one.
//   3. CLAWDOT_PERSONAL_DENYLIST         (environment): the same plain words, comma separated.
// Allowlist: scripts/personal-allowlist.txt, lines "<path> <sha256>" for a word that is legitimate in one place
// (for example the copyright holder in NOTICE). "*" is a wildcard inside the path.
//
// Tokens compared against the hashes: each run of letters and digits, each run joined by "-", "_" or "." (so
// "a-b" is checked as "a", "b" and "a-b"), and adjacent pairs inside longer runs. The report names the file and
// line and a hash prefix, never the word. `node scripts/check-personal.mjs --hash word...` prints hashes.
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";

const sha = (s) => createHash("sha256").update(s).digest("hex");

if (process.argv.includes("--hash")) {
  for (const w of process.argv.slice(process.argv.indexOf("--hash") + 1)) console.log(sha(w.toLowerCase()));
  process.exit(0);
}

process.chdir(execFileSync("git", ["rev-parse", "--show-toplevel"], { encoding: "utf8" }).trim());

const lines = (path) =>
  existsSync(path)
    ? readFileSync(path, "utf8")
        .split("\n")
        .map((l) => l.replace(/#.*/, "").trim())
        .filter(Boolean)
    : [];

const hashes = new Set(lines("scripts/personal-denylist.sha256").map((l) => l.toLowerCase()));
const plain = [...lines(".personal-denylist"), ...(process.env.CLAWDOT_PERSONAL_DENYLIST ?? "").split(",")]
  .map((w) => w.trim().toLowerCase())
  .filter(Boolean);

const escape = (p) => p.replace(/[.+?^${}()|[\]\\]/g, "\\$&");
const allow = lines("scripts/personal-allowlist.txt").map((l) => {
  const [glob, hash] = l.split(/\s+/);
  return { re: new RegExp("^" + glob.split("*").map(escape).join(".*") + "$"), hash: (hash ?? "").toLowerCase() };
});
const allowed = (file, hash) => allow.some((a) => a.hash === hash && a.re.test(file));

const skip = /(^|\/)(Cargo\.lock|package-lock\.json)$|\.(png|jpe?g|gif|ico|wav|ogg|opus|onnx|mww|tflite|bin|woff2?|gz|zip)$/i;
const files = execFileSync("git", ["ls-files", "-z"], { encoding: "utf8", maxBuffer: 1 << 28 })
  .split("\0")
  .filter((f) => f && !skip.test(f) && existsSync(f));

function tokens(low) {
  const out = new Set();
  for (const m of low.matchAll(/[a-z0-9]+(?:[-_.][a-z0-9]+)*/g)) {
    out.add(m[0]);
    const bits = m[0].split(/([-_.])/); // words at even indexes, separators at odd ones
    for (let i = 0; i < bits.length; i += 2) {
      out.add(bits[i]);
      if (i + 2 < bits.length) out.add(bits[i] + bits[i + 1] + bits[i + 2]);
    }
  }
  return out;
}

let bad = 0;
for (const f of files) {
  let text;
  try {
    text = readFileSync(f, "utf8");
  } catch {
    continue;
  }
  if (text.includes("\0")) continue;
  const rows = text.split("\n");
  for (let i = 0; i < rows.length; i++) {
    const low = rows[i].toLowerCase();
    for (const t of tokens(low)) {
      const h = sha(t);
      if (hashes.has(h) && !allowed(f, h)) {
        console.error(`personal data: ${f}:${i + 1} contains a denylisted word (hash ${h.slice(0, 12)})`);
        bad++;
      }
    }
    for (const w of plain) {
      const h = sha(w);
      if (low.includes(w) && !allowed(f, h)) {
        console.error(`personal data: ${f}:${i + 1} contains a word from your local denylist (hash ${h.slice(0, 12)})`);
        bad++;
      }
    }
  }
}
if (bad) {
  console.error(
    `personal data: ${bad} hit(s). Use a neutral example (docs/process/personal-data.md). ` +
      `If a word is legitimate in that file, add "<path> <hash>" to scripts/personal-allowlist.txt.`,
  );
  process.exit(1);
}
console.log(`personal data: ${files.length} tracked files clean`);
