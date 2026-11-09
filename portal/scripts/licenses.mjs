// Fails if any installed npm package has a licence outside the allow-list (cargo-deny does not see npm).
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ALLOWED = new Set(["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "0BSD"]);
const nm = join(dirname(fileURLToPath(import.meta.url)), "..", "node_modules");

function packages(dir) {
  return readdirSync(dir, { withFileTypes: true }).filter((d) => d.isDirectory() && d.name !== ".bin").flatMap((d) =>
    d.name.startsWith("@") ? packages(join(dir, d.name)) : [join(dir, d.name)]);
}

const bad = [];
let n = 0;
for (const p of packages(nm)) {
  let meta;
  try { meta = JSON.parse(readFileSync(join(p, "package.json"), "utf8")); } catch { continue; }
  n++;
  const lic = typeof meta.license === "string" ? meta.license : meta.license?.type;
  const parts = String(lic ?? "UNKNOWN").replace(/[()]/g, "").split(/\s+(?:OR|AND)\s+/);
  if (!parts.every((x) => ALLOWED.has(x)) && !parts.some((x) => ALLOWED.has(x) && / OR /.test(String(lic)))) bad.push(`${meta.name}@${meta.version}: ${lic}`);
}
if (bad.length) { console.error("portal: licences outside the allow-list:\n  " + bad.join("\n  ")); process.exit(1); }
console.log(`portal: ${n} installed packages, all licences on the allow-list`);
