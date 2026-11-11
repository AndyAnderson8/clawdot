# Memory and skills

How ClawDot remembers things for you, and how it uses skills written for other assistants. Requirements: REQ-MEM-001..005 and REQ-SKL-001..005 in [the product spec](../specs/0001-product-requirements.md).

## Memory that scales

Memory is off until you turn it on (Settings, Features). Then you can say "remember that my printer is called Pixel", or type a memory into the portal. Only your own words are saved: the assistant cannot save, change or forget a memory on its own, and anything that looks like a password or key is refused.

**Limits.** Up to 500 memories of 300 characters, 64 KB in all. That is about 150 KB of memory on the device, well inside the budget in [the resource spec](../specs/0002-resource-budget.md).

**What the assistant sees.** The assistant does not get all of them every time. Every turn, ClawDot picks the memories that fit what you just said and the sentence before it:

- With 20 memories or fewer, all of them are shown, as before.
- With more, the 12 best matches are shown, never more than 1,600 characters. Each keeps its true number, so "forget note 7" still means the same one. If nothing matches, the 3 newest are shown.
- The score counts words the memory shares with your request. Rare words count more than common ones, so "printer" outweighs "the". Newer memories win ties. There are no embeddings and no extra downloads; it all runs on the device in under a millisecond.
- The assistant can also look one up on request with its `recall` tool ("what is my printer called?"). `recall` only reads.

**Searching in the portal.** `GET /api/notes?q=printer` returns the matching memories, best first, with the `position` of each in the full list. The administrator is required.

**Failure modes.** A search with only common words ("what is the") matches nothing and returns an empty list. If memory is off, search and `recall` return nothing. A memory file edited by hand is trimmed to the limits on load.

## Skills from other assistants

A skill in the open `SKILL.md` format is a folder:

```
tea-timer/
  SKILL.md        header with name and description, then the instructions
  times.md        optional helper files: small plain text
  examples/one.txt
```

```
---
name: tea-timer
description: Use when the user wants to time a cup of tea.
---
Ask which tea, then set a timer for the usual time (see times.md).
```

**Importing.** In the Skills tab choose a skill folder. The portal sends the files to `POST /api/skills/import` as `{"format":"skill-folder","files":[{"path","text"},...]}` and ClawDot reads them. The answer includes `import`, which says how many skills were added and lists everything left out with the reason.

**What is kept.**

- `name` and `description` from the header. The description is kept apart from the instructions and tells the assistant when to use the skill. Instructions are up to 2,000 characters; a longer skill is skipped with that reason.
- `source`, the file it came from, so you can tell where it came from.
- Helper files, up to 8 plain-text files (`md`, `txt`, `json`, `yaml`, `yml`, `csv`, `tsv`, `toml`), 4,000 characters each and 16,000 in all.
- Ignored on purpose: `allowed-tools` and every other header key. A skill cannot give itself tools. Set the tool list yourself in the skill's editor.

**What is never done.** ClawDot does not run anything from a skill. Scripts (`.sh`, `.py`, `.js` and the like) and binaries are left out and listed. Helper files are given to the assistant as reference text, inside markers, and as much as fits in the skill budget (6,000 characters for all skills); the rest are only named. A skill still cannot turn off confirmation or the rules about untrusted text.

**Review before use.** Every imported skill arrives turned off and marked `needs_review`. Open it, read the instructions and files, and choose Accept (`POST /api/skills/accept`). Only then can you turn it on. Editing a skill does not accept it.

**Failure modes.** A name that already exists is skipped. More than 16 skills in total is refused. A request over 64 files or 384 KB is refused. A path that is hidden, deeper than 4 parts, or tries to leave the folder (`../`) is skipped.
