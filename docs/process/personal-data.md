# Personal data and examples

This repository is public. Everything tracked in it, and everything in its history, can be read by anyone. Examples, tests, scenarios, comments and docs must therefore never carry details of the maintainer's own life or equipment. They tend to leak from the working context (a path on the maintainer's PC, the name of a device at home, a local time zone), not from the product.

## What not to put in examples

- Real names, user names, e-mail addresses and account names of the maintainer or of anyone else (the licence holder in `NOTICE` and the repository owner in URLs are the legitimate exceptions).
- Names of the maintainer's machines, devices, containers, networks (including the Wi-Fi name) and home services.
- Home or office IP addresses and host names. Use the documentation ranges or generic private values (`192.168.1.20`, `10.0.0.5`, `example.com`, `device.local`).
- Paths from a real machine (`C:\Users\<name>\...`, `/home/<name>/...`). Use `/home/user/project`.
- Device serial numbers, MAC addresses, tokens, keys, certificates, even expired ones. Test secrets must be obviously fake (`ghp_0123456789abcdefghij`).
- The maintainer's real time zone, city or other location as an example. Use a neutral one.
- Real audio of a real voice, real inbox events, real logs.

## Neutral examples to use

| Need | Use |
|---|---|
| A thing the user names in a note | "my printer is called Pixel", "the porch light is called Beacon" |
| A time zone | `America/New_York`, `Europe/Berlin` |
| A working directory | `/home/user/project` |
| A user or owner on a code host | `octocat` |
| A LAN address | `192.168.1.20` |
| A host | `example.com`, `host.example` |
| A person | "Alex", "Sam" |

## The guard

`scripts/check-personal.mjs` runs in `scripts/check.sh` (needs only `node`) and in the `docs` job of CI. It fails when a tracked text file contains a denylisted word. The denylist must not spell the words out in a public repository, so there are three sources:

1. `scripts/personal-denylist.sha256` is tracked and holds the SHA-256 of each lowercase word, with a comment saying only what kind of word it is. The guard hashes every word and every `-`, `_` or `.` joined run in each file and compares. Add a word with `node scripts/check-personal.mjs --hash <word>` and paste the hash in. A hash of a short common word can be guessed, so treat it as a tripwire, not as secrecy.
2. `.personal-denylist` is git-ignored and local: plain words, one per line, matched as case-insensitive substrings. Use it for anything you do not want even hashed in the repo (a Wi-Fi name, a serial number). It only protects the checkout it is in.
3. `CLAWDOT_PERSONAL_DENYLIST` is the same list as a comma separated environment variable, for CI secrets or a shell profile.

Reports name the file and line and a hash prefix, never the word, so a CI log does not repeat it.

A word that is legitimate in one place (the copyright holder in `NOTICE`) goes in `scripts/personal-allowlist.txt` as `<path> <sha256>`. The entry is per file and per word; do not allow a word everywhere.

## If something personal is already committed

Removing it from the current tree is a normal commit. It stays in history, which this project does not rewrite (shared history is never rewritten, [git-workflow.md](git-workflow.md)). Treat anything that is a secret, as opposed to merely personal, as leaked: rotate it. Whether to rewrite history anyway is the maintainer's decision alone.
